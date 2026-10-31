//! `jx new` / `jx init`: create a project layout without any installed JDK
//! (M1). The manifest pins the verified current LTS (ADR 0002); switching
//! later is `jx jdk use <feature>` or an edit production.

use std::path::Path;

use jx_common::error::{JxError, Kind};
use jx_toolchain as toolchain;

/// File contents for a plain application entry point.
const APP_MAIN: &str = "package app;\n\npublic class App {\n    public static void main(String[] args) {\n        System.out.println(\"Hello, jx!\");\n    }\n}\n";

/// Sample library class + JUnit 5 test for `--lib` (`jx test` runs it).
const LIB_CLASS: &str = "package app;\n\npublic class Lib {\n    public static int add(int a, int b) {\n        return a + b;\n    }\n}\n";
const LIB_TEST: &str = "package app;\n\nimport static org.junit.jupiter.api.Assertions.assertEquals;\n\nimport org.junit.jupiter.api.Test;\n\nclass LibTest {\n    @Test\n    void addsTwoNumbers() {\n        assertEquals(2, Lib.add(1, 1));\n    }\n}\n";

const GITIGNORE: &str = "/target\n";

fn manifest_for(name: &str, version: &str, lib: bool) -> String {
    let mut t = format!(
        "# jx manifest: see docs/manifest.md\n\n[project]\nname = \"{name}\"\nversion = \"{version}\"\njdk = \"{}\"\n",
        toolchain::DEFAULT_FEATURE
    );
    if lib {
        t.push_str("\n[dev-dependencies]\njunit = \"org.junit.platform:junit-platform-console-standalone:1\"\n");
    } else {
        t.push_str("\n[run]\nmain = \"app.App\"\n");
    }
    t
}

/// The files to write: (relative path, bytes). Empty dirs come separately.
fn files(lib: bool) -> Vec<(&'static str, String)> {
    let mut v = vec![
        ("project.toml", manifest_for("PROJECT_NAME", "0.1.0", lib)),
        (".gitignore", GITIGNORE.to_string()),
    ];
    if lib {
        v.push(("src/main/java/app/Lib.java", LIB_CLASS.to_string()));
        v.push(("src/test/java/app/LibTest.java", LIB_TEST.to_string()));
    } else {
        v.push(("src/main/java/app/App.java", APP_MAIN.to_string()));
    }
    v
}

const EMPTY_DIRS: [&str; 2] = ["src/main/resources", "src/test/resources"];

fn validate_project_name(name: &str) -> Result<(), JxError> {
    let ok = !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        && name
            .as_bytes()
            .first()
            .is_some_and(|b| b.is_ascii_alphabetic() || *b == b'_');
    if ok {
        return Ok(());
    }
    Err(
        JxError::new(Kind::Usage, format!("`{name}` is not a valid project name")).hint(
            "letters, digits, - and _ only, starting with a letter or _ (it becomes a folder name)",
        ),
    )
}

/// Create in-place (init) or inside `root/new/name` (new).
fn scaffold_into(root: &Path, name: &str, lib: bool, in_place: bool) -> Result<(), JxError> {
    if root.join("project.toml").exists() {
        return Err(JxError::new(
            Kind::Manifest,
            if in_place {
                "this directory already has a project.toml"
            } else {
                "the target directory already has a project.toml"
            },
        )
        .at(root.join("project.toml").display().to_string())
        .hint("edit it, or scaffold into a fresh directory"));
    }
    for marker in ["pom.xml", "build.gradle", "build.gradle.kts"] {
        if root.join(marker).exists() {
            return Err(JxError::new(
                Kind::Manifest,
                format!(
                    "this directory already has a {marker}: jx does not clobber existing builds"
                ),
            )
            .at(root.join(marker).display().to_string())
            .hint("`jx import` converts a Maven or Gradle build to a jx manifest (planned M7)"));
        }
    }
    std::fs::create_dir_all(root).map_err(|e| {
        JxError::new(Kind::Io, "cannot create the project directory")
            .at(root.display().to_string())
            .caused_by(e)
    })?;
    for (rel, contents) in files(lib) {
        let contents = contents.replace("PROJECT_NAME", name);
        let target = root.join(rel);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                JxError::new(Kind::Io, "cannot create a source directory")
                    .at(parent.display().to_string())
                    .caused_by(e)
            })?;
        }
        std::fs::write(&target, contents).map_err(|e| {
            JxError::new(Kind::Io, "cannot write a scaffold file")
                .at(target.display().to_string())
                .caused_by(e)
        })?;
    }
    for dir in EMPTY_DIRS {
        std::fs::create_dir_all(root.join(dir)).map_err(|e| {
            JxError::new(Kind::Io, "cannot create a resources directory")
                .at(root.join(dir).display().to_string())
                .caused_by(e)
        })?;
    }
    Ok(())
}

/// `jx new <name> [--lib]`
pub fn new_project(name: &str, lib: bool) -> Result<std::process::ExitCode, JxError> {
    validate_project_name(name)?;
    let cwd = std::env::current_dir()
        .map_err(|e| JxError::new(Kind::Io, "cannot read the current directory").caused_by(e))?;
    let target = cwd.join(name);
    if target.exists() {
        let is_empty = std::fs::read_dir(&target)
            .map(|mut it| it.next().is_none())
            .unwrap_or(false);
        if !is_empty {
            return Err(JxError::new(
                Kind::Io,
                format!("the directory {name} already exists and is not empty"),
            )
            .at(target.display().to_string())
            .hint("use another name, or run `jx init` inside the existing tree"));
        }
    }
    scaffold_into(&target, name, lib, false)?;
    println!(
        "created jx project `{name}` ({kind}):",
        kind = if lib { "library" } else { "app" }
    );
    println!("  cd {name}");
    println!("  {}", if lib { "jx test" } else { "jx run" });
    Ok(std::process::ExitCode::SUCCESS)
}

/// `jx init`
pub fn init_project() -> Result<std::process::ExitCode, JxError> {
    let cwd = std::env::current_dir()
        .map_err(|e| JxError::new(Kind::Io, "cannot read the current directory").caused_by(e))?;
    let display_name = cwd
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "project".to_string());
    validate_project_name(&display_name)?;
    scaffold_into(&cwd, &display_name, false, true)?;
    println!("jx initialised `{display_name}` here; next: jx run");
    Ok(std::process::ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn names_must_be_clean() {
        assert!(validate_project_name("hello").is_ok());
        assert!(validate_project_name("my_world-2").is_ok());
        assert!(validate_project_name("9world").is_err());
        assert!(validate_project_name("").is_err());
        assert!(validate_project_name("bad name").is_err());
        assert!(validate_project_name("../escape").is_err());
    }

    #[test]
    fn scaffolds_app_layout_without_jdk() {
        let dir = tempdir().unwrap();
        let project = dir.path().join("direct");
        scaffold_into(&project, "direct", false, false).unwrap();
        assert!(project.join("project.toml").is_file());
        assert!(project.join("src/main/java/app/App.java").is_file());
        assert!(project.join("src/main/resources").is_dir());
        assert!(project.join("src/test/resources").is_dir());
        assert!(project.join(".gitignore").is_file());
        let manifest = std::fs::read_to_string(project.join("project.toml")).unwrap();
        assert!(manifest.contains("name = \"direct\""));
        assert!(manifest.contains("jdk = \"25\""));
        assert!(manifest.contains("main = \"app.App\""));
        assert!(!manifest.contains("junit"));
    }

    #[test]
    fn scaffolds_lib_layout_with_junit() {
        let dir = tempdir().unwrap();
        let project = dir.path().join("libproj");
        scaffold_into(&project, "libproj", true, false).unwrap();
        assert!(project.join("src/main/java/app/Lib.java").is_file());
        assert!(project.join("src/test/java/app/LibTest.java").is_file());
        let manifest = std::fs::read_to_string(project.join("project.toml")).unwrap();
        assert!(
            manifest.contains("org.junit.platform:junit-platform-console-standalone"),
            "{manifest}"
        );
        assert!(!manifest.contains("[run]"), "a library has no main class");
    }

    #[test]
    fn refuses_existing_project_or_build() {
        let dir = tempdir().unwrap();
        let project = dir.path().join("p");
        scaffold_into(&project, "p", false, false).unwrap();
        assert!(scaffold_into(&project, "p", false, false).is_err());
        let dir2 = tempdir().unwrap();
        std::fs::write(dir2.path().join("pom.xml"), "<project/>").unwrap();
        let err = scaffold_into(dir2.path(), "maven", false, true).unwrap_err();
        assert!(
            err.message
                .contains("pom.xml: jx does not clobber existing builds")
        );
        assert!(err.hint.is_some_and(|h| h.contains("jx import")));
    }
}
