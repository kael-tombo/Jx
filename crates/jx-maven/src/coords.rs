//! Maven coordinates (`group:artifact:version[:classifier[:extension]]`) and
//! the remote-repository file paths they map to.
//!
//! Layout verified against <https://maven.apache.org/repositories/layout.html>
//! and recorded in `docs/decisions/0003-repository-client-and-pom-model.md`:
//! `groupId` maps to a path by replacing each `.` with `/`; artifact id,
//! version, classifier and extension are used verbatim in
//! `{group}/{artifact}/{version}/{artifact}-{version}[-{classifier}].{ext}`.
//! The POM is always `{artifact}-{version}.pom` — no classifier.

use crate::xml::SnapshotInfo;
use jx_common::error::{JxError, Kind};

/// Characters allowed in group/artifact/classifier identifiers (also the
/// path-safety bound: every character is filesystem- and URL-safe).
const IDENT_CHARS: fn(char) -> bool =
    |c: char| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-';
/// Extension of a repository artifact: page for versions is any of the above
/// plus `+` (semver build metadata like `1.0.0+build.1` exists on Central).
const VERSION_CHARS: fn(char) -> bool =
    |c: char| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-' || c == '+';

/// A snapshot version's trailing marker (case-insensitive at the call site).
const SNAPSHOT_SUFFIX_LOWER: &str = "-snapshot";
/// A snapshot version's canonical length, used for suffix removal.
const SNAPSHOT_SUFFIX_LEN: usize = 9;

/// One Maven artifact identity.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Coordinate {
    pub group_id: String,
    pub artifact_id: String,
    /// The `project.version` string as written (`1.2.3`, or `1.2.3-SNAPSHOT`).
    pub version: String,
    pub classifier: Option<String>,
    /// File extension of the artifact itself (`jar` default; `pom`, `war`, …).
    pub extension: String,
}

impl Coordinate {
    /// Parse `group:artifact:version[:classifier[:extension]]`. Extension
    /// defaults to `jar`; passing an extension without a classifier is
    /// unambiguous because the 4th field is always the classifier.
    pub fn parse(text: &str) -> Result<Coordinate, JxError> {
        let parts: Vec<&str> = text.split(':').collect();
        if !(3..=5).contains(&parts.len()) {
            return Err(
                JxError::new(Kind::Resolve, format!("not a Maven coordinate: {text}"))
                    .hint("the shape is `group:artifact:version[:classifier[:extension]]`"),
            );
        }
        // An explicitly present field must be non-empty: `a:b:1.0::` is
        // malformed, not "classifier and extension both empty".
        let classifier = match parts.get(3) {
            None => None,
            Some(c) => Some(validate_ident(c, "classifier")?),
        };
        let extension = match parts.get(4) {
            None => "jar".to_string(),
            Some(ext) => {
                let ext = ext.to_ascii_lowercase();
                if ext.is_empty() || !ext.chars().all(|c| c.is_ascii_alphanumeric()) {
                    return Err(
                        JxError::new(Kind::Resolve, format!("invalid extension {ext:?}"))
                            .hint("extensions are letters/digits (`jar`, `pom`, `aar`, …)"),
                    );
                }
                ext
            }
        };
        Ok(Coordinate {
            group_id: validate_ident(parts[0], "groupId")?,
            artifact_id: validate_ident(parts[1], "artifactId")?,
            version: validate_version(parts[2])?,
            classifier,
            extension,
        })
    }

    /// Whether the version string marks a snapshot (`-SNAPSHOT` suffix,
    /// case-insensitively per Maven's own `Artifact.isSnapshot`).
    pub fn is_snapshot(&self) -> bool {
        self.version
            .to_ascii_lowercase()
            .ends_with(SNAPSHOT_SUFFIX_LOWER)
    }

    /// The version with any trailing `SNAPSHOT` marker removed — the folder
    /// level for snapshot metadata (`com/google/guava/guava/1.0/`) and the
    /// base part of timestamped snapshot file names.
    pub fn base_version(&self) -> &str {
        if self.is_snapshot() {
            &self.version[..self.version.len() - SNAPSHOT_SUFFIX_LEN]
        } else {
            &self.version
        }
    }

    /// Directory path for this artifact's version, `com/a/b/1.0`(snapshot
    /// folders keep the `-SNAPSHOT` suffix as written).
    pub fn version_directory(&self) -> String {
        format!(
            "{}/{}/{}",
            self.group_id.replace('.', "/"),
            self.artifact_id,
            self.version
        )
    }

    /// Metadata directory: `{group}/{artifact}/{baseVersion}` for snapshot
    /// metadata, `{group}/{artifact}` for the version list.
    pub fn metadata_directory(&self, for_snapshot: bool) -> String {
        if for_snapshot {
            self.version_directory()
        } else {
            format!("{}/{}", self.group_id.replace('.', "/"), self.artifact_id)
        }
    }

    /// Release/NN-classified artifact file name inside the version directory.
    /// `None` path part is only produced through other constructors.
    pub fn artifact_file_name(&self) -> String {
        let base = format!("{}-{}", self.artifact_id, self.version);
        match &self.classifier {
            Some(c) => format!("{base}-{c}.{}", self.extension),
            None => format!("{base}.{}", self.extension),
        }
    }

    /// The POM file name: always classifier-less.
    pub fn pom_file_name(&self) -> String {
        format!("{}-{}.pom", self.artifact_id, self.version)
    }

    /// Repository-relative path of the artifact file (non-snapshot form).
    pub fn artifact_path(&self) -> String {
        format!("{}/{}", self.version_directory(), self.artifact_file_name())
    }

    /// Repository-relative path of the POM (non-snapshot form).
    pub fn pom_path(&self) -> String {
        format!("{}/{}", self.version_directory(), self.pom_file_name())
    }

    /// Repository-relative path of the version-level snapshot metadata
    /// (`maven-metadata.xml` inside the version directory), only for
    /// snapshots.
    pub fn snapshot_metadata_path(&self) -> String {
        format!("{}/maven-metadata.xml", self.version_directory())
    }

    /// The `key` Maven prints: `group:artifact:version[:classifier]` — used
    /// in lockfiles and diagnostics.
    pub fn key(&self) -> String {
        match &self.classifier {
            Some(c) => format!(
                "{}:{}:{}:{c}",
                self.group_id, self.artifact_id, self.version
            ),
            None => format!("{}:{}:{}", self.group_id, self.artifact_id, self.version),
        }
    }

    /// Same coordinates with a different version string: how a `1.0-SNAPSHOT`
    /// coordinate is lifted to its concrete timestamped form.
    pub fn with_version(&self, version: impl Into<String>) -> Coordinate {
        let mut with = self.clone();
        with.version = version.into();
        with
    }

    /// The concrete version this snapshot resolves to, from the
    /// `<snapshot>` block of its version-level `maven-metadata.xml`:
    /// `1.0-SNAPSHOT` → `1.0-20250101.070011-12`. Fails when the metadata
    /// publisher wrote no timestamp/build number.
    pub fn resolved_snapshot_version(&self, snapshot: &SnapshotInfo) -> Result<String, JxError> {
        if !self.is_snapshot() {
            return Err(JxError::new(
                Kind::Resolve,
                format!("{} is not a snapshot coordinate", self.key()),
            ));
        }
        if snapshot.timestamp.is_empty() || snapshot.build_number.is_empty() {
            return Err(
                JxError::new(
                    Kind::Resolve,
                    format!("snapshot metadata for {} has no timestamp/build number", self.key()),
                )
                .hint("the repository published an incomplete snapshot; check the CI job that deployed it"),
            );
        }
        Ok(format!(
            "{}-{}-{}",
            self.base_version(),
            snapshot.timestamp,
            snapshot.build_number
        ))
    }
}

fn validate_ident(part: &str, what: &str) -> Result<String, JxError> {
    if part.is_empty() {
        return Err(JxError::new(
            Kind::Resolve,
            format!("empty ({what}) in a Maven coordinate"),
        )
        .hint("the shape is `group:artifact:version[:classifier[:extension]]` "));
    }
    if !part.chars().all(IDENT_CHARS) {
        return Err(JxError::new(
            Kind::Resolve,
            format!("invalid character(s) in {what}: {part:?}"),
        )
        .hint(format!(
            "{what} may contain letters, digits and `.`, `_`, `-` only"
        )));
    }
    Ok(part.to_string())
}

fn validate_version(part: &str) -> Result<String, JxError> {
    if part.is_empty() {
        return Err(
            JxError::new(Kind::Resolve, "empty version in a Maven coordinate")
                .hint("the shape is `group:artifact:version[:classifier[:extension]]` "),
        );
    }
    if !part.chars().all(VERSION_CHARS) {
        return Err(JxError::new(
            Kind::Resolve,
            format!("invalid version: {part:?}"),
        )
        .hint("versions are repository path segments: letters, digits and `.`, `_`, `-`, `+` only"));
    }
    Ok(part.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_coordinate() {
        let c = Coordinate::parse("com.google.guava:guava:23.0-jre:android:jar").unwrap();
        assert_eq!(c.group_id, "com.google.guava");
        assert_eq!(c.classifier.as_deref(), Some("android"));
        assert_eq!(c.extension, "jar");
        assert!(!c.is_snapshot());
        assert_eq!(c.key(), "com.google.guava:guava:23.0-jre:android");
    }

    #[test]
    fn minimal_coordinate_defaults() {
        let c = Coordinate::parse("org.apache.commons:commons-lang3:3.14.0").unwrap();
        assert_eq!(c.classifier, None);
        assert_eq!(c.extension, "jar");
        assert_eq!(c.artifact_file_name(), "commons-lang3-3.14.0.jar");
        assert_eq!(c.pom_file_name(), "commons-lang3-3.14.0.pom");
        assert_eq!(
            c.artifact_path(),
            "org/apache/commons/commons-lang3/3.14.0/commons-lang3-3.14.0.jar"
        );
        assert_eq!(
            c.pom_path(),
            "org/apache/commons/commons-lang3/3.14.0/commons-lang3-3.14.0.pom"
        );
    }

    #[test]
    fn classifier_without_extension_is_fourth_field() {
        let c = Coordinate::parse("a:b:1.0:tests").unwrap();
        assert_eq!(c.classifier.as_deref(), Some("tests"));
        assert_eq!(c.extension, "jar");
        assert_eq!(c.artifact_file_name(), "b-1.0-tests.jar");
    }

    #[test]
    fn snapshot_shape() {
        let c = Coordinate::parse("org.k:t:1.0-SNAPSHOT").unwrap();
        assert!(c.is_snapshot());
        // Record the SNAPSHOT marker: metadata lives in a folder per version.
        assert_eq!(c.version_directory(), "org/k/t/1.0-SNAPSHOT");
        assert_eq!(c.base_version(), "1.0");
        assert_eq!(
            c.snapshot_metadata_path(),
            "org/k/t/1.0-SNAPSHOT/maven-metadata.xml"
        );
    }

    #[test]
    fn lowercase_snapshot_marker() {
        let c = Coordinate::parse("org.k:t:2.0-snapshot").unwrap();
        assert!(c.is_snapshot());
        assert_eq!(c.base_version(), "2.0");
    }

    #[test]
    fn rejects_wrong_shapes() {
        for bad in [
            "a:b",
            "a:b:c:d:e:f",
            ":a:1.0",
            "a::1.0",
            "a:b:",
            "a:b:1.0::",
        ] {
            assert!(
                Coordinate::parse(bad).is_err(),
                "expected {bad} to be rejected"
            );
        }
    }

    #[test]
    fn rejects_path_unsafe_identifiers() {
        // The coordinate ends up as a repository path segment; keep segments
        // safely tokenized.
        for bad in ["a:b:1../", "a:b:c/d", "a:b:1.0:tests/jar", "a:b:1.0:\\"] {
            assert!(
                Coordinate::parse(bad).is_err(),
                "expected {bad} to be rejected"
            );
        }
        // Rare but legal: semver build metadata.
        assert!(
            Coordinate::parse("a:b:1.0.0+build.1").is_ok(),
            "plus-signed versions are real and allowed"
        );
    }

    #[test]
    fn metadata_directory_versions() {
        let c = Coordinate::parse("g:a:1.0").unwrap();
        assert_eq!(c.metadata_directory(false), "g/a");
        assert_eq!(c.metadata_directory(true), "g/a/1.0");
    }

    #[test]
    fn snapshot_resolution_lifts_to_the_timestamped_version() {
        let c = Coordinate::parse("g:a:1.0-SNAPSHOT").unwrap();
        let info = SnapshotInfo {
            timestamp: "20250101.070011".to_string(),
            build_number: "12".to_string(),
        };
        let resolved = c.resolved_snapshot_version(&info).unwrap();
        assert_eq!(resolved, "1.0-20250101.070011-12");
        let concrete = c.with_version(&resolved);
        assert!(!concrete.is_snapshot());
        assert_eq!(
            concrete.artifact_file_name(),
            "a-1.0-20250101.070011-12.jar"
        );
    }

    #[test]
    fn snapshot_resolution_fails_without_timestamp() {
        let c = Coordinate::parse("g:a:1.0-SNAPSHOT").unwrap();
        let info = SnapshotInfo {
            timestamp: String::new(),
            build_number: "3".to_string(),
        };
        let e = c.resolved_snapshot_version(&info).unwrap_err();
        assert!(e.message.contains("no timestamp/build number"), "{e}");
    }

    #[test]
    fn snapshot_resolution_rejects_releases() {
        let c = Coordinate::parse("g:a:1.0").unwrap();
        let info = SnapshotInfo {
            timestamp: "20250101.070011".to_string(),
            build_number: "12".to_string(),
        };
        assert!(c.resolved_snapshot_version(&info).is_err());
    }
}
