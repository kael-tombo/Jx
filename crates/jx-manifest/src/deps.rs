//! The dependency tables of project.toml (`[dependencies]` and
//! `[dev-dependencies]`): alias → coordinate strings and platform entries.
//!
//! Shape and rules per `docs/manifest.md`:
//!
//! ```toml
//! guava = "com.google.guava:guava:33"                  # or full `33.0.0`
//! slf4j = "org.slf4j:slf4j-api:=2.0.16"                 # `=` pins exactly
//! jackson-bom = { platform = "com.fasterxml.jackson:jackson-bom:2.18.0" }
//! jackson-databind = "com.fasterxml.jackson.core:jackson-databind" # from BOM
//! junit = "org.junit.jupiter:junit-jupiter:5"           # (dev-dependencies)
//! ```
//!
//! Validated here: the coordinate shape, version-charset path safety, exact
//! SNAPSHOT rejection, and the `=`/prefix split. What a *wanted* version
//! means (prefix compatibility, pre-release exclusion) is selection policy —
//! the resolver applies it in M3 with `ComparableVersion`.

use std::path::Path;

use jx_common::error::{JxError, Kind};

use crate::unknown_key_error;

/// A dependency as written in project.toml, validated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DependencySpec {
    /// The local alias key (`guava` above), as written.
    pub alias: String,
    pub group_id: String,
    pub artifact_id: String,
    pub classifier: Option<String>,
    /// What the manifest wants: a compatibility prefix or an exact pin.
    /// `None` means "no version here — a `platform` dependency supplies it".
    pub wanted: Option<WantedVersion>,
    /// `true` for `{ platform = "…" }` entries: a BOM under this alias.
    pub platform: bool,
}

/// The version as requested. The resolver expands a [`WantedVersion::Prefix`]
/// to the highest stable release with that prefix (pre-releases excluded
/// unless the prefix is an exact pin), and [`WantedVersion::Exact`] pins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WantedVersion {
    /// `N`, `N.M`, `N.M.P` and friends: "highest release with this prefix".
    Prefix(String),
    /// A leading `=`: exactly this version, prefix-matching disabled.
    Exact(String),
}

impl WantedVersion {
    /// The version text without the `=` marker.
    pub fn as_str(&self) -> &str {
        match self {
            WantedVersion::Prefix(v) | WantedVersion::Exact(v) => v,
        }
    }

    pub fn is_exact(&self) -> bool {
        matches!(self, WantedVersion::Exact(_))
    }
}

/// Characters a version may contain in a manifest (same set the repository
/// client accepts for path segments; keeps aliases out of URL surprises).
const VERSION_CHARS: fn(char) -> bool =
    |c: char| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-' || c == '+';
/// Characters a groupId/artifactId/classifier may contain.
const IDENT_CHARS: fn(char) -> bool =
    |c: char| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-';

/// Parse both dependency tables. Returns `(dependencies, dev_dependencies)`.
pub fn parse_tables(
    root: &toml::Table,
    path: &Path,
) -> Result<(Vec<DependencySpec>, Vec<DependencySpec>), JxError> {
    let deps = parse_table("dependencies", root, path)?;
    let dev_deps = parse_table("dev-dependencies", root, path)?;
    Ok((deps, dev_deps))
}

/// Parse `[processors]`: annotation-processor jars and their transitive
/// dependencies. The table shares the dependency grammar, but every entry
/// must name a concrete version: a processor path cannot be filled from a
/// BOM in another table, and `{ platform = … }` would put a POM where javac
/// expects a jar (ADR 0014).
pub fn parse_processors(root: &toml::Table, path: &Path) -> Result<Vec<DependencySpec>, JxError> {
    let specs = parse_table("processors", root, path)?;
    for spec in &specs {
        if spec.platform {
            return Err(JxError::new(
                Kind::Manifest,
                format!(
                    "`{}` in [processors] is a platform entry",
                    spec.alias
                ),
            )
            .at(path.display().to_string())
            .hint("annotation processors are jars: give each one a version, e.g. `org.mapstruct:mapstruct-processor:1.6.3`"));
        }
        if spec.wanted.is_none() {
            return Err(JxError::new(
                Kind::Manifest,
                format!(
                    "`{}` in [processors] declares no version",
                    spec.alias
                ),
            )
            .at(path.display().to_string())
            .hint("a processor needs an explicit version; BOM-supplied versions are not resolved for [processors]"));
        }
    }
    Ok(specs)
}

fn parse_table(
    name: &str,
    root: &toml::Table,
    path: &Path,
) -> Result<Vec<DependencySpec>, JxError> {
    let raw = match root.get(name) {
        None => return Ok(Vec::new()),
        Some(value) => value,
    };
    let table = raw.as_table().ok_or_else(|| {
        JxError::new(Kind::Manifest, format!("[{name}] must be a table"))
            .at(path.display().to_string())
    })?;
    let mut specs = Vec::with_capacity(table.len());
    for (alias, value) in table {
        let spec = parse_entry(name, alias, value, path)?;
        let duplicate = specs.iter().any(|d: &DependencySpec| {
            d.group_id == spec.group_id && d.artifact_id == spec.artifact_id
        });
        if duplicate {
            return Err(JxError::new(
                Kind::Manifest,
                format!(
                    "`{alias}` in [{name}] names {}:{} but an earlier alias already declares it",
                    spec.group_id, spec.artifact_id
                ),
            )
            .at(path.display().to_string())
            .hint(
                "one alias per group:artifact (with the same classifier); delete the duplicate",
            ));
        }
        specs.push(spec);
    }
    Ok(specs)
}

fn parse_entry(
    table: &str,
    alias: &str,
    value: &toml::Value,
    path: &Path,
) -> Result<DependencySpec, JxError> {
    let where_ =
        |what: &str| JxError::new(Kind::Manifest, format!("`{what}` in [{table}] `{alias}`"));
    // `{ platform = "g:a:v" }` marks the BOM itself.
    if let Some(entry) = value.as_table() {
        if let Some(platform_raw) = entry.get("platform") {
            for key in entry.keys() {
                if key != "platform" {
                    return Err(unknown_key_error(table, key, &["platform"]));
                }
            }
            let platform = platform_raw
                .as_str()
                .ok_or_else(|| where_("platform").hint("`platform = \"group:artifact:version\""))?
                .to_owned();
            let (group_id, artifact_id, wanted, classifier) = parse_coordinate(&platform)?;
            if classifier.is_some() {
                return Err(JxError::new(
                    Kind::Manifest,
                    format!("`{alias}` in [{table}] sets a classifier, but platform dependencies are whole BOMs"),
                )
                .at(path.display().to_string()));
            }
            let wanted = wanted.ok_or_else(|| {
                JxError::new(
                    Kind::Manifest,
                    format!("the platform `{alias}` in [{table}] has no version"),
                )
                .at(path.display().to_string())
                .hint("a BOM needs a version to import: `…:group:artifact:version`")
            })?;
            return Ok(DependencySpec {
                alias: alias.to_string(),
                group_id,
                artifact_id,
                classifier: None,
                wanted: Some(wanted),
                platform: true,
            });
        }
        // A table without `platform` is only unknown keys.
        let found = entry.keys().next().cloned().unwrap_or_default();
        return Err(unknown_key_error(table, &found, &["platform"]));
    }
    // The string form.
    let raw = value
        .as_str()
        .ok_or_else(|| where_("value").hint("`group:artifact[:version][:classifier]`"))?;
    let (group_id, artifact_id, wanted, classifier) = parse_coordinate(raw)?;
    Ok(DependencySpec {
        alias: alias.to_string(),
        group_id,
        artifact_id,
        classifier,
        wanted,
        platform: false,
    })
}

/// Parse one dependency exactly as a user would type it at `jx add`:
/// `group:artifact[:version][:classifier]`, or a BOM with `platform = true`.
/// The alias is not validated here (that is the caller's naming choice);
/// every coordinate rule is.
pub fn parse_user_spec(alias: &str, raw: &str, platform: bool) -> Result<DependencySpec, JxError> {
    let (group_id, artifact_id, wanted, classifier) = parse_coordinate(raw)?;
    if platform {
        if classifier.is_some() {
            return Err(JxError::new(
                Kind::Manifest,
                format!("`{alias}` sets a classifier, but platform dependencies are whole BOMs"),
            )
            .hint("platform entries import version management; they name no classified file"));
        }
        if wanted.is_none() {
            return Err(JxError::new(
                Kind::Manifest,
                format!("the platform `{alias}` has no version"),
            )
            .hint("a BOM needs a version to import: `…:group:artifact:version`"));
        }
    }
    Ok(DependencySpec {
        alias: alias.to_string(),
        group_id,
        artifact_id,
        classifier,
        wanted,
        platform,
    })
}

/// `group:artifact[:version][:classifier]` → parts. The version is wanted
/// (prefix or exact); the extension is not part of V1's manifest grammar.
fn parse_coordinate(
    raw: &str,
) -> Result<(String, String, Option<WantedVersion>, Option<String>), JxError> {
    let parts: Vec<&str> = raw.split(':').collect();
    if !(2..=4).contains(&parts.len()) {
        return Err(
            JxError::new(Kind::Manifest, format!("not a Maven coordinate: {raw}"))
                .hint("`group:artifact[:version][:classifier]`, e.g. `com.google.guava:guava:33`"),
        );
    }
    let group_id = ident(parts[0], "groupId", raw)?;
    let artifact_id = ident(parts[1], "artifactId", raw)?;
    let wanted = match parts.get(2) {
        None => None,
        Some(version) => Some(wanted(version, raw)?),
    };
    let classifier = match parts.get(3) {
        None => None,
        Some(c) => {
            if c.is_empty() {
                return Err(malformed(raw, "the classifier is empty"));
            }
            if !c.chars().all(IDENT_CHARS) {
                return Err(malformed(
                    raw,
                    &format!(
                        "the classifier {c:?} may contain letters, digits and `.`, `_`, `-` only"
                    ),
                ));
            }
            Some(c.to_string())
        }
    };
    Ok((group_id, artifact_id, wanted, classifier))
}

/// Prefix or exact (`=`); always rejects SNAPSHOTs, which Maven would
/// re-resolve on every build.
fn wanted(text: &str, raw: &str) -> Result<WantedVersion, JxError> {
    if let Some(rest) = text.strip_prefix('=') {
        let rest = rest.trim();
        if rest.is_empty() {
            return Err(malformed(raw, "the `=` pin has no version after it"));
        }
        check_snapshot_and_shape(rest, raw)?;
        return Ok(WantedVersion::Exact(rest.to_string()));
    }
    check_snapshot_and_shape(text, raw)?;
    Ok(WantedVersion::Prefix(text.to_string()))
}

fn malformed(raw: &str, what: &str) -> JxError {
    JxError::new(Kind::Manifest, format!("bad dependency coordinate {raw:?}: {what}"))
        .hint("`group:artifact[:version][:classifier]`; versions are `N`, `N.M`, `N.M.P` (highest matching release) or `=N.M.P` (pinned)")
}

fn check_snapshot_and_shape(text: &str, raw: &str) -> Result<(), JxError> {
    if text.is_empty() {
        return Err(malformed(raw, "the version is empty"));
    }
    if text.to_ascii_lowercase().ends_with("-snapshot") {
        return Err(JxError::new(
            Kind::Manifest,
            format!(
                "dependency {} uses a SNAPSHOT version",
                raw.split(':').take(2).collect::<Vec<_>>().join(":")
            ),
        )
        .hint(
            "jx resolves releases only: SNAPSHOTs re-resolve on every build; \
             pin a release version instead",
        ));
    }
    if !text.chars().all(VERSION_CHARS) {
        return Err(malformed(
            raw,
            "the version may contain letters, digits and `.`, `_`, `-`, `+` only",
        ));
    }
    Ok(())
}

fn ident(part: &str, what: &str, raw: &str) -> Result<String, JxError> {
    if part.is_empty() {
        return Err(malformed(raw, &format!("the {what} is empty")));
    }
    if !part.chars().all(IDENT_CHARS) {
        return Err(malformed(
            raw,
            &format!("the {what} {part:?} may contain letters, digits and `.`, `_`, `-` only"),
        ));
    }
    Ok(part.to_string())
}

/// One `[overrides]` entry: an explicit version applied to every occurrence
/// of that artifact in the dependency graph (ADR 0004: overrides win
/// everywhere and are always visible in the resolution report).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverrideSpec {
    pub group_id: String,
    pub artifact_id: String,
    /// The version as written. Override values always *pin*: the resolver
    /// treats the text as an exact version whether or not it carries `=`.
    pub wanted: WantedVersion,
}

/// Parse the `[overrides]` table: quoted `group:artifact` keys, version
/// values. SNAPSHOT and charset rules match the dependency tables.
pub fn parse_overrides(root: &toml::Table, path: &Path) -> Result<Vec<OverrideSpec>, JxError> {
    let raw = match root.get("overrides") {
        None => return Ok(Vec::new()),
        Some(value) => value,
    };
    let table = raw.as_table().ok_or_else(|| {
        JxError::new(Kind::Manifest, "[overrides] must be a table").at(path.display().to_string())
    })?;
    let mut overrides = Vec::with_capacity(table.len());
    for (key, value) in table {
        let version = value.as_str().ok_or_else(|| {
            JxError::new(
                Kind::Manifest,
                format!("`{key}` in [overrides] must be a version string"),
            )
            .at(path.display().to_string())
            .hint("overrides look like `\"org.slf4j:slf4j-api\" = \"2.0.16\"`")
        })?;
        let parts: Vec<&str> = key.split(':').collect();
        if parts.len() != 2 || parts.iter().any(|p| p.is_empty()) {
            return Err(JxError::new(
                Kind::Manifest,
                format!("the override key {key:?} is not `group:artifact`"),
            )
            .at(path.display().to_string())
            .hint("a whole artifact is overridden, not a classified file of it"));
        }
        let group_id = ident(parts[0], "groupId", key)?;
        let artifact_id = ident(parts[1], "artifactId", key)?;
        let wanted = wanted(version, key)?;
        if overrides
            .iter()
            .any(|o: &OverrideSpec| o.group_id == group_id && o.artifact_id == artifact_id)
        {
            return Err(JxError::new(
                Kind::Manifest,
                format!("{group_id}:{artifact_id} is overridden twice in [overrides]"),
            )
            .at(path.display().to_string())
            .hint("one entry per artifact — merge them into a single version"));
        }
        overrides.push(OverrideSpec {
            group_id,
            artifact_id,
            wanted,
        });
    }
    Ok(overrides)
}

#[cfg(test)]
mod tests {
    use super::*;
    use jx_common::error::Kind;
    use std::path::PathBuf;

    fn parse(toml_text: &str) -> Result<(Vec<DependencySpec>, Vec<DependencySpec>), JxError> {
        let value: toml::Table = toml::from_str(toml_text).expect("test TOML");
        parse_tables(&value, &PathBuf::from("project.toml"))
    }

    #[test]
    fn parses_string_entries_with_prefix_versions() {
        let (deps, dev) = parse(
            r#"
[dependencies]
guava = "com.google.guava:guava:33"
slf4j = "org.slf4j:slf4j-api:=2.0.16"
bare = "g:a"

[dev-dependencies]
junit = "org.junit.jupiter:junit-jupiter:5"
"#,
        )
        .expect("parsed");
        assert_eq!(deps.len(), 3);
        // TOML tables are ordered maps, not insertion-ordered: look up by
        // alias rather than by position.
        let by_alias = |alias: &str| {
            deps.iter()
                .find(|dep| dep.alias == alias)
                .unwrap_or_else(|| panic!("alias {alias} parsed"))
        };
        let guava = by_alias("guava");
        assert_eq!(guava.group_id, "com.google.guava");
        assert_eq!(guava.artifact_id, "guava");
        assert_eq!(
            guava.wanted.as_ref(),
            Some(&WantedVersion::Prefix("33".into()))
        );
        assert!(!guava.platform);
        assert_eq!(
            by_alias("slf4j").wanted.as_ref(),
            Some(&WantedVersion::Exact("2.0.16".into()))
        );
        assert!(
            by_alias("bare").wanted.is_none(),
            "version comes from a platform"
        );
        assert_eq!(dev.len(), 1);
    }

    #[test]
    fn platform_entries_mark_the_bom() {
        let (deps, _) = parse(
            r#"
[dependencies]
jackson-bom = { platform = "com.fasterxml.jackson:jackson-bom:2.18.0" }
"#,
        )
        .expect("parsed");
        assert_eq!(deps.len(), 1);
        assert!(deps[0].platform);
        assert_eq!(
            deps[0].wanted.as_ref().map(WantedVersion::as_str),
            Some("2.18.0")
        );
        assert_eq!(deps[0].classifier, None);
    }

    #[test]
    fn classifiers_parse_in_the_string_form() {
        let (deps, _) = parse(
            r#"
[dependencies]
agent = "dev.dirs:dirs:1:tests"
"#,
        )
        .expect("parsed");
        assert_eq!(deps[0].classifier.as_deref(), Some("tests"));
    }

    #[test]
    fn snapshots_are_rejected_in_both_forms() {
        for raw in [
            r#"
[dependencies]
bad = "g:a:1.0-SNAPSHOT"
"#,
            r#"
[dependencies]
bad = "g:a:=1.0-SNAPSHOT"
"#,
            r#"
[dependencies]
bad = { platform = "g:a:2.0-snapshot" }
"#,
        ] {
            let error = parse(raw).expect_err("snapshots are not resolvable");
            assert!(error.message.contains("SNAPSHOT"), "{error}");
        }
    }

    #[test]
    fn malformed_coordinates_are_named() {
        for raw in [
            "[dependencies]\nbad = \"onlyone\"\n",
            "[dependencies]\nbad = \"g:a:1:a:j\"\n",
            "[dependencies]\nbad = \"g::1\"\n",
            "[dependencies]\nbad = \"g:a:=\"\n",
            "[dependencies]\nbad = \"g:a:1.0/../etc\"\n",
        ] {
            let error = parse(raw).expect_err("malformed coordinate");
            assert!(error.kind == Kind::Manifest, "{error}");
        }
    }

    #[test]
    fn platform_tables_reject_unknown_keys_and_classifiers() {
        let error = parse("[dependencies]\nbad = { platform = \"g:a:1\", classifier = \"x\" }\n")
            .expect_err("unknown key");
        assert!(
            error.message.contains("unknown key `classifier`"),
            "{error}"
        );
        assert!(
            error.message.contains("did you mean `platform`?"),
            "{error}"
        );

        let xerr = parse("[dependencies]\nbad = { platform = \"g:a:1:tests\" }\n")
            .expect_err("classifier on a BOM");
        assert!(xerr.message.contains("classifier"), "{xerr}");
    }

    #[test]
    fn versionless_platform_entry_is_an_error() {
        let error =
            parse("[dependencies]\npom = { platform = \"g:a\" }\n").expect_err("no version");
        assert!(error.message.contains("has no version"), "{error}");
    }

    #[test]
    fn duplicate_coordinates_are_rejected_within_a_table() {
        let error = parse("[dependencies]\none = \"g:a:1\"\ntwo = \"g:a:2\"\n")
            .expect_err("same coordinate twice");
        assert!(error.message.contains("`two` in [dependencies]"), "{error}");
    }

    #[test]
    fn duplicated_across_tables_is_fine() {
        let (_, dev) =
            parse("[dependencies]\none = \"g:a:1\"\n\n[dev-dependencies]\none = \"g:a:5\"\n")
                .expect("two tables are separate");
        assert_eq!(dev.len(), 1);
    }

    fn processors_of(toml_text: &str) -> Result<Vec<DependencySpec>, JxError> {
        let value: toml::Table = toml::from_str(toml_text).expect("test TOML");
        parse_processors(&value, &PathBuf::from("project.toml"))
    }

    #[test]
    fn processors_parse_like_dependencies() {
        let specs = processors_of(
            r#"
[processors]
mapstruct = "org.mapstruct:mapstruct-processor:1.6.3"
lombok = "org.projectlombok:lombok:=1.18.36"
"#,
        )
        .expect("parsed");
        assert_eq!(specs.len(), 2);
        let mapstruct = specs
            .iter()
            .find(|spec| spec.alias == "mapstruct")
            .expect("mapstruct parsed");
        assert_eq!(mapstruct.group_id, "org.mapstruct");
        assert_eq!(
            mapstruct.wanted.as_ref(),
            Some(&WantedVersion::Prefix("1.6.3".into()))
        );
        assert!(!mapstruct.platform);
    }

    #[test]
    fn processors_need_an_explicit_version() {
        let error =
            processors_of("[processors]\nmapstruct = \"org.mapstruct:mapstruct-processor\"\n")
                .expect_err("no version");
        assert!(error.message.contains("declares no version"), "{error}");
        assert!(
            error
                .hint
                .as_deref()
                .is_some_and(|hint| hint.contains("explicit version")),
            "{error}"
        );
    }

    #[test]
    fn processors_reject_platform_entries() {
        let error = processors_of(
            "[processors]\nbom = { platform = \"org.mapstruct:mapstruct-bom:1.6.3\" }\n",
        )
        .expect_err("a BOM is not a processor jar");
        assert!(error.message.contains("is a platform entry"), "{error}");
        assert!(
            error
                .hint
                .as_deref()
                .is_some_and(|hint| hint.contains("jars")),
            "{error}"
        );
    }

    #[test]
    fn processors_reject_duplicate_coordinates() {
        let error = processors_of(
            "[processors]\none = \"org.mapstruct:mapstruct-processor:1.6.3\"\ntwo = \"org.mapstruct:mapstruct-processor:1.6.4\"\n",
        )
        .expect_err("same coordinate twice");
        assert!(error.message.contains("`two` in [processors]"), "{error}");
    }

    fn overrides_of(toml_text: &str) -> Result<Vec<OverrideSpec>, JxError> {
        let value: toml::Table = toml::from_str(toml_text).expect("test TOML");
        parse_overrides(&value, &PathBuf::from("project.toml"))
    }

    #[test]
    fn overrides_parse_group_artifact_keys_with_versions() {
        let specs = overrides_of(
            r#"
[overrides]
"org.slf4j:slf4j-api" = "2.0.16"
"com.google.guava:guava" = "=33.4.8-jre"
"#,
        )
        .expect("parsed");
        assert_eq!(specs.len(), 2);
        let slf4j = specs
            .iter()
            .find(|s| s.artifact_id == "slf4j-api")
            .expect("slf4j parsed");
        let guava = specs
            .iter()
            .find(|s| s.artifact_id == "guava")
            .expect("guava parsed");
        assert_eq!(slf4j.group_id, "org.slf4j");
        assert_eq!(slf4j.wanted.as_str(), "2.0.16");
        assert_eq!(guava.wanted.as_str(), "33.4.8-jre");
        assert!(guava.wanted.is_exact());
    }

    #[test]
    fn overrides_reject_snapshots_bad_keys_and_duplicates() {
        for raw in [
            r#"[overrides]
"org.slf4j:slf4j-api" = "2.0.16-SNAPSHOT"
"#,
            r#"[overrides]
"org.slf4j" = "2.0.16"
"#,
            r#"[overrides]
"org.slf4j:slf4j-api:x" = "2.0.16"
"#,
            r#"[overrides]
"g:a" = 2
"#,
        ] {
            let error = overrides_of(raw).expect_err("bad override");
            assert!(error.kind == Kind::Manifest, "{error}");
        }
        // A literally duplicated TOML key never reaches the validator: the
        // parser itself rejects it, which is also a clear failure.
        let duplicate =
            toml::from_str::<toml::Table>("[overrides]\n\"g:a\" = \"1\"\n\"g:a\" = \"2\"\n");
        assert!(duplicate.is_err(), "the TOML parser rejects duplicate keys");
    }
}
