//! The effective-POM loader: parent-chain fetching, inheritance merging,
//! `${...}` interpolation and `dependencyManagement` consolidation including
//! BOM (`scope=import`) expansion.
//!
//! Order mirrors Maven's model builder, verified in ADR 0003: parent POMs are
//! fetched recursively, inheritance is applied **before** interpolation (so
//! `${project.version}` inside a parent POM resolves to *this* project's
//! version — the behaviour platform BOMs rely on), then the merged model is
//! interpolated, then `dependencyManagement` is consolidated with BOM
//! imports expanded recursively.
//!
//! Nothing here decides resolution: this module returns an *effective model*
//! ([`EffectivePom`]); M3's resolver builds the graph on top of it.

use std::collections::{HashMap, HashSet};

use jx_common::error::{JxError, Kind};

use crate::client::Client;
use crate::coords::Coordinate;
use crate::xml::{Dependency, RawPom, RawRepository};

/// Bounds for parent chains and BOM import depth: safeguards make malformed
/// or hostile remotes terminate instead of wedging the resolver.
const MAX_CHAIN_DEPTH: usize = 32;
const MAX_IMPORT_DEPTH: usize = 32;
const MAX_INTERPOLATION_DEPTH: usize = 16;

/// A POM identity, kept separately so interpolation can reference it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Coords {
    pub group_id: String,
    pub artifact_id: String,
    pub version: String,
}

impl Coords {
    /// The coordinate this identity names, as a POM (used for parents and
    /// imported BOMs: the POM is what is fetched, never the artifact).
    fn as_pom(&self) -> Coordinate {
        Coordinate {
            group_id: self.group_id.clone(),
            artifact_id: self.artifact_id.clone(),
            version: self.version.clone(),
            classifier: None,
            extension: "pom".to_string(),
        }
    }
}

/// The effective model of one POM: coordinates resolved through the parent
/// chain, properties merged and interpolated, dependencies with versions
/// filled from (interpolated, import-expanded) `dependencyManagement`.
#[derive(Clone, Debug, PartialEq)]
pub struct EffectivePom {
    pub coords: Coords,
    /// `<packaging>`, `jar` default.
    pub packaging: String,
    /// Interpolated merged properties, in document order (parents first).
    pub properties: Vec<(String, String)>,
    /// The project's direct dependencies. Versions are concrete where the
    /// consolidated `dependencyManagement` supplies one; `version: None`
    /// means "no version was determined" and is a resolution error the
    /// caller must diagnose. Classifier/scope/type/exclusions inherit from
    /// the management entry when the dependency omits them; `optional` stays
    /// as written (the M2 data model does not distinguish "absent" from
    /// `false`, so management-supplied `optional` is not propagated; ADR 0003).
    pub dependencies: Vec<Dependency>,
    /// Consolidated `dependencyManagement`: the parent chain's entries first,
    /// then this POM's own (overriding same `group:artifact` entries), then
    /// BOM imports appended for coordinates not already managed. Interpolated.
    /// `scope=import` entries stay in the list as markers.
    pub dependency_management: Vec<Dependency>,
    /// Merged, de-duplicated `<repositories>` + `<pluginRepositories>`: the
    /// parent chain's entries first, then this POM's own. The *resolver*
    /// decides whether and in what order these extend the configured
    /// repositories (M3, per ADR 0003).
    pub repositories: Vec<RawRepository>,
}

impl EffectivePom {
    /// The managed entry for `group:artifact`, if any. The lookup key is
    /// `(groupId, artifactId)` — Maven's management lookup ignores
    /// classifier/type.
    pub fn managed(&self, group_id: &str, artifact_id: &str) -> Option<&Dependency> {
        self.dependency_management
            .iter()
            .find(|d| d.group_id == group_id && d.artifact_id == artifact_id)
    }
}

/// Loads effective POMs through a repository [`Client`]. Parent POMs and
/// imported BOMs are fetched through the same checksum-verified pipeline as
/// everything else, and the content cache makes repeated loads cheap.
pub struct PomLoader<'a> {
    client: &'a Client,
}

/// What `${...}` may resolve to: merged properties, this POM's coordinates,
/// and the nearest ancestor's (`project.parent.*`).
struct Ctxt<'x> {
    props: &'x HashMap<String, String>,
    coords: &'x Coords,
    parent: Option<&'x Coords>,
}

impl<'a> PomLoader<'a> {
    pub fn new(client: &'a Client) -> PomLoader<'a> {
        PomLoader { client }
    }

    /// Load the effective model for `coord`.
    pub fn load(&self, coord: &Coordinate) -> Result<EffectivePom, JxError> {
        let mut visited: HashSet<String> = HashSet::new();
        self.load_inner(coord, &mut visited, 0)
    }

    /// `visited` tracks imported-BOM coordinates for cycle detection; parent
    /// chains get their own set inside [`Self::fetch_chain`].
    fn load_inner(
        &self,
        coord: &Coordinate,
        visited: &mut HashSet<String>,
        depth: usize,
    ) -> Result<EffectivePom, JxError> {
        if depth > MAX_IMPORT_DEPTH || !visited.insert(coord.key()) {
            return Err(
                JxError::new(Kind::Resolve, "BOM imports cycle or nest absurdly deep")
                    .at(coord.key())
                    .hint("a BOM imports itself directly or through another BOM; jx stops at 32 levels"),
            );
        }
        let chain = self.fetch_chain(coord)?;

        // Inheritance merge, parent-before-child; then interpolation of the
        // assembled model (Maven's order — see the module comment).
        let merged = merge_chain(&chain)?;
        let own = own_coords(&chain)?;
        let parent = parent_coords(&chain, &own);

        // Properties resolve in document order, so a later property may
        // reference an earlier one; each resolved value enters the context
        // immediately.
        let mut props: HashMap<String, String> = HashMap::new();
        let mut properties: Vec<(String, String)> = Vec::with_capacity(merged.properties.len());
        for (name, value) in &merged.properties {
            let ctxt = Ctxt {
                props: &props,
                coords: &own,
                parent: parent.as_ref(),
            };
            let expanded = interpolate_text(value, &ctxt, 0)?;
            props.insert(name.clone(), expanded.clone());
            properties.push((name.clone(), expanded));
        }

        let interpolate = |dep: &Dependency| -> Result<Dependency, JxError> {
            interpolate_dependency(
                dep,
                &Ctxt {
                    props: &props,
                    coords: &own,
                    parent: parent.as_ref(),
                },
            )
        };
        let mut dependencies = Vec::with_capacity(merged.dependencies.len());
        for dep in &merged.dependencies {
            dependencies.push(interpolate(dep)?);
        }
        let mut management = Vec::with_capacity(merged.dependency_management.len());
        for dep in &merged.dependency_management {
            management.push(interpolate(dep)?);
        }

        // BOM imports expand against the interpolated list, so
        // `${project.version}`-shaped import coordinates work.
        expand_imports(&mut management, self.client, visited, depth)?;

        Ok(EffectivePom {
            coords: own,
            packaging: merged.packaging,
            properties,
            dependencies: apply_management(dependencies, &management),
            dependency_management: management,
            repositories: merged.repositories,
        })
    }

    /// Fetch the POM and its whole parent chain (checksums verified like
    /// every other fetch). Returns parent-first: `chain[0]` is the rootmost
    /// ancestor, `chain[last]` is `coord` itself.
    fn fetch_chain(&self, coord: &Coordinate) -> Result<Vec<RawPom>, JxError> {
        let mut walked: Vec<RawPom> = Vec::new(); // child … root while walking up
        let mut seen: HashSet<String> = HashSet::new();
        let mut current = coord.clone();
        loop {
            if walked.len() > MAX_CHAIN_DEPTH || !seen.insert(current.key()) {
                return Err(JxError::new(
                    Kind::Resolve,
                    "the parent chain cycles or nests absurdly deep",
                )
                .at(coord.key())
                .hint("the chain of <parent> declarations is broken; jx stops after 32 levels"));
            }
            let outcome = self.client.fetch_pom(&current).map_err(|e| {
                e.at(format!(
                    "POM of {} (as a parent of its chain)",
                    current.key()
                ))
            })?;
            let text = String::from_utf8_lossy(&outcome.bytes).into_owned();
            let raw = crate::xml::parse_pom(&text, &current.key())?;
            if raw.artifact_id != current.artifact_id {
                return Err(JxError::new(
                    Kind::Resolve,
                    format!(
                        "the repository served a POM naming artifact `{}`, not `{}`",
                        raw.artifact_id, current.artifact_id
                    ),
                )
                .at(current.key())
                .hint(
                    "check the repository serves this layout path for the coordinate jx asked for",
                ));
            }
            let parent = raw.parent.clone();
            walked.push(raw);
            match parent {
                Some(parent) => {
                    if parent.group_id.is_empty()
                        || parent.artifact_id.is_empty()
                        || parent.version.is_empty()
                    {
                        return Err(JxError::new(
                            Kind::Resolve,
                            "the <parent> block is missing groupId/artifactId/version",
                        )
                        .at(current.key())
                        .hint("the parent block needs all three coordinates to be fetchable"));
                    }
                    current = Coords {
                        group_id: parent.group_id,
                        artifact_id: parent.artifact_id,
                        version: parent.version,
                    }
                    .as_pom();
                }
                None => {
                    walked.reverse();
                    return Ok(walked);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Inheritance merge (uninterpolated — Maven merges before it interpolates)
// ---------------------------------------------------------------------------

/// The merge target: coordinates stay `Option` because they are resolved
/// *after* merging, together with interpolation.
#[derive(Debug)]
struct MergedPom {
    packaging: String,
    properties: Vec<(String, String)>,
    dependencies: Vec<Dependency>,
    dependency_management: Vec<Dependency>,
    repositories: Vec<RawRepository>,
}

/// Merge the parent chain into one raw model: the POM under load wins,
/// ancestors supply everything else. Fields the resolver consumes only.
fn merge_chain(chain: &[RawPom]) -> Result<MergedPom, JxError> {
    let Some(child) = chain.last() else {
        return Err(JxError::new(Kind::Internal, "the POM chain is empty"));
    };
    let mut properties: Vec<(String, String)> = Vec::new(); // child wins duplicates
    let mut deps = OrderedMap::default();
    let mut management = OrderedMap::default();
    let mut repositories: Vec<RawRepository> = Vec::new(); // parents first, child last

    for pom in chain {
        for (name, value) in &pom.properties {
            if let Some(existing) = properties.iter_mut().find(|(n, _)| n == name) {
                existing.1 = value.clone();
            } else {
                properties.push((name.clone(), value.clone()));
            }
        }
        for dep in &pom.dependencies {
            deps.upsert(dep);
        }
        for dep in &pom.dependency_management {
            management.upsert(dep);
        }
        for repo in &pom.repositories {
            if !repositories
                .iter()
                .any(|r| r.id == repo.id && r.url == repo.url)
            {
                repositories.push(repo.clone());
            }
        }
    }

    Ok(MergedPom {
        packaging: child.packaging.clone(),
        properties,
        dependencies: deps.raise(),
        dependency_management: management.raise(),
        repositories,
    })
}

/// Insertion-ordered overwrite map keyed by `group:artifact` (Maven's
/// dependency key): a child's entry replaces an ancestor's whole entry.
#[derive(Default)]
struct OrderedMap {
    entries: Vec<Entry>,
}

struct Entry {
    key: String,
    dependency: Dependency,
}

impl OrderedMap {
    fn upsert(&mut self, dep: &Dependency) {
        let key = format!("{}:{}", dep.group_id, dep.artifact_id);
        match self.entries.iter().position(|e| e.key == key) {
            Some(index) => self.entries[index].dependency = dep.clone(),
            None => self.entries.push(Entry {
                key,
                dependency: dep.clone(),
            }),
        }
    }

    fn raise(self) -> Vec<Dependency> {
        self.entries.into_iter().map(|e| e.dependency).collect()
    }
}

/// Nearest-defined value: the POM under load first, then ancestors child →
/// root. This is Maven's inheritance rule for `groupId`/`version` (they may
/// live anywhere in the chain; `artifactId` is never inherited).
fn nearest_defined(chain: &[RawPom], field: impl Fn(&RawPom) -> Option<String>) -> Option<String> {
    chain.iter().rev().find_map(field)
}

/// The child's own field first, else the nearest-defined one up the chain.
/// A function (not a closure) because closures cannot take `impl Fn` args.
fn inherit_field(
    chain: &[RawPom],
    field: impl Fn(&RawPom) -> Option<String> + Copy,
) -> Option<String> {
    match chain.split_last() {
        Some((child, ancestors)) => field(child).or_else(|| nearest_defined(ancestors, field)),
        None => None,
    }
}

/// The coordinates of the POM under load after inheritance.
fn own_coords(chain: &[RawPom]) -> Result<Coords, JxError> {
    let child = chain.last().expect("chain is non-empty");
    let group_id = inherit_field(chain, |p| p.group_id.clone()).ok_or_else(|| {
        JxError::new(Kind::Resolve, "no groupId anywhere in the parent chain")
            .at(child.artifact_id.clone())
            .hint("the POM and every ancestor of it omit a groupId; one of them must declare it")
    })?;
    let version = inherit_field(chain, |p| p.version.clone()).ok_or_else(|| {
        JxError::new(Kind::Resolve, "no version anywhere in the parent chain")
            .at(child.artifact_id.clone())
            .hint("the POM and every ancestor of it omit a version; one of them must declare it")
    })?;
    Ok(Coords {
        group_id,
        artifact_id: child.artifact_id.clone(),
        version,
    })
}

/// The coordinates of the nearest ancestor, for `project.parent.*`. When the
/// ancestor's identity is absent even up its own chain — structurally broken
/// POMs — the project's own values stand in, only so interpolation does not
/// stall; nothing else consumes a broken ancestry.
fn parent_coords(chain: &[RawPom], own: &Coords) -> Option<Coords> {
    if chain.len() < 2 {
        return None;
    }
    let ancestor_chain = &chain[..chain.len() - 1];
    let direct = &ancestor_chain[ancestor_chain.len() - 1];
    let group_id = direct
        .group_id
        .clone()
        .or_else(|| {
            nearest_defined(&ancestor_chain[..ancestor_chain.len() - 1], |p| {
                p.group_id.clone()
            })
        })
        .unwrap_or_else(|| own.group_id.clone());
    let version = direct
        .version
        .clone()
        .or_else(|| {
            nearest_defined(&ancestor_chain[..ancestor_chain.len() - 1], |p| {
                p.version.clone()
            })
        })
        .unwrap_or_else(|| own.version.clone());
    Some(Coords {
        group_id,
        artifact_id: direct.artifact_id.clone(),
        version,
    })
}

// ---------------------------------------------------------------------------
// Interpolation
// ---------------------------------------------------------------------------

/// Maven's `${...}` rules, as implemented by `StringSearchInterpolator`:
/// unresolved placeholders stay literally as written, names starting with a
/// dot are never touched ("${x}" is left alone), and a value that itself
/// contains a placeholder expands recursively. Depth is bounded; past the
/// bound jx reports a circular definition instead of looping.
fn interpolate_text(text: &str, ctxt: &Ctxt, depth: usize) -> Result<String, JxError> {
    if depth > MAX_INTERPOLATION_DEPTH {
        return Err(JxError::new(
            Kind::Resolve,
            "circular or too-deep property interpolation",
        )
        .at(ctxt.coords.group_id.clone())
        .hint("a property references itself (directly or through others); shorten the chain or inline the value"));
    }
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            // Unterminated placeholder: keep the tail as written.
            out.push_str(&rest[start..]);
            return Ok(out);
        };
        let name = &after[..end];
        rest = &after[end + 1..];
        if name.starts_with('.') {
            out.push_str("${");
            out.push_str(name);
            out.push('}');
            continue;
        }
        match lookup(name, ctxt) {
            Some(value) => out.push_str(&interpolate_text(&value, ctxt, depth + 1)?),
            None => {
                out.push_str("${");
                out.push_str(name);
                out.push('}');
            }
        }
    }
    out.push_str(rest);
    Ok(out)
}

/// Precedence (Maven's model sources): structural `project.*`/`pom.*`
/// coordinates first, then declared properties, then the legacy bare
/// spellings of old POMs.
fn lookup(name: &str, ctxt: &Ctxt) -> Option<String> {
    for (prefix, allow_parent) in [("project.", true), ("pom.", true)] {
        if let Some(rest) = name.strip_prefix(prefix) {
            return lookup_project(rest, allow_parent, ctxt);
        }
    }
    if let Some(v) = ctxt.props.get(name) {
        return Some(v.clone());
    }
    // Legacy aliases from Maven 2-era POMs.
    match name {
        "groupId" => Some(ctxt.coords.group_id.clone()),
        "artifactId" => Some(ctxt.coords.artifact_id.clone()),
        "version" => Some(ctxt.coords.version.clone()),
        // The Maven 2-era legacy spellings include `parent.version` and
        // friends; like the bare `version` alias they only fill in when no
        // declared property has that name (checked above).
        rest => lookup_project(rest, true, ctxt),
    }
}

/// `project.*` lookups; `parent.` sub-names are admitted only from the
/// explicit prefixes.
fn lookup_project(rest: &str, allow_parent: bool, ctxt: &Ctxt) -> Option<String> {
    if let Some(sub) = rest.strip_prefix("parent.") {
        if !allow_parent {
            return None;
        }
        let parent = ctxt.parent?;
        return match sub {
            "groupId" => Some(parent.group_id.clone()),
            "artifactId" => Some(parent.artifact_id.clone()),
            "version" => Some(parent.version.clone()),
            _ => None,
        };
    }
    match rest {
        "groupId" => Some(ctxt.coords.group_id.clone()),
        "artifactId" => Some(ctxt.coords.artifact_id.clone()),
        "version" => Some(ctxt.coords.version.clone()),
        _ => None,
    }
}

fn interpolate_dependency(dep: &Dependency, ctxt: &Ctxt) -> Result<Dependency, JxError> {
    let text = |value: &Option<String>| -> Result<Option<String>, JxError> {
        match value.as_deref() {
            None => Ok(None),
            Some(v) => Ok(Some(interpolate_text(v, ctxt, 0)?)),
        }
    };
    Ok(Dependency {
        version: text(&dep.version)?,
        classifier: text(&dep.classifier)?,
        ..dep.clone()
    })
}

// ---------------------------------------------------------------------------
// dependencyManagement consolidation and BOM imports
// ---------------------------------------------------------------------------

/// Expand `scope=import` management entries. Each import loads the target
/// BOM (whose own imports expand first, inside its load), and its
/// consolidated table is appended — only for coordinates not already
/// managed, because imports never override nearer declarations (Maven's
/// rule). The imports themselves stay in the list as markers.
fn expand_imports(
    management: &mut Vec<Dependency>,
    client: &Client,
    visited: &mut HashSet<String>,
    depth: usize,
) -> Result<(), JxError> {
    // Expand against a snapshot so entries spliced below are never
    // re-expanded by this pass (their own load already expanded them).
    let imports: Vec<Dependency> = management
        .iter()
        .filter(|d| d.scope.as_deref() == Some("import"))
        .cloned()
        .collect();
    for import in imports {
        if import.package_type.as_deref() != Some("pom") {
            return Err(JxError::new(
                Kind::Resolve,
                format!(
                    "the `dependencyManagement` import {}:{} must declare `<type>pom</type>`",
                    import.group_id, import.artifact_id
                ),
            )
            .hint("Maven only serves a BOM through `scope=import` with the `pom` type"));
        }
        let version = import.version.clone().ok_or_else(|| {
            JxError::new(
                Kind::Resolve,
                format!(
                    "the BOM import {}:{} has no version",
                    import.group_id, import.artifact_id
                ),
            )
            .hint("Maven's BOM imports need an explicit `<version>` to fetch and merge")
        })?;
        let coord = Coords {
            group_id: import.group_id.clone(),
            artifact_id: import.artifact_id.clone(),
            version,
        }
        .as_pom();
        let bom = PomLoader { client }.load_inner(&coord, visited, depth + 1)?;
        for dep in bom.dependency_management {
            if dep.scope.as_deref() == Some("import") {
                continue;
            }
            let key = (dep.group_id.clone(), dep.artifact_id.clone());
            if !management
                .iter()
                .any(|d| d.group_id == key.0 && d.artifact_id == key.1)
            {
                management.push(dep);
            }
        }
    }
    Ok(())
}

/// Fill absent version/classifier/type/scope from the consolidated
/// management table, and union its exclusions in (Maven merges both sides).
/// A dependency left with `version: None` is left for the resolver to
/// diagnose — the error then references the coordinates jx is resolving.
fn apply_management(dependencies: Vec<Dependency>, management: &[Dependency]) -> Vec<Dependency> {
    dependencies
        .into_iter()
        .map(|dep| {
            match management
                .iter()
                .find(|m| m.group_id == dep.group_id && m.artifact_id == dep.artifact_id)
            {
                None => dep,
                Some(managed) => Dependency {
                    version: dep.version.clone().or(managed.version.clone()),
                    classifier: dep.classifier.clone().or(managed.classifier.clone()),
                    package_type: dep.package_type.clone().or(managed.package_type.clone()),
                    scope: dep.scope.clone().or(managed.scope.clone()),
                    exclusions: merge_exclusions(&dep.exclusions, &managed.exclusions),
                    ..dep
                },
            }
        })
        .collect()
}

/// Union preserving order: the dependency's own exclusions first, then the
/// management side's.
fn merge_exclusions(
    own: &[(String, String)],
    managed: &[(String, String)],
) -> Vec<(String, String)> {
    let mut out = own.to_vec();
    for exclusion in managed {
        if !out.contains(exclusion) {
            out.push(exclusion.clone());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pom(group_id: Option<&str>, artifact_id: &str, version: Option<&str>) -> RawPom {
        RawPom {
            parent: None,
            group_id: group_id.map(str::to_string),
            artifact_id: artifact_id.to_string(),
            version: version.map(str::to_string),
            packaging: "jar".to_string(),
            properties: Vec::new(),
            dependencies: Vec::new(),
            dependency_management: Vec::new(),
            repositories: Vec::new(),
            distribution_url: None,
            source_text: String::new(),
        }
    }

    fn coords(g: &str, a: &str, v: &str) -> Coords {
        Coords {
            group_id: g.to_string(),
            artifact_id: a.to_string(),
            version: v.to_string(),
        }
    }

    fn interp(
        text: &str,
        props: &HashMap<String, String>,
        own: &Coords,
        parent: Option<&Coords>,
    ) -> Result<String, JxError> {
        let ctxt = Ctxt {
            props,
            coords: own,
            parent,
        };
        interpolate_text(text, &ctxt, 0)
    }

    #[test]
    fn nearest_defined_walks_children_first() {
        // grandparent declares the group, nobody else does: the child of a
        // three-deep chain inherits it, and the parent's coordinates fall
        // back to the chain (version falls back to the child's).
        let grand = pom(Some("grand"), "root", Some("1.0"));
        let parent = pom(None, "middle", None);
        let child = pom(None, "leaf", None);
        let chain = vec![grand, parent, child];
        let own = own_coords(&chain).expect("inherited coordinates");
        assert_eq!(
            own,
            coords("grand", "leaf", "1.0"),
            "artifactId is never inherited; group/version come from the nearest definer"
        );
        let parent = parent_coords(&chain, &own).expect("middle exists");
        assert_eq!(parent, coords("grand", "middle", "1.0"));
    }

    #[test]
    fn own_coords_errors_when_the_chain_declares_nothing() {
        let orphan = pom(None, "orphan", None);
        let error = own_coords(&[orphan]).expect_err("no identity anywhere");
        assert!(error.message.contains("no groupId"), "{error}");
    }

    #[test]
    fn interpolation_resolves_recursively_and_keeps_unresolved_placeholders() {
        let own = coords("g", "a", "1.2.3");
        let parent = coords("gp", "p", "9");
        let mut props = HashMap::new();
        props.insert("x.v".to_string(), "2.3.1".to_string());
        props.insert("outer".to_string(), "A${x.v}B>${inner}".to_string());
        props.insert("inner".to_string(), "C".to_string());

        assert_eq!(
            interp("plain", &props, &own, Some(&parent)).unwrap(),
            "plain"
        );
        assert_eq!(
            interp("${x.v}", &props, &own, Some(&parent)).unwrap(),
            "2.3.1"
        );
        assert_eq!(
            interp("${outer}", &props, &own, Some(&parent)).unwrap(),
            "A2.3.1B>C",
            "a value embedding another defined property resolves through it"
        );
        assert_eq!(
            interp("v=${x.v}", &props, &own, Some(&parent)).unwrap(),
            "v=2.3.1"
        );
        // Structural spellings.
        assert_eq!(
            interp("${project.version}", &props, &own, Some(&parent)).unwrap(),
            "1.2.3"
        );
        assert_eq!(
            interp("${project.parent.groupId}", &props, &own, Some(&parent)).unwrap(),
            "gp"
        );
        assert_eq!(
            interp("${pom.artifactId}", &props, &own, Some(&parent)).unwrap(),
            "a"
        );
        // Legacy bare spellings.
        assert_eq!(
            interp("${groupId}", &props, &own, Some(&parent)).unwrap(),
            "g"
        );
        assert_eq!(
            interp("${parent.version}", &props, &own, Some(&parent)).unwrap(),
            "9"
        );
        // Unknown names and dot-prefixed names stay exactly as written.
        assert_eq!(
            interp("${nope.x}", &props, &own, Some(&parent)).unwrap(),
            "${nope.x}"
        );
        assert_eq!(
            interp("${ .weird }", &props, &own, Some(&parent)).unwrap(),
            "${ .weird }"
        );
        assert_eq!(interp("50%", &props, &own, Some(&parent)).unwrap(), "50%");
    }

    #[test]
    fn interpolation_depth_is_bounded() {
        let own = coords("g", "a", "1");
        let props = HashMap::new();
        let unresolved = interp("${x}", &props, &own, None).is_ok();
        assert!(unresolved, "an unknown placeholder is fine at depth 0");
        // A worst-case chain of self-referencing values.
        let mut deep = HashMap::new();
        deep.insert("x".to_string(), "${x}${x}${x}${x}".to_string());
        let error =
            interp("${x}", &deep, &own, None).expect_err("self-referencing properties stop");
        assert!(error.message.contains("circular"), "{error}");
    }

    #[test]
    fn a_property_shadowing_a_legacy_name_wins() {
        // Declared properties beat the legacy bare spellings; the structural
        // `project.*` sources are checked before properties, so
        // `project.version` still names the coordinate.
        let own = coords("g", "a", "1.2.3");
        let mut props = HashMap::new();
        props.insert("version".to_string(), "0.0.1".to_string());
        assert_eq!(interp("${version}", &props, &own, None).unwrap(), "0.0.1");
        assert_eq!(
            interp("${project.version}", &props, &own, None).unwrap(),
            "1.2.3"
        );
    }

    #[test]
    fn ordered_map_overrides_by_key_keeps_order() {
        let mut first = pom(Some("g"), "first", None);
        first.properties.push(("k".to_string(), "v".to_string()));
        let mut second = pom(Some("g"), "second", None);
        first.dependencies.push(crate::xml::Dependency {
            group_id: "g".to_string(),
            artifact_id: "dep".to_string(),
            version: Some("1".to_string()),
            package_type: None,
            classifier: None,
            scope: None,
            optional: false,
            exclusions: Vec::new(),
        });
        second.dependencies.push(crate::xml::Dependency {
            group_id: "g".to_string(),
            artifact_id: "dep".to_string(),
            version: Some("2".to_string()),
            package_type: None,
            classifier: None,
            scope: None,
            optional: false,
            exclusions: Vec::new(),
        });
        let merged = merge_chain(&[first, second]).expect("two entries");
        assert_eq!(merged.dependencies.len(), 1);
        assert_eq!(
            merged.dependencies[0].version.as_deref(),
            Some("2"),
            "the later (child) entry replaces the ancestor's whole entry"
        );
        // Management and dependencies are separate tables.
        assert!(merged.dependency_management.is_empty());
    }

    #[test]
    fn exclusions_union_in_order() {
        let own = vec![
            ("g".to_string(), "a".to_string()),
            ("g".to_string(), "b".to_string()),
        ];
        let managed = vec![
            ("g".to_string(), "b".to_string()),
            ("h".to_string(), "c".to_string()),
        ];
        assert_eq!(
            merge_exclusions(&own, &managed),
            vec![
                ("g".to_string(), "a".to_string()),
                ("g".to_string(), "b".to_string()),
                ("h".to_string(), "c".to_string()),
            ]
        );
    }
}
