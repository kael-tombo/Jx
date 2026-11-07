//! The project.toml model.
//!
//! Scope: the `[project]` table is parsed strictly (unknown keys are errors
//! with a did-you-mean suggestion), `[dependencies]`/`[dev-dependencies]`
//! entries are validated coordinate shapes (`deps` module; their selection
//! semantics belong to the resolver), `[run]` is re-parsed as in M1, `[build]`
//! selects the build engine, other reserved tables are recognized opaquely,
//! and unknown tables are rejected.
//! The checkout of values that `jx build`/`run` need today lives in
//! [`Manifest::jdk_feature`] and [`Manifest::main_class`]. The target format
//! is specified in docs/manifest.md; `set_jdk` performs a comment- and
//! format-preserving line edit, never a reserialization.

use std::path::Path;

use jx_common::error::{JxError, Kind};

pub mod deps;
pub mod edit;
pub use deps::{DependencySpec, WantedVersion};
pub use edit::{DepTable, add_dependency, has_alias, remove_dependency};

/// Tables jx's manifest supports or reserves for upcoming milestones.
const KNOWN_TABLES: [&str; 9] = [
    "project",
    "dependencies",
    "dev-dependencies",
    "processors",
    "overrides",
    "run",
    "repositories",
    "build",
    "package",
];

/// Keys allowed inside `[project]`.
const PROJECT_KEYS: [&str; 3] = ["name", "version", "jdk"];

/// Keys allowed inside `[build]`.
const BUILD_KEYS: [&str; 1] = ["engine"];

/// Keys allowed inside `[package]`.
const PACKAGE_KEYS: [&str; 1] = ["framework"];

/// Which engine owns the project's build.
///
/// `Jx` is the default and the only mode in which jx compiles, tests and
/// packages by itself. `Maven` and `Gradle` delegate the lifecycle to the
/// project's own build tool while jx still pins and installs the JDK. In a
/// delegated project the external build owns dependency resolution too, so the
/// manifest's dependency commands are refused rather than allowed to describe
/// a graph the build does not use (ADR 0012).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum BuildEngine {
    /// jx's own javac/JUnit/packaging pipeline.
    #[default]
    Jx,
    Maven,
    Gradle,
}

impl BuildEngine {
    pub fn as_str(self) -> &'static str {
        match self {
            BuildEngine::Jx => "jx",
            BuildEngine::Maven => "maven",
            BuildEngine::Gradle => "gradle",
        }
    }

    pub fn is_delegated(self) -> bool {
        !matches!(self, BuildEngine::Jx)
    }

    /// `[build] engine = "…"`, rejected with the allowed set when unknown.
    pub fn parse(raw: &str, path: &Path) -> Result<Self, JxError> {
        match raw.trim() {
            "jx" => Ok(BuildEngine::Jx),
            "maven" => Ok(BuildEngine::Maven),
            "gradle" => Ok(BuildEngine::Gradle),
            other => Err(JxError::new(
                Kind::Manifest,
                format!("[build] engine = {other:?} is not a build engine jx knows"),
            )
            .at(path.display().to_string())
            .hint("use `engine = \"jx\"`, `\"maven\"` or `\"gradle\"`")),
        }
    }
}

/// `[package] framework`: the runnable layout `jx package` writes.
///
/// The adapter writes a layout, not a plugin build. `SpringBoot` reproduces
/// the Boot 3.2-and-later executable jar (including Boot 4); `Micronaut` and
/// `Vertx` need the merged fat jar their own shadow/fat-jar builds produce.
/// `JavaFx` selects a portable thin jar launched through a separate launcher
/// class. `Quarkus` and `JakartaEe` are recognized so the refusal is
/// specific: Quarkus generates its layout while augmenting, and a WAR needs
/// container-provided APIs jx cannot express.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Framework {
    /// Thin jar plus `dist/lib`, or a merged fat jar with `--fat`.
    #[default]
    Plain,
    /// Boot 3.2-and-later executable jar (`BOOT-INF/classes`, `BOOT-INF/lib`).
    SpringBoot,
    /// One merged executable jar, as Micronaut's shadow jar is.
    Micronaut,
    /// One merged executable jar launched through the Vert.x launcher.
    Vertx,
    /// Portable thin jar for a JavaFX launcher class.
    JavaFx,
    /// Refused: Quarkus generates its runnable layout during augmentation.
    Quarkus,
    /// Refused: jx has no `provided` scope for a container WAR.
    JakartaEe,
}

impl Framework {
    pub fn as_str(self) -> &'static str {
        match self {
            Framework::Plain => "plain",
            Framework::SpringBoot => "spring-boot",
            Framework::Micronaut => "micronaut",
            Framework::Vertx => "vertx",
            Framework::JavaFx => "javafx",
            Framework::Quarkus => "quarkus",
            Framework::JakartaEe => "jakarta-ee",
        }
    }

    /// `[package] framework = "…"`, rejected with the allowed set.
    pub fn parse(raw: &str, path: &Path) -> Result<Self, JxError> {
        match raw.trim() {
            "plain" => Ok(Framework::Plain),
            "spring-boot" => Ok(Framework::SpringBoot),
            "micronaut" => Ok(Framework::Micronaut),
            "vertx" => Ok(Framework::Vertx),
            "javafx" => Ok(Framework::JavaFx),
            "quarkus" => Ok(Framework::Quarkus),
            "jakarta-ee" | "javaee" | "j2ee" => Ok(Framework::JakartaEe),
            other => Err(JxError::new(
                Kind::Manifest,
                format!("[package] framework = {other:?} is not a framework jx packages"),
            )
            .at(path.display().to_string())
            .hint("use `plain`, `spring-boot`, `micronaut`, `vertx`, `javafx`, `quarkus` or `jakarta-ee`")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    pub name: String,
    pub version: String,
    /// The pinned feature release as written, e.g. `"25"`.
    pub jdk: String,
    /// `[run] main` when set.
    pub main: Option<String>,
    /// `[dependencies]`, validated shapes (semantics: the resolver, M3).
    pub dependencies: Vec<deps::DependencySpec>,
    /// `[dev-dependencies]`, validated shapes.
    pub dev_dependencies: Vec<deps::DependencySpec>,
    /// `[processors]`, validated shapes: annotation-processor jars with
    /// explicit versions (ADR 0014).
    pub processors: Vec<deps::DependencySpec>,
    /// `[overrides]`, validated shapes: explicit, always-visible pins (ADR 0004).
    pub overrides: Vec<deps::OverrideSpec>,
    /// `[build] engine`; `Jx` unless the project delegates its build.
    pub build_engine: BuildEngine,
    /// `[package] framework`; `Plain` unless a layout adapter is selected.
    pub framework: Framework,
}

use jx_common::diagnose::{closest, unknown_key_error as shared_unknown_key_error};

pub(crate) fn unknown_key_error(kind_table: &str, key: &str, candidates: &[&str]) -> JxError {
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
    parse_text(&raw, path)
}

/// Parse manifest text that has not been written to disk yet (for example a
/// manifest generated by `jx import`). `path` is the intended destination and
/// only appears in diagnostics.
pub fn parse_text(raw: &str, path: &Path) -> Result<Manifest, JxError> {
    let value: toml::Value = toml::from_str(raw).map_err(|e| {
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

    let mut build_engine = BuildEngine::Jx;
    let mut framework = Framework::Plain;
    if let Some(build) = table.get("build") {
        let build = build.as_table().ok_or_else(|| {
            JxError::new(Kind::Manifest, "[build] must be a table").at(path.display().to_string())
        })?;
        for key in build.keys() {
            if !BUILD_KEYS.contains(&key.as_str()) {
                return Err(unknown_key_error("build", key, &BUILD_KEYS));
            }
        }
        if let Some(value) = build.get("engine") {
            let raw = value.as_str().ok_or_else(|| {
                JxError::new(Kind::Manifest, "[build] engine must be a string")
                    .at(path.display().to_string())
                    .hint("use `engine = \"jx\"`, `\"maven\"` or `\"gradle\"`")
            })?;
            build_engine = BuildEngine::parse(raw, path)?;
        }
    }

    if let Some(package) = table.get("package") {
        let package = package.as_table().ok_or_else(|| {
            JxError::new(Kind::Manifest, "[package] must be a table").at(path.display().to_string())
        })?;
        for key in package.keys() {
            if !PACKAGE_KEYS.contains(&key.as_str()) {
                return Err(unknown_key_error("package", key, &PACKAGE_KEYS));
            }
        }
        if let Some(value) = package.get("framework") {
            let raw = value.as_str().ok_or_else(|| {
                JxError::new(Kind::Manifest, "[package] framework must be a string")
                    .at(path.display().to_string())
                    .hint("use `framework = \"spring-boot\"`")
            })?;
            framework = Framework::parse(raw, path)?;
        }
    }

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

    // Dependency tables are validated here so a typo in a coordinate fails
    // at parse time, never deep inside resolution.
    let (dependencies, dev_dependencies) = deps::parse_tables(table, path)?;
    let processors = deps::parse_processors(table, path)?;
    let overrides = deps::parse_overrides(table, path)?;

    Ok(Manifest {
        name,
        version,
        jdk,
        main,
        dependencies,
        dev_dependencies,
        processors,
        overrides,
        build_engine,
        framework,
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
    fn build_engine_defaults_to_jx_and_parses_the_delegates() {
        let dir = tempdir().unwrap();
        let base = "[project]\nname = \"h\"\nversion = \"0.1.0\"\njdk = \"25\"\n";
        let p = manifest_at(dir.path(), base);
        assert_eq!(parse(&p).unwrap().build_engine, BuildEngine::Jx);

        for (raw, expected) in [
            ("maven", BuildEngine::Maven),
            ("gradle", BuildEngine::Gradle),
            ("jx", BuildEngine::Jx),
        ] {
            let p = manifest_at(
                dir.path(),
                &format!("{base}\n[build]\nengine = \"{raw}\"\n"),
            );
            assert_eq!(parse(&p).unwrap().build_engine, expected, "{raw}");
        }
    }

    #[test]
    fn build_engine_is_strict() {
        let dir = tempdir().unwrap();
        let base = "[project]\nname = \"h\"\nversion = \"0.1.0\"\njdk = \"25\"\n";

        let p = manifest_at(dir.path(), &format!("{base}\n[build]\nengine = \"ant\"\n"));
        let e = parse(&p).unwrap_err();
        assert!(e.message.contains("not a build engine"), "{e}");
        assert!(e.hint.is_some_and(|h| h.contains("gradle")));

        let p = manifest_at(dir.path(), &format!("{base}\n[build]\nengine = 1\n"));
        assert!(parse(&p).unwrap_err().message.contains("must be a string"));

        let p = manifest_at(dir.path(), &format!("{base}\n[build]\nengin = \"maven\"\n"));
        let e = parse(&p).unwrap_err();
        assert!(e.message.contains("unknown key `engin`"), "{e}");
        assert!(e.message.contains("did you mean `engine`"), "{e}");

        let p = manifest_at(dir.path(), &format!("build = \"maven\"\n\n{base}"));
        let e = parse(&p).unwrap_err();
        assert!(e.message.contains("must be a table"), "{e}");
    }

    #[test]
    fn framework_defaults_to_plain_and_parses_every_adapter() {
        let dir = tempdir().unwrap();
        let base = "[project]\nname = \"h\"\nversion = \"0.1.0\"\njdk = \"25\"\n";
        let p = manifest_at(dir.path(), base);
        assert_eq!(parse(&p).unwrap().framework, Framework::Plain);

        for (raw, expected) in [
            ("plain", Framework::Plain),
            ("spring-boot", Framework::SpringBoot),
            ("micronaut", Framework::Micronaut),
            ("vertx", Framework::Vertx),
            ("javafx", Framework::JavaFx),
            ("quarkus", Framework::Quarkus),
            ("jakarta-ee", Framework::JakartaEe),
            ("javaee", Framework::JakartaEe),
            ("j2ee", Framework::JakartaEe),
        ] {
            let p = manifest_at(
                dir.path(),
                &format!("{base}\n[package]\nframework = \"{raw}\"\n"),
            );
            assert_eq!(parse(&p).unwrap().framework, expected, "{raw}");
        }
    }

    #[test]
    fn package_framework_is_strict() {
        let dir = tempdir().unwrap();
        let base = "[project]\nname = \"h\"\nversion = \"0.1.0\"\njdk = \"25\"\n";

        let p = manifest_at(
            dir.path(),
            &format!("{base}\n[package]\nframework = \"dropwizard\"\n"),
        );
        let e = parse(&p).unwrap_err();
        assert!(e.message.contains("not a framework jx packages"), "{e}");
        assert!(e.hint.is_some_and(|hint| hint.contains("spring-boot")));

        let p = manifest_at(dir.path(), &format!("{base}\n[package]\nframework = 1\n"));
        assert!(parse(&p).unwrap_err().message.contains("must be a string"));

        let p = manifest_at(
            dir.path(),
            &format!("{base}\n[package]\nframwork = \"spring-boot\"\n"),
        );
        let e = parse(&p).unwrap_err();
        assert!(e.message.contains("unknown key `framwork`"), "{e}");

        let p = manifest_at(dir.path(), &format!("package = \"spring-boot\"\n\n{base}"));
        let e = parse(&p).unwrap_err();
        assert!(e.message.contains("must be a table"), "{e}");
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

    #[test]
    fn dependency_coordinates_are_validated_during_parse() {
        let dir = tempdir().unwrap();
        let p = manifest_at(
            dir.path(),
            "[project]\nname = \"h\"\nversion = \"0.1.0\"\njdk = \"25\"\n\n[dependencies]\ng = \"g:a:1\"\nb = \"not a coordinate\"\n",
        );
        let e = parse(&p).unwrap_err();
        assert!(e.message.contains("not a Maven coordinate"), "{e}");

        let p = manifest_at(
            dir.path(),
            "[project]\nname = \"h\"\nversion = \"0.1.0\"\njdk = \"25\"\n\n[dependencies]\ns = \"g:a:1.0-SNAPSHOT\"\n",
        );
        let e = parse(&p).unwrap_err();
        assert!(e.message.contains("SNAPSHOT"), "{e}");
    }
}
