//! The BFS graph walk and its decision record (ADR 0004).
//!
//! Semantics, all visible in [`Resolution`]:
//! * roots come from both manifest tables (`[dependencies]` +
//!   `[dev-dependencies]`, depth 1); platform entries are supply tables and
//!   fetch as POMs themselves;
//! * a dependency without a version takes it from the consolidated
//!   `dependencyManagement` of the POM that declared it (the effective-POM
//!   loader already does that, including BOM imports) — direct entries take
//!   it from a `platform` BOM of their table;
//! * the nearer declaration wins a version fork; the loser's subtree
//!   naturally aligns because only the winner was expanded (BFS order), and
//!   the fork is recorded, never swallowed (Maven's `mvn dependency:tree`
//!   only shows the winner);
//! * at equal depth the first declaration wins, also recorded;
//! * `[overrides]` replace the version at every occurrence they match,
//!   including the manifest's own pins, and each replacement is reported;
//! * optional / non-transitive scopes (`test`, `provided`, `system`) and
//!   `pom` packages are skipped by name.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use jx_common::error::{JxError, Kind};
use jx_manifest::deps::{DependencySpec, OverrideSpec, WantedVersion};
use jx_maven::xml;
use jx_maven::{Client, Coordinate, PomLoader};

use crate::selection::select_version;

/// One resolved artifact: GA identity + the winning concrete version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SolvedNode {
    pub group_id: String,
    pub artifact_id: String,
    pub version: String,
    pub classifier: Option<String>,
    /// Edges from the manifest root; direct dependencies are depth 1.
    pub depth: u32,
    /// Reachable only through `[dev-dependencies]`. Shared artifacts that a
    /// main dependency also needs are not dev.
    pub dev: bool,
}

/// A version fork the nearest-wins rule decided, kept visible (ADR 0004: a
/// conflict is never silent).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    pub group_id: String,
    pub artifact_id: String,
    /// The version kept — the nearer (or first-declared) path's.
    pub winner_version: String,
    pub winner_depth: u32,
    pub loser_version: String,
    pub loser_depth: u32,
}

/// One place where an `[overrides]` entry replaced the declared version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverrideApplication {
    pub group_id: String,
    pub artifact_id: String,
    pub declared_version: String,
    pub effective_version: String,
}

/// One edge of the resolved graph, kept so `jx tree` and `jx why` can show
/// how an artifact entered the build. Edges of losing conflicts are kept
/// too; the conflict report says which version actually won.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edge {
    /// The declaring node (`group:artifact[:classifier]`); `None` for a
    /// manifest root.
    pub parent: Option<String>,
    pub group_id: String,
    pub artifact_id: String,
    pub classifier: Option<String>,
    /// The version this edge asked for, after `[overrides]`.
    pub version: String,
    /// Edges from a manifest root are depth 1.
    pub depth: u32,
    /// `true` only on a root edge declared in `[dev-dependencies]`;
    /// transitive edges carry `false` and inherit scope from their root.
    pub dev: bool,
}

impl Edge {
    /// `group:artifact[:classifier]` of the child.
    pub fn key(&self) -> String {
        node_key(
            &self.group_id,
            &self.artifact_id,
            self.classifier.as_deref(),
        )
    }
}

/// The canonical `group:artifact[:classifier]` key used by nodes and edges.
pub fn node_key(group_id: &str, artifact_id: &str, classifier: Option<&str>) -> String {
    match classifier {
        Some(c) => format!("{group_id}:{artifact_id}:{c}"),
        None => format!("{group_id}:{artifact_id}"),
    }
}

/// A POM dependency that was deliberately not expanded, named for the
/// report (ADR 0004: no silent trims).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skipped {
    /// The `group:artifact` of the POM that declared the dependency.
    pub parent: String,
    pub group_id: String,
    pub artifact_id: String,
    pub reason: String,
}

/// The resolution outcome: canonical node order + every visible decision.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Resolution {
    /// Canonical order `(group, artifact, classifier, version)`.
    pub nodes: Vec<SolvedNode>,
    /// The traversed graph edges in BFS order (roots first).
    pub edges: Vec<Edge>,
    pub conflicts: Vec<Conflict>,
    pub replacements: Vec<OverrideApplication>,
    pub skipped: Vec<Skipped>,
    /// The platform (BOM) artifacts fetched to supply versions.
    pub platforms: Vec<String>,
}

impl SolvedNode {
    /// `group:artifact[:classifier]`, the identity keys of nodes and edges.
    pub fn key(&self) -> String {
        node_key(
            &self.group_id,
            &self.artifact_id,
            self.classifier.as_deref(),
        )
    }
}

impl Resolution {
    /// `group:artifact[:classifier]` of every node, canonical order.
    pub fn keys(&self) -> Vec<String> {
        self.nodes.iter().map(SolvedNode::key).collect()
    }
}

/// One BFS work item: what to expand, how far from the manifest, and the
/// exclusions in force above it on its path.
struct Entry {
    coord: Coordinate,
    depth: u32,
    exclusions: Arc<HashSet<(String, String)>>,
}

/// Resolve the whole graph from the manifest's dependency tables (both are
/// passed separately so the report can later distinguish them).
///
/// Panics never carry data the error would lose: platform "no version here"
/// is a manifest-parse-time guarantee, but it is still checked, not
/// unwrapped.
pub fn resolve(
    loader: &PomLoader<'_>,
    client: &Client,
    dependencies: &[DependencySpec],
    dev_dependencies: &[DependencySpec],
    overrides: &[OverrideSpec],
) -> Result<Resolution, JxError> {
    // ------------------------------------------------------------------
    // 1. Platform BOMs: fetch each as a POM and collect the versions its
    //    consolidated management supplies to the table.
    // ------------------------------------------------------------------
    let mut supplied: HashMap<(String, String), String> = HashMap::new();
    let mut platforms = Vec::new();
    for spec in dependencies
        .iter()
        .chain(dev_dependencies.iter())
        .filter(|spec| spec.platform)
    {
        let wanted = wanted_of(spec)?;
        let version = concrete(client, &spec.group_id, &spec.artifact_id, wanted)?;
        let coord = Coordinate {
            group_id: spec.group_id.clone(),
            artifact_id: spec.artifact_id.clone(),
            version,
            classifier: None,
            extension: "pom".to_string(),
        };
        let effective = loader.load(&coord)?;
        platforms.push(coord.key());
        for managed in &effective.dependency_management {
            // The loader keeps `scope = import` entries in the consolidated
            // list as markers of *which* BOM was imported — they name the
            // BOM itself, not a version the BOM supplies.
            if managed.scope.as_deref() == Some("import") {
                continue;
            }
            let Some(managed_version) = &managed.version else {
                continue;
            };
            // Maven's import order: already-managed coordinates stay
            // managed; an earlier platform entry wins like an earlier BOM.
            supplied
                .entry((managed.group_id.clone(), managed.artifact_id.clone()))
                .or_insert_with(|| managed_version.clone());
        }
    }

    // ------------------------------------------------------------------
    // 2. Roots: every non-platform entry of both tables enters at depth 1.
    // ------------------------------------------------------------------
    let mut resolution = Resolution {
        platforms,
        ..Resolution::default()
    };
    let mut queue: VecDeque<Entry> = VecDeque::new();
    for (spec, dev) in dependencies
        .iter()
        .map(|spec| (spec, false))
        .chain(dev_dependencies.iter().map(|spec| (spec, true)))
    {
        if spec.platform {
            continue;
        }
        // Exact pins pass through as written; prefixes read the listing.
        // Versionless entries take the version a platform BOM supplies —
        // those are concrete strings, never prefixes.
        let version = match &spec.wanted {
            Some(wanted @ WantedVersion::Prefix(_)) => {
                concrete(client, &spec.group_id, &spec.artifact_id, wanted)?
            }
            Some(WantedVersion::Exact(pin)) => pin.clone(),
            None => supplied
                .get(&(spec.group_id.clone(), spec.artifact_id.clone()))
                .cloned()
                .ok_or_else(|| versionless_entry_error(spec))?,
        };
        resolution.edges.push(Edge {
            parent: None,
            group_id: spec.group_id.clone(),
            artifact_id: spec.artifact_id.clone(),
            classifier: spec.classifier.clone(),
            version: version.clone(),
            depth: 1,
            dev,
        });
        queue.push_back(Entry {
            coord: Coordinate {
                group_id: spec.group_id.clone(),
                artifact_id: spec.artifact_id.clone(),
                version,
                classifier: spec.classifier.clone(),
                extension: "jar".to_string(),
            },
            depth: 1,
            exclusions: Arc::new(HashSet::new()),
        });
    }

    // ------------------------------------------------------------------
    // 3. The walk.
    // ------------------------------------------------------------------
    let mut solved: HashMap<(String, String), SolvedNode> = HashMap::new();
    while let Some(entry) = queue.pop_front() {
        let (group_id, artifact_id) = (&entry.coord.group_id, &entry.coord.artifact_id);
        let declared = entry.coord.version.clone();
        // Overrides replace first: the graph is solved under the replaced
        // version, so no fork can form against it (ADR 0004: they win
        // everywhere and every win is visible).
        let version = match overrides
            .iter()
            .find(|o| o.group_id == group_id.as_str() && o.artifact_id == artifact_id.as_str())
        {
            // Override values always pin (their own doc rule), whatever
            // marker they carried.
            Some(ov) => match &ov.wanted {
                WantedVersion::Prefix(v) | WantedVersion::Exact(v) => {
                    if v != &declared {
                        resolution.replacements.push(OverrideApplication {
                            group_id: group_id.clone(),
                            artifact_id: artifact_id.clone(),
                            declared_version: declared.clone(),
                            effective_version: v.clone(),
                        });
                    }
                    v.clone()
                }
            },
            None => declared.clone(),
        };

        let key = (group_id.clone(), artifact_id.clone());
        if let Some(existing) = solved.get(&key) {
            // BFS pops shallow-first, so a stored node here is always as
            // near as this occurrence or nearer: keep it, record the fork.
            if existing.version != version {
                resolution.conflicts.push(Conflict {
                    group_id: group_id.clone(),
                    artifact_id: artifact_id.clone(),
                    winner_version: existing.version.clone(),
                    winner_depth: existing.depth,
                    loser_version: version,
                    loser_depth: entry.depth,
                });
            }
            continue;
        }
        // Two occurrences that differ only by classifier converge on one
        // node: Maven resolves versions per GA (its management table's
        // lookup ignores classifier) and the classifier rides along from
        // whichever path was solved first.
        solved.insert(
            key,
            SolvedNode {
                group_id: group_id.clone(),
                artifact_id: artifact_id.clone(),
                version: version.clone(),
                classifier: entry.coord.classifier.clone(),
                depth: entry.depth,
                // Filled in after the walk from the root scopes.
                dev: false,
            },
        );

        // The POM must be the one for the version actually solved under
        // `[overrides]`, not the version some ancestor declared.
        let mut effective_coord = entry.coord.clone();
        effective_coord.version = version.clone();
        let effective = loader.load(&effective_coord)?;
        for dep in &effective.dependencies {
            let mut skip = |reason: String| {
                resolution.skipped.push(Skipped {
                    parent: effective_coord.key(),
                    group_id: dep.group_id.clone(),
                    artifact_id: dep.artifact_id.clone(),
                    reason,
                })
            };
            if dep.optional {
                skip("optional".to_string());
                continue;
            }
            match dep.scope.as_deref().unwrap_or("compile") {
                "compile" | "runtime" => {}
                // Maven only uses provided for the declaring module's own
                // compilation; consumers never inherit it. Including it here
                // pulled build-only tools (annotation processors and the
                // like) into every downstream graph.
                "provided" => {
                    skip("scope `provided` is not transitive".to_string());
                    continue;
                }
                "test" => {
                    skip("scope `test` is not transitive".to_string());
                    continue;
                }
                "system" => {
                    skip("scope `system` carries its own path in V1 jx".to_string());
                    continue;
                }
                "import" => {
                    // An import-scope entry inside `dependencies` (Maven 2
                    // style) is a management marker, not a build input.
                    skip("scope `import` is management, not a dependency".to_string());
                    continue;
                }
                other => {
                    skip(format!("unknown scope `{other}`"));
                    continue;
                }
            }
            // A dependency that is itself a BOM ships no jar; its management
            // is already merged into this POM's consolidated table. Resolving
            // it would put a .pom "artifact" into the lockfile — wrong per
            // ADR 0004 (artifacts only).
            if dep.package_type.as_deref() == Some("pom") {
                skip("pom packaging: management merged, no jar to ship".to_string());
                continue;
            }
            if entry
                .exclusions
                .contains(&(dep.group_id.clone(), dep.artifact_id.clone()))
            {
                skip("excluded by a nearer declaration".to_string());
                continue;
            }
            let Some(dep_version) = &dep.version else {
                return Err(JxError::new(
                    Kind::Resolve,
                    format!(
                        "{}:{} declares no version and no management supplies one",
                        dep.group_id, dep.artifact_id
                    ),
                )
                .at(entry.coord.key())
                .hint("jx fills versionless entries from the declaring POM's dependencyManagement (BOMs included); the declaring POM must import a BOM that manages this artifact"));
            };
            if dep_version.to_ascii_uppercase().ends_with("-SNAPSHOT") {
                return Err(JxError::new(
                    Kind::Resolve,
                    format!(
                        "{}/{} requires the SNAPSHOT {}:{}",
                        entry.coord.group_id, entry.coord.artifact_id, dep.group_id, dep.artifact_id
                    ),
                )
                .at(entry.coord.key())
                .hint("jx V1 resolves releases only — a lockfile cannot pin a re-resolving build; ask the upstream project for a release"));
            }
            let mut child_exclusions = (*entry.exclusions).clone();
            for (excluded_group, excluded_artifact) in &dep.exclusions {
                child_exclusions.insert((excluded_group.clone(), excluded_artifact.clone()));
            }
            resolution.edges.push(Edge {
                // Nodes and edges are joined by identity, not version: the
                // winning version is looked up in `Resolution::nodes`.
                parent: Some(node_key(
                    &effective_coord.group_id,
                    &effective_coord.artifact_id,
                    effective_coord.classifier.as_deref(),
                )),
                group_id: dep.group_id.clone(),
                artifact_id: dep.artifact_id.clone(),
                classifier: dep.classifier.clone(),
                version: dep_version.clone(),
                depth: entry.depth + 1,
                dev: false,
            });
            queue.push_back(Entry {
                coord: Coordinate {
                    group_id: dep.group_id.clone(),
                    artifact_id: dep.artifact_id.clone(),
                    version: dep_version.clone(),
                    classifier: dep.classifier.clone(),
                    extension: dep
                        .package_type
                        .clone()
                        .unwrap_or_else(|| "jar".to_string()),
                },
                depth: entry.depth + 1,
                exclusions: Arc::new(child_exclusions),
            });
        }
    }

    resolution.nodes.extend(solved.into_values());
    mark_dev_nodes(&mut resolution);
    resolution.nodes.sort_by(|a, b| {
        (&a.group_id, &a.artifact_id, &a.classifier, &a.version).cmp(&(
            &b.group_id,
            &b.artifact_id,
            &b.classifier,
            &b.version,
        ))
    });
    Ok(resolution)
}

/// Mark every node reachable only from `[dev-dependencies]` roots. The
/// distinction decides which jars javac sees for main sources; it is derived
/// from the edges after solving, so an artifact shared by main and dev paths
/// correctly counts as main.
fn mark_dev_nodes(resolution: &mut Resolution) {
    let mut children: HashMap<String, Vec<String>> = HashMap::new();
    let mut frontier = VecDeque::new();
    let mut main = HashSet::new();
    for edge in &resolution.edges {
        match &edge.parent {
            Some(parent) => children.entry(parent.clone()).or_default().push(edge.key()),
            None if !edge.dev => {
                let child = edge.key();
                if main.insert(child.clone()) {
                    frontier.push_back(child);
                }
            }
            None => {}
        }
    }
    while let Some(parent) = frontier.pop_front() {
        if let Some(next) = children.get(&parent) {
            for child in next {
                if main.insert(child.clone()) {
                    frontier.push_back(child.clone());
                }
            }
        }
    }
    for node in &mut resolution.nodes {
        node.dev = !main.contains(&node.key());
    }
}

/// Expand one wanted pin to a concrete version: exact pins pass through
/// (404s at fetch time if the string is wrong), prefixes read the
/// repositories' published version list (ADR 0004).
fn concrete(
    client: &Client,
    group_id: &str,
    artifact_id: &str,
    wanted: &WantedVersion,
) -> Result<String, JxError> {
    let offered = match wanted {
        // An exact pin needs no listing — the string decides.
        WantedVersion::Exact(pin) => vec![pin.clone()],
        WantedVersion::Prefix(_) => offered_versions(client, group_id, artifact_id)?,
    };
    match select_version(wanted, &offered) {
        Some(version) => Ok(version),
        None => Err(no_matching_version(group_id, artifact_id, wanted, &offered)),
    }
}

/// Fetch and parse the artifact-level version listing as published versions.
fn offered_versions(
    client: &Client,
    group_id: &str,
    artifact_id: &str,
) -> Result<Vec<String>, JxError> {
    // The listing directory never depends on a version; the version field
    // is honoured only by the artifact paths this coordinate is otherwise
    // used for.
    let coord = Coordinate {
        group_id: group_id.to_string(),
        artifact_id: artifact_id.to_string(),
        version: String::new(),
        classifier: None,
        extension: "jar".to_string(),
    };
    let path = format!("{}/maven-metadata.xml", coord.metadata_directory(false));
    let outcome = client
        .fetch_version_listing(&coord)?
        .ok_or_else(|| {
            JxError::new(
                Kind::Resolve,
                format!("no repository publishes a version list for {group_id}:{artifact_id}"),
            )
            .hint("prefix pins (`33`, `2.0`) need the repositories' maven-metadata.xml; pin exactly (`=33.4.8-jre`) to skip it")
        })?;
    let text = String::from_utf8_lossy(&outcome.bytes).into_owned();
    let metadata = xml::parse_metadata(&text, &path)?;
    Ok(metadata.versions)
}

fn no_matching_version(
    group_id: &str,
    artifact_id: &str,
    wanted: &WantedVersion,
    offered: &[String],
) -> JxError {
    match wanted {
        WantedVersion::Exact(pin) => {
            JxError::new(Kind::Resolve, format!("the exact pin `{pin}` names no listed version of {group_id}:{artifact_id}"))
                .at(format!("{group_id}:{artifact_id}"))
                .hint("exact pins match the string as written; drop the `=` to let jx pick the highest release")
        }
        WantedVersion::Prefix(prefix) => {
            let mut sorted: Vec<&String> = offered.iter().collect();
            sorted.sort();
            sorted.dedup();
            let shown = sorted
                .iter()
                .take(6)
                .map(|offered| offered.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            let more = if sorted.len() > 6 {
                format!(" … and {} more", sorted.len() - 6)
            } else {
                String::new()
            };
            JxError::new(
                Kind::Resolve,
                format!(
                    "no stable version of {group_id}:{artifact_id} extends the wanted prefix {prefix}"
                ),
            )
            .at(format!("{group_id}:{artifact_id}"))
            .hint(format!(
                "listed: {shown}{more}; pre-releases are skipped (pin exactly with `=` to take one)"
            ))
        }
    }
}

fn versionless_entry_error(spec: &DependencySpec) -> JxError {
    // Only constructed when the platform lookup found nothing, so the
    // message names the remedy directly.
    JxError::new(
        Kind::Resolve,
        format!(
            "the dependency `{}` ({}:{}) declares no version and no platform entry in its table supplies one",
            spec.alias, spec.group_id, spec.artifact_id
        ),
    )
    .at(spec.alias.clone())
    .hint("give it a version, or add a `{ platform = … }` BOM entry to the same table that manages it")
}

/// The wanted pin of an entry; platform entries always carry one (parse-time
/// validation), but the resolver re-checks instead of trusting it.
fn wanted_of(spec: &DependencySpec) -> Result<&WantedVersion, JxError> {
    spec.wanted.as_ref().ok_or_else(|| {
        JxError::new(
            Kind::Manifest,
            format!("the platform `{}` has no version", spec.alias),
        )
        .hint("a BOM needs a version to import: `…:group:artifact:version`")
    })
}
