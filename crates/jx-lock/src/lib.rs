//! `jx.lock` — the reproducibility record, specified in
//! `docs/decisions/0004-lockfile-and-resolution-policy.md`.
//!
//! The lockfile lists every artifact the build consumes with the exact
//! version resolved at `jx resolve`/`jx add` time and the sha256 that
//! artifacts must check out as. Reading it is the resolver's *fast path*:
//! with a lock and a present cache entry, no network is touched.
//!
//! Serialization is deterministic and canonical (sorted artifacts, stable
//! key order, atomic whole-file replace), because `jx.lock` is a
//! reviewable diff, not a write cache.

use std::path::Path;

use jx_common::diagnose::unknown_key_error;
use jx_common::error::{JxError, Kind};

/// The newest schema jx writes. Version 1 locks are still read (every entry
/// counts as a main artifact); version 2 adds the optional `dev` flag;
/// version 3 adds the optional `app` and `processor` flags (ADR 0014).
const SCHEMA_VERSION: u32 = 3;

const LOCK_KEYS: [&str; 3] = ["artifact", "jdk", "version"];
const JDK_KEYS: [&str; 4] = ["feature", "provider", "release", "sha256"];
const ARTIFACT_KEYS: [&str; 8] = [
    "app",
    "artifact",
    "classifier",
    "dev",
    "group",
    "processor",
    "sha256",
    "version",
];

/// The exact toolchain build the lock was produced with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PinnedJdk {
    pub provider: String,
    /// The feature release pin, `25`-style.
    pub feature: u32,
    /// The exact release build, as the provider spells it.
    pub release: String,
    /// The archive's sha256, the cache key.
    pub sha256: String,
}

/// One buildable artifact: identity + resolved version + integrity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PinnedArtifact {
    pub group_id: String,
    pub artifact_id: String,
    /// The concrete version string (`1.0-20250101.070011-12` shape included).
    pub version: String,
    pub classifier: Option<String>,
    /// sha256 of the artifact bytes, the content-cache key.
    pub sha256: String,
    /// Reachable only through `[dev-dependencies]`; kept out of the main
    /// compile/run classpath. Omitted (false) for main and for v1 locks.
    pub dev: bool,
    /// Part of the application graph (`[dependencies]` or
    /// `[dev-dependencies]`). `app = false` is serialized only for entries
    /// that exist solely for annotation processing.
    pub app: bool,
    /// On the annotation-processor path (`-processorpath`). An artifact can
    /// be both `app` and `processor` when the same jar serves both roles.
    pub processor: bool,
}

impl PinnedArtifact {
    /// The Maven key Maven-idiom style: `group:artifact[:classifier]`.
    pub fn key(&self) -> String {
        match &self.classifier {
            Some(c) => format!("{}:{}:{c}", self.group_id, self.artifact_id),
            None => format!("{}:{}", self.group_id, self.artifact_id),
        }
    }
}

/// A parsed, validated `jx.lock`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Lockfile {
    pub version: u32,
    pub jdk: Option<PinnedJdk>,
    /// Canonical order: `(group, artifact, classifier, version)`.
    pub artifacts: Vec<PinnedArtifact>,
}

impl Lockfile {
    /// Look up one artifact by identity.
    pub fn find(
        &self,
        group_id: &str,
        artifact_id: &str,
        classifier: Option<&str>,
    ) -> Option<&PinnedArtifact> {
        self.artifacts.iter().find(|a| {
            a.group_id == group_id
                && a.artifact_id == artifact_id
                && a.classifier.as_deref() == classifier
        })
    }
}

/// Read and validate a lockfile. A missing file is an error — callers
/// resolve first (there is no "empty lock" that means "everything is
/// allowed", which would mask cache misses).
pub fn read(path: &Path) -> Result<Lockfile, JxError> {
    let raw = std::fs::read_to_string(path).map_err(|e| {
        JxError::new(Kind::Lock, "cannot read jx.lock")
            .at(path.display().to_string())
            .caused_by(e)
            .hint("run `jx resolve` (or `jx add`), which writes the lockfile")
    })?;
    let lock = parse(&raw).map_err(|e| e.at(path.display().to_string()))?;
    Ok(lock)
}

/// Parse and validate lockfile text.
pub fn parse(raw: &str) -> Result<Lockfile, JxError> {
    let value: toml::Table = toml::from_str(raw).map_err(|e| {
        JxError::new(Kind::Lock, "cannot parse jx.lock (syntax error)")
            .caused_by(e)
            .hint("the error quote names the offending TOML construct")
    })?;
    let mut version: Option<i64> = None;
    let mut jdk_raw = None;
    let mut artifacts_raw = None;
    for (key, value) in &value {
        match key.as_str() {
            "version" => {
                version = value.as_integer();
            }
            "jdk" => {
                jdk_raw = Some(value.as_table().ok_or_else(|| {
                    JxError::new(Kind::Lock, "`[jdk]` in jx.lock must be a table")
                })?)
            }
            "artifact" => {
                artifacts_raw = Some(value.as_array().ok_or_else(|| {
                    JxError::new(
                        Kind::Lock,
                        "`[[artifact]]` in jx.lock must be an array of tables",
                    )
                })?)
            }
            other => {
                return Err(unknown_key_error(Kind::Lock, "jx.lock", other, &LOCK_KEYS));
            }
        }
    }
    let version = match version {
        None => {
            return Err(
                JxError::new(Kind::Lock, "jx.lock has no `version` key")
                    .hint("write the lockfile with `jx resolve`; a lock without a schema version is not this tool's"),
            )
        }
        Some(v) if v > i64::from(SCHEMA_VERSION) => {
            return Err(JxError::new(
                Kind::Lock,
                format!(
                    "this jx understands jx.lock versions 1 through {SCHEMA_VERSION}, the file says {v}"
                ),
            )
            .hint(
                "the lockfile was written by a newer jx; upgrade jx (or delete jx.lock and re-resolve)",
            ));
        }
        Some(v) if v < 1 => {
            return Err(JxError::new(
                Kind::Lock,
                format!("the lockfile `version` must be {SCHEMA_VERSION}"),
            )
            .hint("write the lockfile with `jx resolve`"));
        }
        Some(v) => u32::try_from(v)
            .expect("validated in range above"),
    };
    let jdk = match jdk_raw {
        None => None,
        Some(table) => Some(parse_jdk(table)?),
    };
    let mut artifacts = Vec::new();
    if let Some(entries) = artifacts_raw {
        for entry in entries {
            let table = entry.as_table().ok_or_else(|| {
                JxError::new(Kind::Lock, "each `[[artifact]]` entry must be a table")
            })?;
            artifacts.push(parse_artifact(table)?);
        }
    }
    // Canonical order, then duplicate detection over adjacent windows:
    // identical coordinates only ever sort next to each other.
    artifacts.sort_by(|a, b| {
        (
            a.group_id.clone(),
            a.artifact_id.clone(),
            a.classifier.clone(),
            a.version.clone(),
        )
            .cmp(&(
                b.group_id.clone(),
                b.artifact_id.clone(),
                b.classifier.clone(),
                b.version.clone(),
            ))
    });
    for window in artifacts.windows(2) {
        let (first, second) = (&window[0], &window[1]);
        if first.group_id == second.group_id
            && first.artifact_id == second.artifact_id
            && first.classifier == second.classifier
        {
            return Err(JxError::new(
                Kind::Lock,
                format!(
                    "the artifact {} is recorded twice in jx.lock (same group/artifact/classifier)",
                    second.key()
                ),
            )
            .hint("delete the duplicate entry and re-resolve"));
        }
    }
    Ok(Lockfile {
        version,
        jdk,
        artifacts,
    })
}

fn parse_jdk(table: &toml::Table) -> Result<PinnedJdk, JxError> {
    let mut provider: Option<String> = None;
    let mut feature: Option<u32> = None;
    let mut release: Option<String> = None;
    let mut sha256: Option<String> = None;
    for (key, value) in table {
        match key.as_str() {
            "provider" => provider = Some(string_field(key, value)?),
            "release" => release = Some(string_field(key, value)?),
            "sha256" => sha256 = Some(checksum(key, value)?),
            "feature" => {
                let raw = value.as_integer().ok_or_else(|| {
                    JxError::new(Kind::Lock, "`[jdk] feature` must be an integer")
                        .hint("`feature = 25`")
                })?;
                if raw <= 0 {
                    return Err(JxError::new(
                        Kind::Lock,
                        "`[jdk] feature` must be a positive feature release",
                    ));
                }
                feature =
                    Some(u32::try_from(raw).map_err(|_| {
                        JxError::new(Kind::Lock, "`[jdk] feature` is out of range")
                    })?);
            }
            other => return Err(unknown_key_error(Kind::Lock, "[jdk]", other, &JDK_KEYS)),
        }
    }
    let provider = provider.ok_or_else(|| missing("[jdk]", "provider"))?;
    let feature = feature.ok_or_else(|| missing("[jdk]", "feature"))?;
    let release = release.ok_or_else(|| missing("[jdk]", "release"))?;
    let sha256 = sha256.ok_or_else(|| missing("[jdk]", "sha256"))?;
    Ok(PinnedJdk {
        provider,
        feature,
        release,
        sha256,
    })
}

fn parse_artifact(table: &toml::Table) -> Result<PinnedArtifact, JxError> {
    let mut group: Option<String> = None;
    let mut artifact: Option<String> = None;
    let mut version: Option<String> = None;
    let mut classifier: Option<Option<String>> = None;
    let mut sha256: Option<String> = None;
    let mut dev = false;
    let mut app = true;
    let mut processor = false;
    for (key, value) in table {
        match key.as_str() {
            "group" => group = Some(string_field(key, value)?),
            "artifact" => artifact = Some(string_field(key, value)?),
            "version" => version = Some(string_field(key, value)?),
            "classifier" => classifier = Some(string_field(key, value).map(Some)?),
            "sha256" => sha256 = Some(checksum(key, value)?),
            "dev" => dev = bool_field(key, value)?,
            "app" => app = bool_field(key, value)?,
            "processor" => processor = bool_field(key, value)?,
            other => {
                return Err(unknown_key_error(
                    Kind::Lock,
                    "[[artifact]]",
                    other,
                    &ARTIFACT_KEYS,
                ));
            }
        }
    }
    let group_id = group.ok_or_else(|| missing("[[artifact]]", "group"))?;
    let artifact_id = artifact.ok_or_else(|| missing("[[artifact]]", "artifact"))?;
    let version = version.ok_or_else(|| missing("[[artifact]]", "version"))?;
    let sha256 = sha256.ok_or_else(|| missing("[[artifact]]", "sha256"))?;
    if !group_id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
    {
        return Err(JxError::new(
            Kind::Lock,
            format!("the [[artifact]] group {group_id:?} contains unexpected characters"),
        )
        .hint("groups are `com.google.guava`-shaped"));
    }
    if !artifact_id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
    {
        return Err(JxError::new(
            Kind::Lock,
            format!(
                "the [[artifact]] artifact name {artifact_id:?} contains unexpected characters"
            ),
        ));
    }
    Ok(PinnedArtifact {
        group_id,
        artifact_id,
        version,
        classifier: classifier.flatten(),
        sha256,
        dev,
        app,
        processor,
    })
}

fn missing(where_: &str, key: &str) -> JxError {
    JxError::new(Kind::Lock, format!("{where_} needs a `{key}`")).hint(write_jx_lock_hint())
}

fn string_field(key: &str, value: &toml::Value) -> Result<String, JxError> {
    value
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| JxError::new(Kind::Lock, format!("`{key}` in jx.lock must be a string")))
}

fn bool_field(key: &str, value: &toml::Value) -> Result<bool, JxError> {
    value.as_bool().ok_or_else(|| {
        JxError::new(
            Kind::Lock,
            format!("`{key}` in jx.lock must be `true` or `false`"),
        )
    })
}

fn checksum(key: &str, value: &toml::Value) -> Result<String, JxError> {
    let hex = string_field(key, value)?;
    let hex = hex.to_lowercase();
    if hex.len() != 64 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(JxError::new(
            Kind::Lock,
            format!("`{key}` in jx.lock is not a sha256 hex digest"),
        )
        .hint("run `jx fetch` again; the recorded value must be 64 hex characters"));
    }
    Ok(hex)
}

// ---------------------------------------------------------------------------
// Canonical, deterministic writing
// ---------------------------------------------------------------------------

/// Serialize in the canonical form of ADR 0004 and write it atomically.
/// Returns whether the file content changed.
pub fn save(path: &Path, lock: &Lockfile) -> Result<bool, JxError> {
    let canonical = to_canonical_text(lock);
    let existing = std::fs::read_to_string(path).ok();
    if existing.as_deref() == Some(canonical.as_str()) {
        return Ok(false);
    }
    write_atomic(path, &canonical)?;
    Ok(true)
}

/// The exact text jx.lock gets, byte for byte, for the given lock content.
pub fn to_canonical_text(lock: &Lockfile) -> String {
    let mut out = format!("version = {}\n", lock.version);
    if let Some(jdk) = &lock.jdk {
        out.push_str("\n[jdk]\n");
        out.push_str(&format!("provider = {}\n", quote(&jdk.provider)));
        out.push_str(&format!("feature = {}\n", jdk.feature));
        out.push_str(&format!("release = {}\n", quote(&jdk.release)));
        out.push_str(&format!("sha256 = {}\n", quote(&jdk.sha256)));
    }
    for artifact in &lock.artifacts {
        out.push_str("\n[[artifact]]\n");
        out.push_str(&format!("group = {}\n", quote(&artifact.group_id)));
        out.push_str(&format!("artifact = {}\n", quote(&artifact.artifact_id)));
        out.push_str(&format!("version = {}\n", quote(&artifact.version)));
        if let Some(classifier) = &artifact.classifier {
            out.push_str(&format!("classifier = {}\n", quote(classifier)));
        }
        if artifact.dev {
            out.push_str("dev = true\n");
        }
        if !artifact.app {
            out.push_str("app = false\n");
        }
        if artifact.processor {
            out.push_str("processor = true\n");
        }
        out.push_str(&format!("sha256 = {}\n", quote(&artifact.sha256)));
    }
    out
}

/// A TOML basic string with the two escapes the character sets above need
/// (`\` and `"`). Identifiers in validated coordinates never reach here with
/// anything else, but the lock is written from parsed input, so escape away.
fn quote(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Write through a temp file in the target directory, then put it in place.
/// `std::fs::rename` does not clobber on Windows, so an existing lockfile is
/// removed first; the idempotence check in `save` keeps this off the common
/// path, and a crash can only ever leave a `.jx.lock.tmp-<pid>` stray (M8
/// hardening may adopt a replace-file helper for a no-gap swap).
fn write_atomic(path: &Path, text: &str) -> Result<(), JxError> {
    let directory = path.parent().unwrap_or_else(|| Path::new("."));
    let tmp = directory.join(format!(".jx.lock.tmp-{}", std::process::id()));
    std::fs::write(&tmp, text).map_err(|e| {
        JxError::new(Kind::Io, "cannot write the temp lockfile")
            .at(tmp.display().to_string())
            .caused_by(e)
    })?;
    if path.exists()
        && let Err(e) = std::fs::remove_file(path)
    {
        let _ = std::fs::remove_file(&tmp);
        return Err(JxError::new(Kind::Io, "cannot replace jx.lock")
            .at(path.display().to_string())
            .caused_by(e)
            .hint("check the file is not open in another program and jx.lock is writable"));
    }
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(JxError::new(Kind::Io, "cannot finalise jx.lock")
            .at(path.display().to_string())
            .caused_by(e));
    }
    Ok(())
}

fn write_jx_lock_hint() -> &'static str {
    "write the lockfile with `jx resolve` (or `jx add`, which resolves then locks)"
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    const SAMPLE: &str = "version = 1\n\
\n\
[jdk]\n\
provider = \"temurin\"\n\
feature = 25\n\
release = \"25.0.1+9\"\n\
sha256 = \"6ad56e1b6b8a35eebb0b9b3aed6a09f4a1a9a9d1a2b3c4d5e6f7a8b9c0d1e2f3\"\n\
\n\
[[artifact]]\n\
group = \"com.s\"\n\
artifact = \"second\"\n\
version = \"2.0\"\n\
sha256 = \"1111111111111111111111111111111111111111111111111111111111111111\"\n\
\n\
[[artifact]]\n\
group = \"com.f\"\n\
artifact = \"first\"\n\
version = \"1.0\"\n\
classifier = \"tests\"\n\
sha256 = \"2222222222222222222222222222222222222222222222222222222222222222\"\n";

    #[test]
    fn parses_and_canonicalises() {
        let lock = parse(SAMPLE).expect("parsed");
        assert_eq!(lock.version, 1);
        assert!(
            lock.artifacts.iter().all(|artifact| !artifact.dev),
            "a v1 lock has no scope information; everything is main"
        );
        let jdk = lock.jdk.as_ref().expect("jdk entry");
        assert_eq!(jdk.provider, "temurin");
        assert_eq!(jdk.feature, 25);
        // Canonical order sorts (group, artifact, classifier …): com.f before
        // com.s regardless of file order.
        assert_eq!(lock.artifacts[0].key(), "com.f:first:tests");
        assert_eq!(lock.artifacts[1].key(), "com.s:second");
    }

    #[test]
    fn canonical_text_is_stable_and_round_trips() {
        let lock = parse(SAMPLE).expect("parsed");
        let text = to_canonical_text(&lock);
        let reparsed = parse(&text).expect("reparsed");
        assert_eq!(lock, reparsed);
        // And again: byte-identical the second time.
        assert_eq!(to_canonical_text(&reparsed), text);
    }

    #[test]
    fn duplicates_are_rejected() {
        // Make both entries `com.s:second` without a classifier: two versions
        // of the same identity is exactly the collision the lock rejects.
        let raw = SAMPLE
            .replace("com.f", "com.s")
            .replace("first", "second")
            .replace("classifier = \"tests\"\n", "");
        let error = parse(&raw).expect_err("two identical coordinates");
        assert!(error.message.contains("recorded twice"), "{error}");
    }

    #[test]
    fn unknown_keys_and_missing_fields_are_diagnosed() {
        let raw = SAMPLE.replace("[[artifact]]", "[[artifactd]]");
        let error = parse(&raw).expect_err("unknown top-level key");
        assert!(error.message.contains("did you mean `artifact`"), "{error}");

        let raw = SAMPLE.replace("group = \"com.s\"", "grup = \"com.s\"");
        let error = parse(&raw).expect_err("unknown artifact key");
        assert!(error.message.contains("did you mean `group`"), "{error}");

        let raw = SAMPLE.replace("feature = 25\n", "");
        let error = parse(&raw).expect_err("missing feature");
        assert!(error.message.contains("needs a `feature`"), "{error}");
    }

    #[test]
    fn bad_sha256_is_rejected() {
        let raw = SAMPLE.replace(
            "1111111111111111111111111111111111111111111111111111111111111111",
            "short",
        );
        let error = parse(&raw).expect_err("non-sha256 value");
        assert!(error.message.contains("sha256"), "{error}");
    }

    #[test]
    fn dotted_artifact_names_are_accepted() {
        // Real Maven artifactIds contain dots (`jakarta.annotation-api`); the
        // lock writer records them, so the reader must accept the same
        // identifier set as groups and manifest coordinates.
        let raw = SAMPLE.replace(
            "artifact = \"second\"",
            "artifact = \"jakarta.annotation-api\"",
        );
        let lock = parse(&raw).expect("dotted artifact names parse");
        assert!(
            lock.artifacts
                .iter()
                .any(|artifact| artifact.artifact_id == "jakarta.annotation-api"),
            "{lock:?}"
        );
    }

    #[test]
    fn newer_schema_versions_are_refused() {
        let raw = SAMPLE.replace("version = 1", "version = 4");
        let error = parse(&raw).expect_err("newer schema");
        assert!(error.message.contains("versions 1 through 3"), "{error}");
        assert!(
            error
                .hint
                .as_deref()
                .is_some_and(|hint| hint.contains("newer jx")),
            "{error}"
        );
    }

    #[test]
    fn version_three_app_and_processor_flags_round_trip() {
        let raw = "version = 3\n\
\n\
[[artifact]]\n\
 group = \"g\"\n\
 artifact = \"main\"\n\
 version = \"1\"\n\
 sha256 = \"1111111111111111111111111111111111111111111111111111111111111111\"\n\
\n\
[[artifact]]\n\
 group = \"g\"\n\
 artifact = \"processor-only\"\n\
 version = \"2\"\n\
 app = false\n\
 processor = true\n\
 sha256 = \"2222222222222222222222222222222222222222222222222222222222222222\"\n\
\n\
[[artifact]]\n\
 group = \"g\"\n\
 artifact = \"shared\"\n\
 version = \"3\"\n\
 processor = true\n\
 sha256 = \"3333333333333333333333333333333333333333333333333333333333333333\"\n";
        let lock = parse(raw).expect("parsed v3");
        assert_eq!(lock.version, 3);
        let main = lock.find("g", "main", None).expect("main");
        assert!(main.app && !main.processor, "v3 defaults are app-only");
        let processor = lock.find("g", "processor-only", None).expect("processor");
        assert!(!processor.app && processor.processor);
        let shared = lock.find("g", "shared", None).expect("shared");
        assert!(shared.app && shared.processor);
        let text = to_canonical_text(&lock);
        assert!(text.contains("app = false"), "{text}");
        assert!(text.contains("processor = true"), "{text}");
        // A default entry serializes neither flag.
        let main_block = text
            .split("[[artifact]]")
            .find(|block| block.contains("artifact = \"main\""))
            .expect("main block");
        assert!(!main_block.contains("app ="), "{text}");
        assert!(!main_block.contains("processor ="), "{text}");
        assert_eq!(parse(&text).expect("reparsed"), lock);
    }

    #[test]
    fn version_two_dev_flags_round_trip() {
        let raw = "version = 2\n\
\n\
[[artifact]]\n\
 group = \"g\"\n\
 artifact = \"main\"\n\
 version = \"1\"\n\
 sha256 = \"1111111111111111111111111111111111111111111111111111111111111111\"\n\
\n\
[[artifact]]\n\
 group = \"g\"\n\
 artifact = \"test-only\"\n\
 version = \"2\"\n\
 dev = true\n\
 sha256 = \"2222222222222222222222222222222222222222222222222222222222222222\"\n";
        let lock = parse(raw).expect("parsed v2");
        assert_eq!(lock.version, 2);
        assert!(lock.find("g", "main", None).is_some_and(|a| !a.dev));
        assert!(lock.find("g", "test-only", None).is_some_and(|a| a.dev));
        let text = to_canonical_text(&lock);
        assert!(text.contains("dev = true"), "{text}");
        assert_eq!(parse(&text).expect("reparsed"), lock);
    }

    #[test]
    fn save_is_atomic_idempotent_and_detects_change() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("jx.lock");
        let lock = parse(SAMPLE).expect("parsed");
        assert!(save(&path, &lock).expect("first save"));
        assert!(!save(&path, &lock).expect("second save is a no-op"));
        assert_eq!(read(&path).expect("readable"), lock);

        let mut changed = lock.clone();
        changed.artifacts[0].version = "1.5".to_string();
        assert!(save(&path, &changed).expect("changed save"));
        assert!(!save(&path, &changed).expect("stable again"));
        read(&path).expect("still readable");
    }

    #[test]
    fn read_missing_lockfile_is_a_clear_error() {
        let dir = tempdir().unwrap();
        let error = read(&dir.path().join("jx.lock")).expect_err("absent file");
        assert!(error.message.contains("cannot read jx.lock"), "{error}");
        assert!(
            error
                .hint
                .as_deref()
                .is_some_and(|hint| hint.contains("jx resolve")),
            "{error}"
        );
    }
}
