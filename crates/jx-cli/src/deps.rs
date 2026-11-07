//! M3 dependency commands: resolve, lock, fetch, tree, why, add and remove.
//!
//! Resolution is always driven from the manifest; the lockfile records the
//! concrete artifacts and their sha256s so `jx fetch` and later builds can
//! work without re-resolving. Edits (`add`/`remove`) are transactional: if
//! the follow-up resolve or fetch fails, the manifest is restored, so a
//! failed command never leaves a half-changed project.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};

use jx_common::config;
use jx_common::error::{JxError, Kind};
use jx_common::paths;
use jx_lock::{Lockfile, PinnedArtifact};
use jx_manifest::{self as manifest, DepTable, DependencySpec, WantedVersion};
use jx_maven::{Client, Coordinate, PomLoader};
use jx_resolve::{Edge, Resolution, SolvedNode};

use crate::Cli;

/// One locked main artifact: the cache path plus the coordinate pieces a
/// distributable layout needs for its file name.
#[derive(Debug, Clone)]
pub(crate) struct LockedJar {
    pub group_id: String,
    pub artifact_id: String,
    pub version: String,
    pub classifier: Option<String>,
    pub path: PathBuf,
}

/// The locked jars a build needs. `main` excludes artifacts reachable only
/// through `[dev-dependencies]`; `test` contains every app artifact;
/// `processors` is the javac `-processorpath` (ADR 0014) and is disjoint from
/// both classpaths except when one jar serves both roles.
pub(crate) struct Classpaths {
    pub main: Vec<PathBuf>,
    pub main_jars: Vec<LockedJar>,
    pub test: Vec<PathBuf>,
    /// Annotation-processor jars, including their transitive dependencies.
    pub processors: Vec<PathBuf>,
    /// Every app artifact, including dev-only ones. Used by exporters that
    /// must map manifest declarations to concrete versions.
    pub all_jars: Vec<LockedJar>,
    /// The JUnit Platform console standalone jar, when the project depends on it.
    pub junit_launcher: Option<PathBuf>,
    /// Version of that launcher, used to choose the compatible invocation.
    pub junit_launcher_version: Option<String>,
}

struct Project {
    manifest_path: PathBuf,
    manifest_dir: PathBuf,
    manifest: manifest::Manifest,
    home: PathBuf,
    client: Client,
}

impl Project {
    fn open(cli: &Cli, command: &str) -> Result<Project, JxError> {
        let manifest_path = crate::locate_manifest(cli)?;
        let manifest = manifest::parse(&manifest_path)?;
        crate::engine::require_jx(
            cli,
            &manifest,
            command,
            "the delegated build owns dependency resolution, so project.toml is not its graph",
        )?;
        let manifest_dir = manifest_path
            .parent()
            .expect("manifest path always has a parent")
            .to_path_buf();
        let home = paths::jx_home()?;
        let settings = config::load(&home)?;
        let client = Client::new(&home, settings, cli.offline)?;
        Ok(Project {
            manifest_path,
            manifest_dir,
            manifest,
            home,
            client,
        })
    }

    fn lock_path(&self) -> PathBuf {
        self.manifest_dir.join("jx.lock")
    }

    fn reload(&mut self) -> Result<(), JxError> {
        self.manifest = manifest::parse(&self.manifest_path)?;
        Ok(())
    }

    fn resolve(&self) -> Result<Resolution, JxError> {
        self.resolve_tables(&self.manifest.dependencies, &self.manifest.dev_dependencies)
    }

    fn resolve_tables(
        &self,
        dependencies: &[DependencySpec],
        dev_dependencies: &[DependencySpec],
    ) -> Result<Resolution, JxError> {
        let loader = PomLoader::new(&self.client);
        jx_resolve::resolve(
            &loader,
            &self.client,
            dependencies,
            dev_dependencies,
            &self.manifest.overrides,
        )
    }

    /// The annotation-processor graph, resolved independently of the app
    /// graph (ADR 0014). `[overrides]` apply to both.
    fn resolve_processors(&self) -> Result<Resolution, JxError> {
        self.resolve_processor_table(&self.manifest.processors)
    }

    fn resolve_processor_table(
        &self,
        processors: &[DependencySpec],
    ) -> Result<Resolution, JxError> {
        let loader = PomLoader::new(&self.client);
        jx_resolve::resolve_processors(&loader, &self.client, processors, &self.manifest.overrides)
    }
}

/// One artifact whose concrete version appears, disappears or moves.
#[derive(Debug, Clone, PartialEq, Eq)]
struct LockChange {
    key: String,
    from: Option<String>,
    to: Option<String>,
}

/// What a lock write did, for the command's summary line.
struct LockReport {
    artifacts: usize,
    changed: bool,
    changes: Vec<LockChange>,
}

/// `jx lock`: resolve and write the lockfile, fetching artifact bytes to
/// record their sha256. `--locked` makes any difference a hard failure.
pub fn lock(cli: &Cli) -> Result<(), JxError> {
    let project = Project::open(cli, "jx lock")?;
    let report = write_lock(cli, &project)?;
    print_lock_summary(&project, &report);
    Ok(())
}

/// `jx update`: re-resolve wanted prefixes and write jx.lock, reporting the
/// version transitions. With a direct dependency, every other direct root is
/// pinned to its locked version so one update does not refresh the rest of
/// the manifest.
pub fn update(cli: &Cli, dependency: Option<&str>) -> Result<(), JxError> {
    if cli.locked {
        return Err(
            JxError::new(Kind::Lock, "--locked: jx update always writes jx.lock")
                .hint("drop --locked; `jx lock --locked` verifies without changing it"),
        );
    }
    let project = Project::open(cli, "jx update")?;
    let existing = read_optional_lock(&project)?;
    let roots = update_roots(&project, dependency, existing.as_ref())?;
    let resolution = project.resolve_tables(&roots.dependencies, &roots.dev_dependencies)?;
    let processors = project.resolve_processor_table(&roots.processors)?;
    let report = write_resolution(cli, &project, &resolution, &processors)?;
    print_update_summary(&project, &report);
    Ok(())
}

/// The manifest roots a selective update resolves. Targets are identified by
/// table and index so an alias can never accidentally match another table.
struct UpdateRoots {
    dependencies: Vec<DependencySpec>,
    dev_dependencies: Vec<DependencySpec>,
    processors: Vec<DependencySpec>,
}

#[derive(Debug, Clone, Copy)]
struct UpdateTarget {
    table: DepTable,
    index: usize,
}

fn update_roots(
    project: &Project,
    dependency: Option<&str>,
    existing: Option<&Lockfile>,
) -> Result<UpdateRoots, JxError> {
    let Some(target_name) = dependency else {
        return Ok(UpdateRoots {
            dependencies: project.manifest.dependencies.clone(),
            dev_dependencies: project.manifest.dev_dependencies.clone(),
            processors: project.manifest.processors.clone(),
        });
    };
    let lock = existing.ok_or_else(|| {
        JxError::new(
            Kind::Lock,
            "jx update with a direct dependency needs an existing jx.lock",
        )
        .at("jx.lock")
        .hint("run `jx lock` first, or run `jx update` without a dependency")
    })?;
    let target = locate_update_target(project, target_name)?;
    let target_spec = specs_for(project, target.table)
        .get(target.index)
        .expect("target index came from this table");
    validate_update_target(target_spec, target.table)?;

    Ok(UpdateRoots {
        dependencies: pin_update_table(
            &project.manifest.dependencies,
            DepTable::Dependencies,
            (target.table == DepTable::Dependencies).then_some(target.index),
            lock,
        )?,
        dev_dependencies: pin_update_table(
            &project.manifest.dev_dependencies,
            DepTable::DevDependencies,
            (target.table == DepTable::DevDependencies).then_some(target.index),
            lock,
        )?,
        processors: pin_update_table(
            &project.manifest.processors,
            DepTable::Processors,
            (target.table == DepTable::Processors).then_some(target.index),
            lock,
        )?,
    })
}

fn specs_for(project: &Project, table: DepTable) -> &[DependencySpec] {
    match table {
        DepTable::Dependencies => &project.manifest.dependencies,
        DepTable::DevDependencies => &project.manifest.dev_dependencies,
        DepTable::Processors => &project.manifest.processors,
    }
}

/// Find one direct root by alias or `group:artifact`. Classified coordinates
/// are intentionally not guessed: an alias is unambiguous.
fn locate_update_target(project: &Project, target: &str) -> Result<UpdateTarget, JxError> {
    let coordinate = if target.contains(':') {
        let parts: Vec<&str> = target.split(':').collect();
        if parts.len() != 2 || parts.iter().any(|part| part.is_empty()) {
            return Err(JxError::new(
                Kind::Usage,
                format!("`{target}` is not a direct-dependency target"),
            )
            .hint("pass an alias, or `group:artifact` without a version or classifier"));
        }
        Some((parts[0], parts[1]))
    } else {
        if target.trim().is_empty() {
            return Err(
                JxError::new(Kind::Usage, "the update target cannot be empty")
                    .hint("pass an alias or `group:artifact`, or omit it to update every prefix"),
            );
        }
        None
    };

    let mut matches = Vec::new();
    for table in [
        DepTable::Dependencies,
        DepTable::DevDependencies,
        DepTable::Processors,
    ] {
        for (index, spec) in specs_for(project, table).iter().enumerate() {
            let hit = match coordinate {
                Some((group_id, artifact_id)) => {
                    spec.group_id == group_id
                        && spec.artifact_id == artifact_id
                        && spec.classifier.is_none()
                }
                None => spec.alias == target,
            };
            if hit {
                matches.push(UpdateTarget { table, index });
            }
        }
    }
    match matches.as_slice() {
        [] => Err(JxError::new(
            Kind::Usage,
            format!("`{target}` is not a direct dependency in project.toml"),
        )
        .hint("`jx tree` lists the declared graph; pass an alias or `group:artifact`")),
        [target] => Ok(*target),
        _ => Err(JxError::new(
            Kind::Usage,
            format!("`{target}` matches more than one direct dependency"),
        )
        .hint("use an alias unique across [dependencies], [dev-dependencies] and [processors]")),
    }
}

fn validate_update_target(spec: &DependencySpec, table: DepTable) -> Result<(), JxError> {
    if spec.platform {
        return Err(JxError::new(
            Kind::Manifest,
            format!("`{}` is a platform BOM, not an updateable direct artifact", spec.alias),
        )
        .at(format!("[{}]", table.name()))
        .hint("run `jx update` without a dependency to refresh every prefix, or change the BOM version by hand"));
    }
    match &spec.wanted {
        Some(WantedVersion::Prefix(_)) => Ok(()),
        Some(WantedVersion::Exact(version)) => Err(JxError::new(
            Kind::Manifest,
            format!("`{}` is pinned exactly (`={version}`)", spec.alias),
        )
        .at(format!("[{}]", table.name()))
        .hint("change it to a prefix, or edit the exact version by hand")),
        None => Err(JxError::new(
            Kind::Manifest,
            format!(
                "`{}` has no manifest version; a platform BOM supplies it",
                spec.alias
            ),
        )
        .at(format!("[{}]", table.name()))
        .hint("update the BOM itself, or run `jx update` without a dependency")),
    }
}

/// Keep exact pins and exact platform entries as written; pin every other
/// direct root to its current locked version. That is what makes a selected
/// update selective at the manifest level.
fn pin_update_table(
    specs: &[DependencySpec],
    table: DepTable,
    target_index: Option<usize>,
    lock: &Lockfile,
) -> Result<Vec<DependencySpec>, JxError> {
    let mut roots = Vec::with_capacity(specs.len());
    for (index, spec) in specs.iter().enumerate() {
        if target_index == Some(index) {
            roots.push(spec.clone());
            continue;
        }
        if spec.platform {
            if matches!(spec.wanted.as_ref(), Some(WantedVersion::Prefix(_))) {
                return Err(JxError::new(
                    Kind::Manifest,
                    format!(
                        "cannot preserve the platform prefix for `{}` during a selective update",
                        spec.alias
                    ),
                )
                .at(format!("[{}]", table.name()))
                .hint("pin that platform entry exactly and update it by hand, or run `jx update` without a dependency"));
            }
            roots.push(spec.clone());
            continue;
        }
        if spec.wanted.as_ref().is_some_and(WantedVersion::is_exact) {
            roots.push(spec.clone());
            continue;
        }
        let pinned = lock
            .find(&spec.group_id, &spec.artifact_id, spec.classifier.as_deref())
            .ok_or_else(|| {
                JxError::new(
                    Kind::Lock,
                    format!(
                        "no locked version for `{}` ({}:{}); selective update needs a complete jx.lock",
                        spec.alias, spec.group_id, spec.artifact_id
                    ),
                )
                .hint("run `jx lock` first, or run `jx update` without a dependency")
            })?;
        let mut pinned_spec = spec.clone();
        pinned_spec.wanted = Some(WantedVersion::Exact(pinned.version.clone()));
        roots.push(pinned_spec);
    }
    Ok(roots)
}

fn read_optional_lock(project: &Project) -> Result<Option<Lockfile>, JxError> {
    let path = project.lock_path();
    if !path.exists() {
        return Ok(None);
    }
    jx_lock::read(&path).map(Some)
}

fn lock_changes(old: Option<&Lockfile>, new: &Lockfile) -> Vec<LockChange> {
    let mut changes = Vec::new();
    if let Some(old) = old {
        for artifact in &old.artifacts {
            let key = artifact.key();
            match new.find(
                &artifact.group_id,
                &artifact.artifact_id,
                artifact.classifier.as_deref(),
            ) {
                Some(replacement) if replacement.version != artifact.version => {
                    changes.push(LockChange {
                        key,
                        from: Some(artifact.version.clone()),
                        to: Some(replacement.version.clone()),
                    });
                }
                Some(_) => {}
                None => changes.push(LockChange {
                    key,
                    from: Some(artifact.version.clone()),
                    to: None,
                }),
            }
        }
    }
    for artifact in &new.artifacts {
        let absent = old.is_none_or(|old| {
            old.find(
                &artifact.group_id,
                &artifact.artifact_id,
                artifact.classifier.as_deref(),
            )
            .is_none()
        });
        if absent {
            changes.push(LockChange {
                key: artifact.key(),
                from: None,
                to: Some(artifact.version.clone()),
            });
        }
    }
    changes.sort_by(|a, b| a.key.cmp(&b.key));
    changes
}

fn print_update_summary(project: &Project, report: &LockReport) {
    let artifacts = format!(
        "{} artifact{}",
        report.artifacts,
        if report.artifacts == 1 { "" } else { "s" }
    );
    if report.changes.is_empty() {
        if report.changed {
            println!("jx update: jx.lock metadata refreshed ({artifacts})");
        } else {
            println!("jx update: no version changes ({artifacts})");
        }
        return;
    }
    println!(
        "jx update: {} version change{} ({artifacts})",
        report.changes.len(),
        if report.changes.len() == 1 { "" } else { "s" }
    );
    for change in &report.changes {
        match (&change.from, &change.to) {
            (Some(from), Some(to)) => println!("  {} {from} -> {to}", change.key),
            (Some(from), None) => println!("  {} removed ({from})", change.key),
            (None, Some(to)) => println!("  {} added ({to})", change.key),
            (None, None) => {}
        }
    }
    if report.changed {
        println!(
            "jx.lock: {artifacts} written to {}",
            project.lock_path().display()
        );
    } else {
        println!("jx.lock: up to date ({artifacts})");
    }
}

/// `jx fetch`: download exactly what the lockfile pins, verifying every
/// checksum against it. Cached entries are re-verified, not trusted.
pub fn fetch(cli: &Cli) -> Result<(), JxError> {
    let project = Project::open(cli, "jx fetch")?;
    let lock = read_lock_for(&project)?;
    if let Some(pin) = &lock.jdk {
        ensure_locked_jdk(&project.home, pin, cli.offline)?;
    }
    let (cached, downloaded, bytes) = fetch_pinned(&project.client, &lock)?;
    println!(
        "jx fetch: {} artifact{} ready ({cached} cached, {downloaded} downloaded, {bytes} bytes)",
        lock.artifacts.len(),
        if lock.artifacts.len() == 1 { "" } else { "s" },
    );
    Ok(())
}

/// `jx tree`: the resolved graph, conflicts and (verbose) skip reasons.
pub fn tree(cli: &Cli) -> Result<(), JxError> {
    let project = Project::open(cli, "jx tree")?;
    let resolution = project.resolve()?;
    print_tree(&resolution, cli.verbose);
    if !project.manifest.processors.is_empty() {
        let processors = project.resolve_processors()?;
        println!();
        println!("annotation processors:");
        print_tree(&processors, cli.verbose);
    }
    Ok(())
}

/// `jx why <group:artifact[:version]>`: one path from a manifest root, in
/// the app graph and, when declared, the annotation-processor graph.
pub fn why(cli: &Cli, coord: &str) -> Result<(), JxError> {
    let project = Project::open(cli, "jx why")?;
    let resolution = project.resolve()?;
    let processor_resolution = if project.manifest.processors.is_empty() {
        None
    } else {
        Some(project.resolve_processors()?)
    };
    let (group_id, artifact_id, wanted_version) = parse_why_target(coord)?;
    let wanted_version = wanted_version.as_deref();
    let in_app = why_has(&resolution, &group_id, &artifact_id, wanted_version);
    let in_processors = processor_resolution
        .as_ref()
        .is_some_and(|resolution| why_has(resolution, &group_id, &artifact_id, wanted_version));
    if !in_app && !in_processors {
        return Err(why_missing(
            &resolution,
            processor_resolution.as_ref(),
            &group_id,
            &artifact_id,
            wanted_version,
        ));
    }
    if in_app && in_processors {
        println!("app graph:");
    }
    if in_app {
        why_in(
            &resolution,
            &group_id,
            &artifact_id,
            wanted_version,
            "the manifest",
        )?;
    }
    if in_processors {
        if in_app {
            println!();
        }
        if let Some(processors) = &processor_resolution {
            println!("annotation processors:");
            why_in(
                processors,
                &group_id,
                &artifact_id,
                wanted_version,
                "[processors]",
            )?;
        }
    }
    Ok(())
}

fn why_node<'a>(
    resolution: &'a Resolution,
    group_id: &str,
    artifact_id: &str,
    wanted_version: Option<&str>,
) -> Option<&'a SolvedNode> {
    resolution.nodes.iter().find(|node| {
        node.group_id == group_id
            && node.artifact_id == artifact_id
            && wanted_version.is_none_or(|wanted| node.version == wanted)
    })
}

fn why_has(
    resolution: &Resolution,
    group_id: &str,
    artifact_id: &str,
    wanted_version: Option<&str>,
) -> bool {
    why_node(resolution, group_id, artifact_id, wanted_version).is_some()
}

fn why_missing(
    app: &Resolution,
    processors: Option<&Resolution>,
    group_id: &str,
    artifact_id: &str,
    wanted_version: Option<&str>,
) -> JxError {
    let mut available: Vec<String> = Vec::new();
    for resolution in std::iter::once(app).chain(processors) {
        for node in &resolution.nodes {
            if node.group_id == group_id
                && node.artifact_id == artifact_id
                && !available.iter().any(|version| version == &node.version)
            {
                available.push(node.version.clone());
            }
        }
    }
    let scope = if processors.is_some() {
        "the app or annotation-processor graph"
    } else {
        "the resolved graph"
    };
    let message = if available.is_empty() {
        format!("{group_id}:{artifact_id} is not in {scope}")
    } else {
        format!(
            "{group_id}:{artifact_id} resolves to {}, not {}",
            available.join(", "),
            wanted_version.unwrap_or("")
        )
    };
    let hint = if processors.is_some() {
        "`jx tree` shows the app graph and the annotation-processor section"
    } else {
        "`jx tree` shows every resolved artifact and where it entered"
    };
    JxError::new(Kind::Resolve, message)
        .at(format!("{group_id}:{artifact_id}"))
        .hint(hint)
}

/// `jx add`: append a validated dependency, then resolve and lock. Any
/// failure restores the manifest byte for byte.
pub fn add(
    cli: &Cli,
    dependency: &str,
    dev: bool,
    platform: bool,
    processor: bool,
    alias: Option<String>,
) -> Result<(), JxError> {
    if cli.locked {
        return Err(JxError::new(
            Kind::Lock,
            "--locked: adding a dependency would change jx.lock",
        )
        .hint("drop --locked for this edit, or update project.toml by hand"));
    }
    if processor && (dev || platform) {
        return Err(JxError::new(
            Kind::Usage,
            "--processor cannot be combined with --dev or --platform",
        )
        .hint("a processor is a build-time jar; it goes to [processors] on its own"));
    }
    let mut project = Project::open(cli, "jx add")?;
    let table = table_of(dev, processor);

    // Parse under a temporary empty alias first: the artifact id is the
    // default alias, and the coordinate rules stay in one place.
    let mut spec = manifest::deps::parse_user_spec("", dependency.trim(), platform)?;
    spec.alias = alias.unwrap_or_else(|| spec.artifact_id.clone());
    let alias = spec.alias.clone();

    if let Some(existing) = existing_coordinate(&project.manifest, &spec) {
        return Err(JxError::new(
            Kind::Manifest,
            format!(
                "{}:{} is already declared as `{existing}` in project.toml",
                spec.group_id, spec.artifact_id
            ),
        )
        .hint("remove that entry first, or use `--alias` only for a different coordinate"));
    }

    // A versionless add picks the highest stable release from the listings;
    // a caller who wants an exact string writes one.
    if spec.wanted.is_none() {
        let version = latest_version(&project.client, &spec)?;
        println!(
            "  latest release of {}:{} is {version}",
            spec.group_id, spec.artifact_id
        );
        spec.wanted = Some(WantedVersion::Prefix(version));
    }

    let original = read_manifest(&project.manifest_path)?;
    manifest::add_dependency(&project.manifest_path, table, &spec)?;
    project.reload()?;
    match write_lock(cli, &project) {
        Ok(report) => {
            println!(
                "added `{}` = {} to [{}]",
                alias,
                describe(&spec),
                table.name()
            );
            print_lock_summary(&project, &report);
            Ok(())
        }
        Err(error) => {
            std::fs::write(&project.manifest_path, original).ok();
            project.reload().ok();
            Err(error.hint("the dependency was rolled back; project.toml is unchanged"))
        }
    }
}

/// `jx remove`: delete one entry, then resolve and lock. Like `add`, a
/// failure restores the manifest.
pub fn remove(cli: &Cli, alias: &str, dev: bool, processor: bool) -> Result<(), JxError> {
    if cli.locked {
        return Err(JxError::new(
            Kind::Lock,
            "--locked: removing a dependency would change jx.lock",
        )
        .hint("drop --locked for this edit, or update project.toml by hand"));
    }
    if processor && dev {
        return Err(
            JxError::new(Kind::Usage, "--processor cannot be combined with --dev")
                .hint("[processors] and [dev-dependencies] are separate tables"),
        );
    }
    let mut project = Project::open(cli, "jx remove")?;
    let table = table_of(dev, processor);
    if !manifest::has_alias(&project.manifest_path, table, alias)? {
        for other in [
            DepTable::Dependencies,
            DepTable::DevDependencies,
            DepTable::Processors,
        ] {
            if other != table && manifest::has_alias(&project.manifest_path, other, alias)? {
                return Err(JxError::new(
                    Kind::Manifest,
                    format!(
                        "`{alias}` is declared in [{}], not [{}]",
                        other.name(),
                        table.name()
                    ),
                )
                .hint(format!(
                    "run `jx remove {alias}{}` to target that table",
                    table_flag(other)
                )));
            }
        }
    }

    let original = read_manifest(&project.manifest_path)?;
    manifest::remove_dependency(&project.manifest_path, table, alias)?;
    project.reload()?;
    match write_lock(cli, &project) {
        Ok(report) => {
            println!("removed `{alias}` from [{}]", table.name());
            print_lock_summary(&project, &report);
            Ok(())
        }
        Err(error) => {
            std::fs::write(&project.manifest_path, original).ok();
            project.reload().ok();
            Err(error.hint("the removal was rolled back; project.toml is unchanged"))
        }
    }
}

fn table_of(dev: bool, processor: bool) -> DepTable {
    match (dev, processor) {
        (_, true) => DepTable::Processors,
        (true, false) => DepTable::DevDependencies,
        (false, false) => DepTable::Dependencies,
    }
}

fn table_flag(table: DepTable) -> &'static str {
    match table {
        DepTable::Dependencies => "",
        DepTable::DevDependencies => " --dev",
        DepTable::Processors => " --processor",
    }
}

fn read_manifest(path: &Path) -> Result<String, JxError> {
    std::fs::read_to_string(path).map_err(|e| {
        JxError::new(Kind::Manifest, "cannot read project.toml")
            .at(path.display().to_string())
            .caused_by(e)
    })
}

fn existing_coordinate<'a>(
    manifest: &'a manifest::Manifest,
    spec: &DependencySpec,
) -> Option<&'a str> {
    manifest
        .dependencies
        .iter()
        .chain(manifest.dev_dependencies.iter())
        .chain(manifest.processors.iter())
        .find(|existing| {
            existing.group_id == spec.group_id
                && existing.artifact_id == spec.artifact_id
                && existing.classifier == spec.classifier
        })
        .map(|existing| existing.alias.as_str())
}

fn describe(spec: &DependencySpec) -> String {
    let mut text = format!("{}:{}", spec.group_id, spec.artifact_id);
    if let Some(wanted) = &spec.wanted {
        text.push(':');
        if wanted.is_exact() {
            text.push('=');
        }
        text.push_str(wanted.as_str());
    }
    if let Some(classifier) = &spec.classifier {
        text.push(':');
        text.push_str(classifier);
    }
    format!("\"{text}\"")
}

fn latest_version(client: &Client, spec: &DependencySpec) -> Result<String, JxError> {
    let coord = Coordinate {
        group_id: spec.group_id.clone(),
        artifact_id: spec.artifact_id.clone(),
        version: String::new(),
        classifier: None,
        extension: "jar".to_string(),
    };
    let outcome = client.fetch_version_listing(&coord)?.ok_or_else(|| {
        JxError::new(
            Kind::Network,
            format!(
                "no configured repository publishes a version list for {}:{}",
                spec.group_id, spec.artifact_id
            ),
        )
        .hint("write a version explicitly: `jx add group:artifact:version`")
    })?;
    let path = format!("{}/maven-metadata.xml", coord.metadata_directory(false));
    let text = String::from_utf8_lossy(&outcome.bytes);
    let metadata = jx_maven::xml::parse_metadata(&text, &path)?;
    jx_resolve::select_latest_stable(&metadata.versions).ok_or_else(|| {
        JxError::new(
            Kind::Resolve,
            format!(
                "{}:{} lists no stable release",
                spec.group_id, spec.artifact_id
            ),
        )
        .hint("only pre-releases are published; pin one explicitly with `=`")
    })
}

/// The classpaths javac/java should use. This is the lock fast path: an
/// existing lock is served from the cache; a missing or older-than-manifest
/// lock is refreshed first. Returns absolute cache paths, never jar bytes.
pub(crate) fn classpaths(cli: &Cli) -> Result<Classpaths, JxError> {
    let project = Project::open(cli, "jx build")?;
    let declared = !project.manifest.dependencies.is_empty()
        || !project.manifest.dev_dependencies.is_empty()
        || !project.manifest.processors.is_empty()
        || !project.manifest.overrides.is_empty();
    let lock_path = project.lock_path();
    if !lock_path.exists() {
        if !declared {
            return Ok(Classpaths {
                main: Vec::new(),
                main_jars: Vec::new(),
                test: Vec::new(),
                processors: Vec::new(),
                all_jars: Vec::new(),
                junit_launcher: None,
                junit_launcher_version: None,
            });
        }
        if cli.locked {
            return Err(JxError::new(
                Kind::Lock,
                "--locked: project.toml declares dependencies but jx.lock is missing",
            )
            .at(lock_path.display().to_string())
            .hint("run `jx lock` once without --locked"));
        }
        write_lock(cli, &project)?;
    } else if manifest_is_newer(&project) {
        // Refresh even when `declared` is now false: dependencies removed by
        // hand must drop out of the classpath instead of lingering in a stale
        // lock.
        if cli.locked {
            return Err(
                JxError::new(Kind::Lock, "--locked: project.toml is newer than jx.lock")
                    .at(lock_path.display().to_string())
                    .hint("run `jx lock` to bring the lockfile up to date"),
            );
        }
        write_lock(cli, &project)?;
    }
    let lock = read_lock_for(&project)?;
    let (_, downloaded, _) = fetch_pinned(&project.client, &lock)?;
    if downloaded > 0 && !cli.quiet {
        println!(
            "  fetched {downloaded} locked artifact{}",
            if downloaded == 1 { "" } else { "s" }
        );
    }
    classpaths_from_lock(&lock, &project.home)
}

/// Turn a verified lock into the classpaths a build uses. App artifacts
/// reach javac and `java`; dev-only artifacts and the JUnit launcher stay on
/// the test side; processor-only artifacts reach javac's `-processorpath`
/// and nothing else (ADR 0014).
fn classpaths_from_lock(lock: &Lockfile, home: &Path) -> Result<Classpaths, JxError> {
    let cache = jx_common::cache::Cache::new(home);
    let mut main = Vec::new();
    let mut main_jars = Vec::new();
    let mut test = Vec::new();
    let mut processors = Vec::new();
    let mut all_jars = Vec::new();
    let mut junit_launcher = None;
    let mut junit_launcher_version = None;
    for artifact in &lock.artifacts {
        let path = cache.path_of(&artifact.sha256)?;
        let jar = LockedJar {
            group_id: artifact.group_id.clone(),
            artifact_id: artifact.artifact_id.clone(),
            version: artifact.version.clone(),
            classifier: artifact.classifier.clone(),
            path: path.clone(),
        };
        if artifact.app {
            all_jars.push(jar.clone());
            if !artifact.dev {
                main.push(path.clone());
                main_jars.push(jar);
            }
            if artifact.artifact_id == "junit-platform-console-standalone" {
                junit_launcher = Some(path.clone());
                junit_launcher_version = Some(artifact.version.clone());
            }
            test.push(path.clone());
        }
        if artifact.processor {
            processors.push(path);
        }
    }
    Ok(Classpaths {
        main,
        main_jars,
        test,
        processors,
        all_jars,
        junit_launcher,
        junit_launcher_version,
    })
}

fn manifest_is_newer(project: &Project) -> bool {
    let modified = |path: &Path| std::fs::metadata(path).and_then(|m| m.modified()).ok();
    match (
        modified(&project.manifest_path),
        modified(&project.lock_path()),
    ) {
        (Some(manifest), Some(lock)) => manifest > lock,
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// Lock writing and fetching
// ---------------------------------------------------------------------------

fn write_lock(cli: &Cli, project: &Project) -> Result<LockReport, JxError> {
    let resolution = project.resolve()?;
    let processors = project.resolve_processors()?;
    write_resolution(cli, project, &resolution, &processors)
}

fn write_resolution(
    cli: &Cli,
    project: &Project,
    resolution: &Resolution,
    processors: &Resolution,
) -> Result<LockReport, JxError> {
    report_conflicts(resolution);
    report_conflicts(processors);
    let lock = build_lock(project, resolution, processors)?;
    let path = project.lock_path();
    let changes = lock_changes(read_optional_lock(project)?.as_ref(), &lock);

    if cli.locked {
        let existing = match jx_lock::read(&path) {
            Ok(lock) => lock,
            Err(_) => {
                return Err(
                    JxError::new(Kind::Lock, "--locked: jx.lock is missing or unreadable")
                        .at(path.display().to_string())
                        .hint("run `jx lock` once without --locked to create it"),
                );
            }
        };
        if existing != lock {
            return Err(
                JxError::new(Kind::Lock, "--locked: resolving would change jx.lock")
                    .at(path.display().to_string())
                    .hint("run `jx lock` to accept the new versions"),
            );
        }
        return Ok(LockReport {
            artifacts: lock.artifacts.len(),
            changed: false,
            changes,
        });
    }

    let changed = jx_lock::save(&path, &lock)?;
    Ok(LockReport {
        artifacts: lock.artifacts.len(),
        changed,
        changes,
    })
}

/// Resolve every node's artifact to bytes, verify it against an existing
/// lock when the version did not change, and build the canonical lockfile.
fn build_lock(
    project: &Project,
    resolution: &Resolution,
    processors: &Resolution,
) -> Result<Lockfile, JxError> {
    let existing = if project.lock_path().exists() {
        Some(jx_lock::read(&project.lock_path())?)
    } else {
        None
    };
    let mut artifacts: Vec<PinnedArtifact> = resolution
        .nodes
        .iter()
        .map(|node| pinned(node, node.dev, true, false))
        .collect();
    // The two graphs meet only here. A GA at two versions cannot be stored
    // in one lockfile, so it is a hard error with the override that fixes
    // it; the same version in both roles becomes one shared entry.
    for node in &processors.nodes {
        if let Some(other) = artifacts.iter().find(|artifact| {
            artifact.group_id == node.group_id
                && artifact.artifact_id == node.artifact_id
                && artifact.version != node.version
        }) {
            return Err(JxError::new(
                Kind::Lock,
                format!(
                    "{}:{} is {} in the app graph and {} in [processors]",
                    node.group_id, node.artifact_id, other.version, node.version
                ),
            )
            .at("jx.lock")
            .hint(format!(
                "one lockfile records one version per group:artifact; add `\"{}:{}\" = \"<version>\"` to [overrides] to pin both graphs, or move the processor to its own project",
                node.group_id, node.artifact_id
            )));
        }
        if let Some(shared) = artifacts.iter_mut().find(|artifact| {
            artifact.group_id == node.group_id
                && artifact.artifact_id == node.artifact_id
                && artifact.classifier == node.classifier
                && artifact.version == node.version
        }) {
            shared.processor = true;
            continue;
        }
        artifacts.push(pinned(node, false, false, true));
    }

    // Fetch bytes for every distinct coordinate. A jar shared by both graphs
    // was merged above, so each coordinate is walked once.
    let mut missing = Vec::new();
    for (index, artifact) in artifacts.iter_mut().enumerate() {
        let coord = coordinate_of_pin(artifact);
        if project.client.is_cached(&coord.artifact_path()) {
            let outcome = project.client.fetch_cached(&coord.artifact_path())?;
            artifact.sha256 = outcome.sha256_hex;
        } else {
            missing.push((index, coord));
        }
    }
    if !missing.is_empty() {
        let coords: Vec<Coordinate> = missing.iter().map(|(_, coord)| coord.clone()).collect();
        let results = project.client.fetch_all(&coords);
        for ((index, coord), result) in missing.into_iter().zip(results) {
            let outcome = result.outcome.map_err(|error| {
                error.hint(format!(
                    "could not fetch {}; `jx lock` needs each artifact's bytes to record its sha256",
                    coord.key()
                ))
            })?;
            artifacts[index].sha256 = outcome.sha256_hex;
        }
    }
    artifacts.sort_by(|a, b| {
        (&a.group_id, &a.artifact_id, &a.classifier, &a.version).cmp(&(
            &b.group_id,
            &b.artifact_id,
            &b.classifier,
            &b.version,
        ))
    });
    if let Some(existing) = &existing {
        for pinned in &artifacts {
            if let Some(old) = existing.find(
                &pinned.group_id,
                &pinned.artifact_id,
                pinned.classifier.as_deref(),
            ) && old.version == pinned.version
                && !old.sha256.eq_ignore_ascii_case(&pinned.sha256)
            {
                return Err(JxError::new(
                    Kind::Lock,
                    format!(
                        "the sha256 of {}:{}:{} changed since jx.lock was written",
                        pinned.group_id, pinned.artifact_id, pinned.version
                    ),
                )
                .at("jx.lock")
                .hint(
                    "released artifacts must be immutable; if this repository is trusted, \
                     delete jx.lock deliberately and run `jx lock` again",
                ));
            }
        }
    }
    // Adopt the newest installed JDK for the feature so the pin tracks the
    // toolchain a build will use. Without one, keep a matching existing pin:
    // there is no installed release to derive a checksum from.
    let feature = project.manifest.jdk_feature()?;
    let existing_jdk = existing.as_ref().and_then(|lock| lock.jdk.clone());
    let jdk = match installed_jdk_pin(project, feature)? {
        Some(pin) => Some(pin),
        None => existing_jdk.filter(|pin| pin.feature == feature),
    };
    Ok(Lockfile {
        version: 3,
        jdk,
        artifacts,
    })
}

/// The `[jdk]` entry for the newest installed JDK of `feature`, with the
/// checksum of the archive it was installed from. `None` when no such JDK is
/// installed or its install archive is no longer cached — the lock stays
/// unpinned rather than claiming a toolchain that is not local.
fn installed_jdk_pin(
    project: &Project,
    feature: u32,
) -> Result<Option<jx_lock::PinnedJdk>, JxError> {
    let Some(jdk) = jx_toolchain::installed_for_feature(&project.home, feature)? else {
        return Ok(None);
    };
    let Some(archive) = jx_toolchain::cached_archive(&project.home, &jdk.version)? else {
        return Ok(None);
    };
    Ok(Some(jx_lock::PinnedJdk {
        provider: jdk.vendor,
        feature,
        release: jdk.version,
        sha256: jx_toolchain::extract::sha256_file(&archive)?,
    }))
}

/// A lock entry before its bytes are fetched. The sha256 is filled by
/// `build_lock`; an empty value never reaches the writer.
fn pinned(node: &SolvedNode, dev: bool, app: bool, processor: bool) -> PinnedArtifact {
    PinnedArtifact {
        group_id: node.group_id.clone(),
        artifact_id: node.artifact_id.clone(),
        version: node.version.clone(),
        classifier: node.classifier.clone(),
        sha256: String::new(),
        dev,
        app,
        processor,
    }
}

fn read_lock_for(project: &Project) -> Result<Lockfile, JxError> {
    jx_lock::read(&project.lock_path())
        .map_err(|error| error.hint("run `jx lock` first; it records the artifacts for `jx fetch`"))
}

/// Fetch (or re-verify) every lock entry. Returns (cached, downloaded, bytes).
fn fetch_pinned(client: &Client, lock: &Lockfile) -> Result<(usize, usize, usize), JxError> {
    let mut cached = 0;
    let mut bytes = 0usize;
    let mut missing = Vec::new();
    for artifact in &lock.artifacts {
        let coord = coordinate_of_pin(artifact);
        if client.is_cached(&coord.artifact_path()) {
            let outcome = client.fetch_cached(&coord.artifact_path())?;
            verify_pinned(artifact, &outcome.sha256_hex)?;
            cached += 1;
            bytes += outcome.bytes.len();
        } else {
            missing.push((artifact, coord));
        }
    }
    let downloaded = missing.len();
    let coords: Vec<Coordinate> = missing.iter().map(|(_, coord)| coord.clone()).collect();
    let results = client.fetch_all(&coords);
    for ((artifact, _), result) in missing.into_iter().zip(results) {
        let outcome = result.outcome?;
        verify_pinned(artifact, &outcome.sha256_hex)?;
        bytes += outcome.bytes.len();
    }
    Ok((cached, downloaded, bytes))
}

fn verify_pinned(artifact: &PinnedArtifact, actual: &str) -> Result<(), JxError> {
    if actual.eq_ignore_ascii_case(&artifact.sha256) {
        return Ok(());
    }
    Err(JxError::new(
        Kind::Lock,
        format!(
            "checksum mismatch for {}:{}:{}",
            artifact.group_id, artifact.artifact_id, artifact.version
        ),
    )
    .at("jx.lock")
    .hint(
        "the lockfile pins the first fetched bytes; an upstream repository replaced this \
         release. Delete jx.lock deliberately and re-lock if that is expected",
    ))
}

fn coordinate_of_pin(artifact: &PinnedArtifact) -> Coordinate {
    Coordinate {
        group_id: artifact.group_id.clone(),
        artifact_id: artifact.artifact_id.clone(),
        version: artifact.version.clone(),
        classifier: artifact.classifier.clone(),
        extension: "jar".to_string(),
    }
}

fn ensure_locked_jdk(home: &Path, pin: &jx_lock::PinnedJdk, offline: bool) -> Result<(), JxError> {
    // The lock records an exact build; never silently substitute another
    // build of the same feature release. The store reuses an installed or
    // cached copy and verifies the archive against the pinned sha256.
    jx_toolchain::ensure_jdk_release(
        home,
        &pin.provider,
        pin.feature,
        &pin.release,
        &pin.sha256,
        offline,
    )?;
    Ok(())
}

fn print_lock_summary(project: &Project, report: &LockReport) {
    let path = project.lock_path();
    if report.changed {
        println!(
            "jx.lock: {} artifact{} written to {}",
            report.artifacts,
            if report.artifacts == 1 { "" } else { "s" },
            path.display()
        );
    } else {
        println!(
            "jx.lock: up to date ({} artifact{})",
            report.artifacts,
            if report.artifacts == 1 { "" } else { "s" },
        );
    }
}

// ---------------------------------------------------------------------------
// Reports
// ---------------------------------------------------------------------------

fn report_conflicts(resolution: &Resolution) {
    for conflict in &resolution.conflicts {
        eprintln!(
            "warning: {}:{} {} (depth {}) loses to {} (depth {}); the nearer version wins",
            conflict.group_id,
            conflict.artifact_id,
            conflict.loser_version,
            conflict.loser_depth,
            conflict.winner_version,
            conflict.winner_depth,
        );
    }
}

fn print_tree(resolution: &Resolution, verbose: bool) {
    if resolution.nodes.is_empty() {
        println!("no dependencies declared");
        return;
    }
    let by_key: HashMap<String, &SolvedNode> = resolution
        .nodes
        .iter()
        .map(|node| (node.key(), node))
        .collect();
    let mut children: HashMap<String, Vec<&Edge>> = HashMap::new();
    let mut roots = Vec::new();
    for edge in &resolution.edges {
        match &edge.parent {
            Some(parent) => children.entry(parent.clone()).or_default().push(edge),
            None => roots.push(edge),
        }
    }
    // A coordinate declared in both tables enters twice; show it once.
    let mut seen_roots = HashSet::new();
    let mut expanded = HashSet::new();
    for edge in roots {
        if !seen_roots.insert(edge.key()) {
            continue;
        }
        print_edge(edge, &by_key, &children, 0, &mut expanded);
    }

    for conflict in &resolution.conflicts {
        eprintln!(
            "conflict: {}:{} {} (depth {}) -> {} kept (depth {})",
            conflict.group_id,
            conflict.artifact_id,
            conflict.loser_version,
            conflict.loser_depth,
            conflict.winner_version,
            conflict.winner_depth,
        );
    }
    if !resolution.platforms.is_empty() {
        println!("platforms: {}", resolution.platforms.join(", "));
    }
    if verbose {
        for skipped in &resolution.skipped {
            println!(
                "skipped: {} -> {}:{} ({})",
                skipped.parent, skipped.group_id, skipped.artifact_id, skipped.reason
            );
        }
    }
}

fn print_edge(
    edge: &Edge,
    by_key: &HashMap<String, &SolvedNode>,
    children: &HashMap<String, Vec<&Edge>>,
    depth: usize,
    expanded: &mut HashSet<String>,
) {
    let key = edge.key();
    let version = by_key
        .get(&key)
        .map(|node| node.version.as_str())
        .unwrap_or(edge.version.as_str());
    let indent = "  ".repeat(depth);
    let mut line = format!("{indent}{}:{}:{version}", edge.group_id, edge.artifact_id);
    if let Some(classifier) = &edge.classifier {
        line.push(':');
        line.push_str(classifier);
    }
    if version != edge.version {
        line.push_str(&format!(" (requested {})", edge.version));
    }
    if !expanded.insert(key.clone()) {
        line.push_str(" (*)");
        println!("{line}");
        return;
    }
    println!("{line}");
    if let Some(next) = children.get(&key) {
        let mut seen = HashSet::new();
        for child in next {
            if !seen.insert(child.key()) {
                continue;
            }
            print_edge(child, by_key, children, depth + 1, expanded);
        }
    }
}

fn parse_why_target(text: &str) -> Result<(String, String, Option<String>), JxError> {
    let parts: Vec<&str> = text.split(':').collect();
    if !(2..=3).contains(&parts.len()) || parts.iter().any(|part| part.is_empty()) {
        return Err(JxError::new(
            Kind::Usage,
            format!("`{text}` is not `group:artifact` or `group:artifact:version`"),
        )
        .hint("example: `jx why com.google.guava:guava`"));
    }
    Ok((
        parts[0].to_string(),
        parts[1].to_string(),
        parts.get(2).map(|version| version.to_string()),
    ))
}

fn why_in(
    resolution: &Resolution,
    group_id: &str,
    artifact_id: &str,
    wanted_version: Option<&str>,
    origin: &str,
) -> Result<(), JxError> {
    let node = why_node(resolution, group_id, artifact_id, wanted_version).ok_or_else(|| {
        let available: Vec<String> = resolution
            .nodes
            .iter()
            .filter(|node| node.group_id == group_id && node.artifact_id == artifact_id)
            .map(|node| node.version.clone())
            .collect();
        let message = if available.is_empty() {
            format!("{group_id}:{artifact_id} is not in the resolved graph")
        } else {
            format!(
                "{group_id}:{artifact_id} resolves to {}, not {}",
                available.join(", "),
                wanted_version.unwrap_or("")
            )
        };
        JxError::new(Kind::Resolve, message)
            .at(format!("{group_id}:{artifact_id}"))
            .hint("`jx tree` shows every resolved artifact and where it entered")
    })?;
    let target = node.key();
    let by_key: HashMap<String, &SolvedNode> = resolution
        .nodes
        .iter()
        .map(|node| (node.key(), node))
        .collect();
    let mut children: HashMap<String, Vec<&Edge>> = HashMap::new();
    let mut frontier: VecDeque<String> = VecDeque::new();
    let mut came_from: HashMap<String, &Edge> = HashMap::new();
    for edge in resolution.edges.iter().filter(|edge| edge.parent.is_none()) {
        let key = edge.key();
        if came_from.insert(key.clone(), edge).is_none() {
            frontier.push_back(key);
        }
    }
    for edge in &resolution.edges {
        if let Some(parent) = &edge.parent {
            children.entry(parent.clone()).or_default().push(edge);
        }
    }
    while let Some(key) = frontier.pop_front() {
        if key == target {
            break;
        }
        if let Some(next) = children.get(&key) {
            for edge in next {
                let child = edge.key();
                if !came_from.contains_key(&child) {
                    came_from.insert(child.clone(), edge);
                    frontier.push_back(child);
                }
            }
        }
    }
    let mut path = Vec::new();
    let mut cursor = target.clone();
    loop {
        let Some(edge) = came_from.get(&cursor) else {
            return Err(JxError::new(
                Kind::Resolve,
                format!(
                    "{group_id}:{artifact_id} is resolved but has no path from a manifest root"
                ),
            )
            .hint("this is a resolver bug worth reporting"));
        };
        path.push(*edge);
        match &edge.parent {
            Some(parent) => cursor = parent.clone(),
            None => break,
        }
    }
    path.reverse();
    for (index, edge) in path.iter().enumerate() {
        let key = edge.key();
        let version = by_key
            .get(&key)
            .map(|node| node.version.as_str())
            .unwrap_or(edge.version.as_str());
        let indent = "  ".repeat(index);
        let direct = if edge.parent.is_none() {
            " (direct)"
        } else {
            ""
        };
        println!(
            "{indent}{}:{}:{version}{direct}",
            edge.group_id, edge.artifact_id
        );
    }
    println!(
        "{group_id}:{artifact_id} is {} level{} from {origin}",
        path.len(),
        if path.len() == 1 { "" } else { "s" }
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use jx_common::cache::Cache;
    use tempfile::tempdir;

    #[test]
    fn lock_scope_splits_main_test_and_processor_classpaths() {
        let dir = tempdir().unwrap();
        let cache = Cache::new(dir.path());
        let main_sha = cache.put(b"main jar").unwrap();
        let dev_sha = cache.put(b"dev jar").unwrap();
        let launcher_sha = cache.put(b"junit console").unwrap();
        let processor_sha = cache.put(b"processor jar").unwrap();
        let shared_sha = cache.put(b"shared jar").unwrap();
        let artifact =
            |name: &str, sha: &str, dev: bool, app: bool, processor: bool| PinnedArtifact {
                group_id: "g".to_string(),
                artifact_id: name.to_string(),
                version: "1.0".to_string(),
                classifier: None,
                sha256: sha.to_string(),
                dev,
                app,
                processor,
            };
        let lock = Lockfile {
            version: 3,
            jdk: None,
            artifacts: vec![
                artifact("main", &main_sha, false, true, false),
                artifact("test-only", &dev_sha, true, true, false),
                artifact(
                    "junit-platform-console-standalone",
                    &launcher_sha,
                    true,
                    true,
                    false,
                ),
                artifact("processor-only", &processor_sha, false, false, true),
                artifact("shared", &shared_sha, false, true, true),
            ],
        };
        let cp = classpaths_from_lock(&lock, dir.path()).unwrap();
        // `shared` is a production app dependency that also happens to be a
        // processor, so it belongs on the main path as well.
        assert_eq!(
            cp.main,
            vec![
                cache.path_of(&main_sha).unwrap(),
                cache.path_of(&shared_sha).unwrap()
            ]
        );
        assert_eq!(cp.main_jars.len(), 2);
        assert_eq!(cp.main_jars[0].artifact_id, "main");
        assert_eq!(cp.main_jars[1].artifact_id, "shared");
        // The test classpath is every app artifact; processors stay out.
        assert_eq!(cp.test.len(), 4);
        assert!(!cp.test.contains(&cache.path_of(&processor_sha).unwrap()));
        assert_eq!(
            cp.processors,
            vec![
                cache.path_of(&processor_sha).unwrap(),
                cache.path_of(&shared_sha).unwrap()
            ]
        );
        // A jar that serves both roles is on every path it serves.
        assert!(cp.test.contains(&cache.path_of(&shared_sha).unwrap()));
        assert_eq!(cp.all_jars.len(), 4);
        assert_eq!(
            cp.junit_launcher,
            Some(cache.path_of(&launcher_sha).unwrap())
        );
        assert_eq!(cp.junit_launcher_version.as_deref(), Some("1.0"));
    }
}
