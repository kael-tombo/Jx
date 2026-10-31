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
/// through `[dev-dependencies]`; `test` contains everything.
pub(crate) struct Classpaths {
    pub main: Vec<PathBuf>,
    pub main_jars: Vec<LockedJar>,
    pub test: Vec<PathBuf>,
    /// Every locked artifact, including dev-only ones. Used by exporters that
    /// must map manifest declarations to concrete versions.
    pub all_jars: Vec<LockedJar>,
    /// The JUnit Platform console standalone jar, when the project depends on it.
    pub junit_launcher: Option<PathBuf>,
}

struct Project {
    manifest_path: PathBuf,
    manifest_dir: PathBuf,
    manifest: manifest::Manifest,
    home: PathBuf,
    client: Client,
}

impl Project {
    fn open(cli: &Cli) -> Result<Project, JxError> {
        let manifest_path = crate::locate_manifest(cli)?;
        let manifest = manifest::parse(&manifest_path)?;
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
        let loader = PomLoader::new(&self.client);
        jx_resolve::resolve(
            &loader,
            &self.client,
            &self.manifest.dependencies,
            &self.manifest.dev_dependencies,
            &self.manifest.overrides,
        )
    }
}

/// What a lock write did, for the command's summary line.
struct LockReport {
    artifacts: usize,
    changed: bool,
}

/// `jx lock`: resolve and write the lockfile, fetching artifact bytes to
/// record their sha256. `--locked` makes any difference a hard failure.
pub fn lock(cli: &Cli) -> Result<(), JxError> {
    let project = Project::open(cli)?;
    let report = write_lock(cli, &project)?;
    print_lock_summary(&project, &report);
    Ok(())
}

/// `jx fetch`: download exactly what the lockfile pins, verifying every
/// checksum against it. Cached entries are re-verified, not trusted.
pub fn fetch(cli: &Cli) -> Result<(), JxError> {
    let project = Project::open(cli)?;
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
    let project = Project::open(cli)?;
    let resolution = project.resolve()?;
    print_tree(&resolution, cli.verbose);
    Ok(())
}

/// `jx why <group:artifact[:version]>`: one path from a manifest root.
pub fn why(cli: &Cli, coord: &str) -> Result<(), JxError> {
    let project = Project::open(cli)?;
    let resolution = project.resolve()?;
    let (group_id, artifact_id, wanted_version) = parse_why_target(coord)?;
    why_in(
        &resolution,
        &group_id,
        &artifact_id,
        wanted_version.as_deref(),
    )?;
    Ok(())
}

/// `jx add`: append a validated dependency, then resolve and lock. Any
/// failure restores the manifest byte for byte.
pub fn add(
    cli: &Cli,
    dependency: &str,
    dev: bool,
    platform: bool,
    alias: Option<String>,
) -> Result<(), JxError> {
    if cli.locked {
        return Err(JxError::new(
            Kind::Lock,
            "--locked: adding a dependency would change jx.lock",
        )
        .hint("drop --locked for this edit, or update project.toml by hand"));
    }
    let mut project = Project::open(cli)?;
    let table = table_of(dev);

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
pub fn remove(cli: &Cli, alias: &str, dev: bool) -> Result<(), JxError> {
    if cli.locked {
        return Err(JxError::new(
            Kind::Lock,
            "--locked: removing a dependency would change jx.lock",
        )
        .hint("drop --locked for this edit, or update project.toml by hand"));
    }
    let mut project = Project::open(cli)?;
    let table = table_of(dev);
    let other = if dev {
        DepTable::Dependencies
    } else {
        DepTable::DevDependencies
    };
    if !manifest::has_alias(&project.manifest_path, table, alias)?
        && manifest::has_alias(&project.manifest_path, other, alias)?
    {
        return Err(JxError::new(
            Kind::Manifest,
            format!(
                "`{alias}` is declared in [{}], not [{}]",
                other.name(),
                table.name()
            ),
        )
        .hint(format!(
            "run `jx remove {alias} --dev` to target that table"
        )));
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

fn table_of(dev: bool) -> DepTable {
    if dev {
        DepTable::DevDependencies
    } else {
        DepTable::Dependencies
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
    let project = Project::open(cli)?;
    let declared = !project.manifest.dependencies.is_empty()
        || !project.manifest.dev_dependencies.is_empty()
        || !project.manifest.overrides.is_empty();
    let lock_path = project.lock_path();
    if !lock_path.exists() {
        if !declared {
            return Ok(Classpaths {
                main: Vec::new(),
                main_jars: Vec::new(),
                test: Vec::new(),
                all_jars: Vec::new(),
                junit_launcher: None,
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

/// Turn a verified lock into the two classpaths a build uses. Main artifacts
/// reach javac and `java`; dev-only artifacts and the JUnit launcher stay on
/// the test side.
fn classpaths_from_lock(lock: &Lockfile, home: &Path) -> Result<Classpaths, JxError> {
    let cache = jx_common::cache::Cache::new(home);
    let mut main = Vec::new();
    let mut main_jars = Vec::new();
    let mut test = Vec::new();
    let mut all_jars = Vec::new();
    let mut junit_launcher = None;
    for artifact in &lock.artifacts {
        let path = cache.path_of(&artifact.sha256)?;
        let jar = LockedJar {
            group_id: artifact.group_id.clone(),
            artifact_id: artifact.artifact_id.clone(),
            version: artifact.version.clone(),
            classifier: artifact.classifier.clone(),
            path: path.clone(),
        };
        all_jars.push(jar.clone());
        if !artifact.dev {
            main.push(path.clone());
            main_jars.push(jar);
        }
        if artifact.artifact_id == "junit-platform-console-standalone" {
            junit_launcher = Some(path.clone());
        }
        test.push(path);
    }
    Ok(Classpaths {
        main,
        main_jars,
        test,
        all_jars,
        junit_launcher,
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
    report_conflicts(&resolution);
    let lock = build_lock(project, &resolution)?;
    let path = project.lock_path();

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
        });
    }

    let changed = jx_lock::save(&path, &lock)?;
    Ok(LockReport {
        artifacts: lock.artifacts.len(),
        changed,
    })
}

/// Resolve every node's artifact to bytes, verify it against an existing
/// lock when the version did not change, and build the canonical lockfile.
fn build_lock(project: &Project, resolution: &Resolution) -> Result<Lockfile, JxError> {
    let existing = if project.lock_path().exists() {
        Some(jx_lock::read(&project.lock_path())?)
    } else {
        None
    };
    let mut artifacts = Vec::with_capacity(resolution.nodes.len());
    let mut missing = Vec::new();
    for node in &resolution.nodes {
        let coord = coordinate(node);
        if project.client.is_cached(&coord.artifact_path()) {
            let outcome = project.client.fetch_cached(&coord.artifact_path())?;
            artifacts.push(pinned(node, outcome.sha256_hex));
        } else {
            missing.push((node, coord));
        }
    }
    if !missing.is_empty() {
        let coords: Vec<Coordinate> = missing.iter().map(|(_, coord)| coord.clone()).collect();
        let results = project.client.fetch_all(&coords);
        for ((node, coord), result) in missing.into_iter().zip(results) {
            let outcome = result.outcome.map_err(|error| {
                error.hint(format!(
                    "could not fetch {}; `jx lock` needs each artifact's bytes to record its sha256",
                    coord.key()
                ))
            })?;
            artifacts.push(pinned(node, outcome.sha256_hex));
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
    Ok(Lockfile {
        version: 2,
        jdk: existing.and_then(|lock| lock.jdk),
        artifacts,
    })
}

fn coordinate(node: &SolvedNode) -> Coordinate {
    Coordinate {
        group_id: node.group_id.clone(),
        artifact_id: node.artifact_id.clone(),
        version: node.version.clone(),
        classifier: node.classifier.clone(),
        extension: "jar".to_string(),
    }
}

fn pinned(node: &SolvedNode, sha256: String) -> PinnedArtifact {
    PinnedArtifact {
        group_id: node.group_id.clone(),
        artifact_id: node.artifact_id.clone(),
        version: node.version.clone(),
        classifier: node.classifier.clone(),
        sha256,
        dev: node.dev,
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
    // build of the same feature release.
    let installed = jx_toolchain::list_installed(home)?;
    if installed
        .iter()
        .any(|jdk| jdk.vendor == pin.provider && jdk.version == pin.release && pin.feature > 0)
    {
        return Ok(());
    }
    if offline {
        return Err(JxError::new(
            Kind::Toolchain,
            format!(
                "jx.lock pins {} {}, which is not installed, and --offline forbids downloading",
                pin.provider, pin.release
            ),
        )
        .hint(format!("run `jx jdk install {}` once online", pin.feature)));
    }
    if pin.provider != "temurin" {
        return Err(JxError::new(
            Kind::Toolchain,
            format!(
                "jx.lock pins provider `{}`, which this jx does not install",
                pin.provider
            ),
        )
        .hint("V1 installs Eclipse Temurin only; change the pin deliberately"));
    }
    let installed = jx_toolchain::ensure_jdk(home, pin.feature)?;
    if installed.version != pin.release {
        return Err(JxError::new(
            Kind::Toolchain,
            format!(
                "jx.lock pins temurin {}, but the provider now installs {}",
                pin.release, installed.version
            ),
        )
        .hint(
            "exact-build installation arrives with the M8 toolchain pin work; remove the \
             [jdk] entry or re-lock deliberately",
        ));
    }
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
) -> Result<(), JxError> {
    let node = resolution
        .nodes
        .iter()
        .find(|node| {
            node.group_id == group_id
                && node.artifact_id == artifact_id
                && wanted_version.is_none_or(|wanted| node.version == wanted)
        })
        .ok_or_else(|| {
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
        "{group_id}:{artifact_id} is {} level{} from the manifest",
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
    fn lock_scope_splits_main_from_test_classpaths() {
        let dir = tempdir().unwrap();
        let cache = Cache::new(dir.path());
        let main_sha = cache.put(b"main jar").unwrap();
        let dev_sha = cache.put(b"dev jar").unwrap();
        let launcher_sha = cache.put(b"junit console").unwrap();
        let artifact = |name: &str, sha: &str, dev: bool| PinnedArtifact {
            group_id: "g".to_string(),
            artifact_id: name.to_string(),
            version: "1.0".to_string(),
            classifier: None,
            sha256: sha.to_string(),
            dev,
        };
        let lock = Lockfile {
            version: 2,
            jdk: None,
            artifacts: vec![
                artifact("main", &main_sha, false),
                artifact("test-only", &dev_sha, true),
                artifact("junit-platform-console-standalone", &launcher_sha, true),
            ],
        };
        let cp = classpaths_from_lock(&lock, dir.path()).unwrap();
        assert_eq!(cp.main, vec![cache.path_of(&main_sha).unwrap()]);
        assert_eq!(cp.main_jars.len(), 1);
        assert_eq!(cp.main_jars[0].artifact_id, "main");
        assert_eq!(cp.test.len(), 3);
        assert_eq!(
            cp.junit_launcher,
            Some(cache.path_of(&launcher_sha).unwrap())
        );
    }
}
