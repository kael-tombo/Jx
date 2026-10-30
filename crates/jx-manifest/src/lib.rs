//! The project.toml model.
//!
//! M1 scope: the `[project]` table is parsed strictly (unknown keys are
//! errors with a did-you-mean suggestion), every other table known to the
//! full manifest format is recognized and carried opaquely for upcoming
//! milestones, and unknown tables are rejected. The checkout of values that
//! `jx build`/`run` need today lives in [`Manifest::jdk_feature`] and
//! [`Manifest::main_class`]. The target format is specified in
//! docs/manifest.md; `set_jdk` performs a comment- and
//! format-preserving line edit, never a reserialization.

use std::path::Path;

use jx_common::error::{JxError, Kind};

/// Tables jx's manifest supports or reserves for upcoming milestones.
const KNOWN_TABLES: [&str; 7] = [
    "project",
    "dependencies",
    "dev-dependencies",
    "processors",
    "overrides",
    "run",
    "repositories",
];

/// Keys allowed inside `[project]`.
const PROJECT_KEYS: [&str; 3] = ["name", "version", "jdk"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    pub name: String,
    pub version: String,
    /// The pinned feature release as written, e.g. `"25"`.
    pub jdk: String,
    /// `[run] main` when set.
    pub main: Option<String>,
}

use jx_common::diagnose::{closest, unknown_key_error as shared_unknown_key_error};

fn unknown_key_error(kind_table: &str, key: &str, candidates: &[&str]) -> JxError {
    shared_unknown_key_error(Kind::Manifest, kind_table, key, candidates)
}
/// Discover the manifest like cargo does: from `start` up through parent
/// directories. Returns the file path of `project.toml`.
pub fn find_manifest(start: &Path) -> Result<std::path::PathBuf, JxError> {
    // No canonicalisation here: on Windows it would deviate into extended
    //-length paths (\\?\C:\…), which breaks equality with the caller's view.
    let mut dir = Some(start.to_path_buf());
    while let Some(d) = dir {
        let candidate = d.join("project.toml");
        if candidate.is_file() {
            return Ok(candidate);
        }
        dir = d.parent().map(Path::to_path_buf);
    }
    Err(JxError::new(
        Kind::Manifest,
        "no project.toml in this directory or any parent",
    )
    .hint("run `jx init` here, `jx new <name>` for a fresh project, or `jx import` for a Maven/Gradle build"))
}

/// Parse a manifest file. Unknown `[project]` keys and unknown tables are
/// rejected with suggestions; values of not-yet-used tables are untouched.
pub fn parse(path: &Path) -> Result<Manifest, JxError> {
    let raw = std::fs::read_to_string(path).map_err(|e| {
        JxError::new(Kind::Manifest, "cannot read project.toml")
            .at(path.display().to_string())
            .caused_by(e)
    })?;
    let value: toml::Value = toml::from_str(&raw).map_err(|e| {
        JxError::new(Kind::Manifest, "cannot parse project.toml (syntax error)")
            .at(path.display().to_string())
            .caused_by(e)
            .hint("the error quote names the offending TOML construct")
    })?;

    let table = value.as_table().ok_or_else(|| {
        JxError::new(Kind::Manifest, "project.toml must start with key tables")
            .at(path.display().to_string())
    })?;
    let mut main: Option<String> = None;
    for (key, _value) in table {
        if !KNOWN_TABLES.contains(&key.as_str()) {
            let guess = closest(key, &KNOWN_TABLES);
            return Err({
                let mut m = format!("unknown key `{key}` in project.toml");
                if let Some(g) = guess {
                    std::fmt::Write::write_fmt(
                        &mut m,
                        format_args!("\n       did you mean `{g}`?"),
                    )
                    .ok();
                }
                JxError::new(Kind::Manifest, m)
                    .hint(format!("known tables: {}", KNOWN_TABLES.join(", ")))
            });
        }
    }
    let project = table.get("project").ok_or_else(|| {
        JxError::new(Kind::Manifest, "project.toml needs a [project] table")
            .at(path.display().to_string())
    })?;
    let project = project.as_table().ok_or_else(|| {
        JxError::new(Kind::Manifest, "[project] must be a table").at(path.display().to_string())
    })?;
    let mut jdk_opt: Option<String> = None;
    for (key, value) in project {
        match key.as_str() {
            "name" => {
                let name = value.as_str().ok_or_else(|| {
                    JxError::new(Kind::Manifest, "[project] name must be a string")
                        .at(path.display().to_string())
                })?;
                if name.is_empty() {
                    return Err(
                        JxError::new(Kind::Manifest, "[project] name must not be empty")
                            .at(path.display().to_string()),
                    );
                }
            }
            "version" => {
                if value.as_str().is_none() {
                    return Err(
                        JxError::new(Kind::Manifest, "[project] version must be a string")
                            .at(path.display().to_string()),
                    );
                }
            }
            "jdk" => {
                let v = value.as_str().ok_or_else(|| {
                    JxError::new(
                        Kind::Manifest,
                        "[project] jdk must be a quoted feature release like \"25\"",
                    )
                    .at(path.display().to_string())
                })?;
                let v = v.trim();
                if v.is_empty() || !v.bytes().all(|b| b.is_ascii_digit()) {
                    return Err(JxError::new(
                        Kind::Manifest,
                        format!("[project] jdk = \"{v}\" is not a plain feature release"),
                    )
                    .hint("V1 pins a feature release, e.g. jdk = \"25\""));
                }
                jdk_opt = Some(v.to_string());
            }
            other => return Err(unknown_key_error("project", other, &PROJECT_KEYS)),
        }
    }
    let jdk = jdk_opt.ok_or_else(|| {
        JxError::new(Kind::Manifest, "[project] has no `jdk` pin")
            .at(path.display().to_string())
            .hint("add jdk = \"25\" (V1 pins a feature release into project.toml)")
    })?;

    if let Some(run) = table.get("run") {
        if let Some(run) = run.as_table() {
            for key in run.keys() {
                if key != "main" {
                    return Err(unknown_key_error("run", key, &["main"]));
                }
            }
            if let Some(v) = run.get("main") {
                let v = v.as_str().ok_or_else(|| {
                    JxError::new(Kind::Manifest, "[run] main must be a string")
                        .at(path.display().to_string())
                })?;
                if v.is_empty() {
                    return Err(JxError::new(Kind::Manifest, "[run] main must not be empty")
                        .at(path.display().to_string()));
                }
                main = Some(v.to_string());
            }
        } else {
            return Err(JxError::new(Kind::Manifest, "[run] must be a table")
                .at(path.display().to_string()));
        }
    }

    let name = project
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("?")
        .to_string();
    let version = project
        .get("version")
        .and_then(|v| v.as_str())
        .unwrap_or("0.0.0")
        .to_string();

    Ok(Manifest {
        name,
        version,
        jdk,
        main,
    })
}

impl Manifest {
    /// The pinned feature release as `u32`; parse errors are diagnosed.
    pub fn jdk_feature(&self) -> Result<u32, JxError> {
        self.jdk.trim().parse::<u32>().map_err(|_| {
            JxError::new(
                Kind::Manifest,
                format!("cannot parse jdk = \"{}\" as a feature release", self.jdk),
            )
        })
    }

    /// The main class pin, or none when discovery must find it.
    pub fn main_class(&self) -> Option<&String> {
        self.main.as_ref()
    }
}

/// Rewrite only the `jdk = "…"` line inside `[project]`, preserving every
/// other byte of the file, including unrelated comments. Appends the line
/// under `[project]` when missing. Returns true when the file changed.
pub fn set_jdk(path: &Path, feature: u32) -> Result<bool, JxError> {
    let raw = std::fs::read_to_string(path).map_err(|e| {
        JxError::new(Kind::Manifest, "cannot read project.toml")
            .at(path.display().to_string())
            .caused_by(e)
    })?;
    let mut in_project = false;
    let mut replaced = false;
    let mut out = String::with_capacity(raw.len() + 8);
    for line in raw.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if trimmed.starts_with('[') {
            in_project =
                trimmed == "[project]" || trimmed == "[project]\n" || trimmed == "[project]\r\n";
        }
        if in_project && trimmed.starts_with("jdk") && !trimmed.trim_start().starts_with('#') {
            let indent = line.len() - line.trim_start().len();
            let eol = if line.ends_with("\r\n") { "\r\n" } else { "\n" };
            out.push_str(&" ".repeat(indent));
            out.push_str(&format!("jdk = \"{feature}\""));
            out.push_str(eol);
            replaced = true;
        } else {
            out.push_str(line);
        }
    }
    if !replaced {
        return Err(
            JxError::new(Kind::Manifest, " project.toml has no `jdk` line to update")
                .at(path.display().to_string())
                .hint("add jdk = \"25\" under [project] first"),
        );
    }
    if out != raw {
        std::fs::write(path, out).map_err(|e| {
            JxError::new(Kind::Manifest, "cannot write project.toml")
                .at(path.display().to_string())
                .caused_by(e)
        })?;
        Ok(true)
    } else {
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::tempdir;

    fn manifest_at(dir: &Path, content: &str) -> PathBuf {
        let path = dir.join("project.toml");
        std::fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn parses_minimal_manifest() {
        let dir = tempdir().unwrap();
        let p = manifest_at(
            dir.path(),
            "[project]\nname = \"h\"\nversion = \"0.1.0\"\njdk = \"25\"\n",
        );
        let m = parse(&p).unwrap();
        assert_eq!(m.jdk_feature().unwrap(), 25);
        assert!(m.main.is_none());
        assert_eq!(m.name, "h");
        assert_eq!(m.version, "0.1.0");
    }

    #[test]
    fn parses_run_main_and_reserved_tables() {
        let dir = tempdir().unwrap();
        let p = manifest_at(
            dir.path(),
            "[project]\nname = \"h\"\nversion = \"0.1.0\"\njdk = \"25\"\n\n[run]\nmain = \"app.App\"\n\n[dependencies]\nguava = \"com.google.guava:guava:33\"\n",
        );
        let m = parse(&p).unwrap();
        assert_eq!(m.main.as_deref(), Some("app.App"));
    }

    #[test]
    fn unknown_table_rejected_with_suggestion() {
        let dir = tempdir().unwrap();
        let p = manifest_at(
            dir.path(),
            "[project]\nname = \"h\"\nversion = \"0.1.0\"\njdk = \"25\"\n\n[dependancies]\nx = \"y\"\n",
        );
        let e = parse(&p).unwrap_err();
        assert!(e.message.contains("did you mean `dependencies`"));
    }

    #[test]
    fn unknown_project_key_rejected() {
        let dir = tempdir().unwrap();
        let p = manifest_at(
            dir.path(),
            "[project]\nname = \"h\"\nversion = \"0.1.0\"\njdk = \"25\"\nversison = \"s\"\n",
        );
        let e = parse(&p).unwrap_err();
        assert!(e.message.contains("unknown key `versison`"));
        // the suggestion, when present, must name one of the allowed keys
        assert!(e.message.contains("did you mean `version`?"));
    }

    #[test]
    fn project_keys_are_strict() {
        // `description` is not part of the M1 [project] keys; the error must
        // name the key and the allowed set — no suggestion of a nonexistent
        // allowed key.
        let dir = tempdir().unwrap();
        let p = manifest_at(
            dir.path(),
            "[project]\nname = \"h\"\nversion = \"0.1.0\"\njdk = \"25\"\ndescription = \"s\"\n",
        );
        let e = parse(&p).unwrap_err();
        assert!(e.message.contains("unknown key `description`"));
        assert!(e.hint.is_some_and(|h| h.contains("name, version, jdk")));
    }

    #[test]
    fn missing_jdk_pin_is_an_error() {
        let dir = tempdir().unwrap();
        let p = manifest_at(dir.path(), "[project]\nname = \"h\"\nversion = \"0.1.0\"\n");
        let e = parse(&p).unwrap_err();
        assert!(e.message.contains("has no `jdk` pin"));
    }

    #[test]
    fn set_jdk_preserves_comments_and_layout() {
        let dir = tempdir().unwrap();
        let p = manifest_at(
            dir.path(),
            "# my solver pin comments\n[project]\nname = \"h\"\n# pin LTS here\njdk = \"21\"\nversion = \"0.1.0\"\n",
        );
        assert!(set_jdk(&p, 25).unwrap());
        let after = std::fs::read_to_string(&p).unwrap();
        assert!(after.contains("# my solver pin comments"));
        assert!(after.contains("# pin LTS here"));
        assert!(after.contains("jdk = \"25\""));
        assert!(!after.contains("jdk = \"21\""));
        // no-op second time
        assert!(!set_jdk(&p, 25).unwrap());
    }

    #[test]
    fn find_manifest_walks_parents() {
        let dir = tempdir().unwrap();
        manifest_at(dir.path(), "[project]\nname = \"h\"\njdk = \"25\"\n");
        let sub = dir.path().join("a/b");
        std::fs::create_dir_all(&sub).unwrap();
        assert_eq!(
            find_manifest(&sub).unwrap(),
            dir.path().join("project.toml")
        );
        let empty = tempdir().unwrap();
        assert!(find_manifest(empty.path()).is_err());
    }
}
