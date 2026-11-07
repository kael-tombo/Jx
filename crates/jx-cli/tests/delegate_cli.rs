//! End-to-end tests of delegated builds.
//!
//! A fake jx home supplies an installed JDK (delegation only needs its
//! directory) and a project wrapper records the arguments and `JAVA_HOME`.
//! No Maven, Gradle or network is involved.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tempfile::TempDir;

struct Fixture {
    home: TempDir,
    project: TempDir,
}

impl Fixture {
    /// A manifest pinned to JDK 25 and delegating to `engine`.
    fn new(engine: &str) -> Fixture {
        let home = TempDir::new().expect("jx home");
        let project = TempDir::new().expect("project dir");
        let jdk = home.path().join("jdks").join("temurin-25.0.5+8");
        std::fs::create_dir_all(jdk.join("bin")).expect("fake jdk");
        std::fs::write(jdk.join("release"), "JAVA_VERSION=25\n").expect("fake release file");
        std::fs::write(
            project.path().join("project.toml"),
            format!(
                "[project]\nname = \"delegated\"\nversion = \"0.1.0\"\njdk = \"25\"\n\n[build]\nengine = \"{engine}\"\n"
            ),
        )
        .expect("manifest");
        Fixture { home, project }
    }

    fn path(&self) -> &Path {
        self.project.path()
    }

    fn jdk(&self) -> PathBuf {
        self.home.path().join("jdks").join("temurin-25.0.5+8")
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_jx"))
            .args(["--color", "never"])
            .args(args)
            .current_dir(self.project.path())
            .env("JX_HOME", self.home.path())
            .env("NO_COLOR", "1")
            .output()
            .expect("jx launches")
    }

    /// A wrapper that records its environment and arguments, then exits with
    /// `code`. The platform-appropriate name is the one jx prefers.
    fn write_wrapper(&self, engine: &str, code: i32) {
        #[cfg(windows)]
        {
            let name = if engine == "maven" {
                "mvnw.cmd"
            } else {
                "gradlew.bat"
            };
            std::fs::write(
                self.path().join(name),
                format!(
                    "@echo off\r\necho %JAVA_HOME%>captured.txt\r\necho %*>args.txt\r\nexit /b {code}\r\n"
                ),
            )
            .expect("wrapper");
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let name = if engine == "maven" { "mvnw" } else { "gradlew" };
            let path = self.path().join(name);
            std::fs::write(
                &path,
                format!(
                    "#!/bin/sh\nprintf '%s\\n' \"$JAVA_HOME\" > captured.txt\nprintf '%s\\n' \"$*\" > args.txt\nexit {code}\n"
                ),
            )
            .expect("wrapper");
            let mut permissions = std::fs::metadata(&path)
                .expect("wrapper metadata")
                .permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&path, permissions).expect("executable wrapper");
        }
    }

    fn recorded_java_home(&self) -> PathBuf {
        let raw = std::fs::read_to_string(self.path().join("captured.txt")).expect("JAVA_HOME");
        PathBuf::from(raw.trim())
    }

    fn recorded_args(&self) -> String {
        std::fs::read_to_string(self.path().join("args.txt"))
            .expect("arguments")
            .trim()
            .to_string()
    }
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        stdout(output),
        stderr(output)
    );
}

fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

#[test]
fn delegated_build_prefers_the_wrapper_and_exports_the_pinned_jdk() {
    let fixture = Fixture::new("maven");
    std::fs::write(fixture.path().join("pom.xml"), "<project/>\n").unwrap();
    fixture.write_wrapper("maven", 0);

    let output = fixture.run(&["build"]);
    assert_success(&output);
    assert!(
        stdout(&output).contains("delegating to Maven: mvnw"),
        "{}",
        stdout(&output)
    );
    assert_eq!(fixture.recorded_args(), "-B compile");
    assert_eq!(
        canonical(&fixture.recorded_java_home()),
        canonical(&fixture.jdk()),
        "the delegated build must see the pinned JDK as JAVA_HOME"
    );
}

#[test]
fn delegated_build_forwards_tool_arguments_after_the_separator() {
    let fixture = Fixture::new("maven");
    std::fs::write(fixture.path().join("pom.xml"), "<project/>\n").unwrap();
    fixture.write_wrapper("maven", 0);

    let output = fixture.run(&["build", "--", "-DskipTests", "-Pfast"]);
    assert_success(&output);
    assert_eq!(fixture.recorded_args(), "-B compile -DskipTests -Pfast");
}

#[test]
fn delegated_test_and_clean_forward_their_own_flags() {
    let fixture = Fixture::new("gradle");
    std::fs::write(fixture.path().join("build.gradle.kts"), "").unwrap();
    fixture.write_wrapper("gradle", 0);

    let tested = fixture.run(&["test", "--", "--tests", "app.LibTest"]);
    assert_success(&tested);
    assert_eq!(
        fixture.recorded_args(),
        "--console=plain test --tests app.LibTest"
    );

    let cleaned = fixture.run(&["clean", "--", "--refresh-dependencies"]);
    assert_success(&cleaned);
    assert_eq!(
        fixture.recorded_args(),
        "--console=plain clean --refresh-dependencies"
    );
}

#[test]
fn tool_arguments_are_refused_when_jx_owns_the_build() {
    let fixture = Fixture::new("jx");
    let target = fixture.path().join("target");
    std::fs::create_dir_all(&target).unwrap();

    for command in ["build", "test", "clean"] {
        let output = fixture.run(&[command, "--", "--parallel"]);
        assert_eq!(output.status.code(), Some(2), "{command}");
        let message = stderr(&output);
        assert!(
            message.contains("need a delegated build"),
            "{command}: {message}"
        );
        assert!(message.contains("--engine maven"), "{command}: {message}");
    }
    assert!(target.is_dir(), "a refused clean must not delete output");
}

#[test]
fn delegated_test_and_clean_use_each_engines_own_goal() {
    let fixture = Fixture::new("gradle");
    std::fs::write(fixture.path().join("build.gradle.kts"), "").unwrap();
    fixture.write_wrapper("gradle", 0);

    let tested = fixture.run(&["test"]);
    assert_success(&tested);
    assert!(
        stdout(&tested).contains("delegating to Gradle: gradlew"),
        "{}",
        stdout(&tested)
    );
    assert_eq!(fixture.recorded_args(), "--console=plain test");

    let cleaned = fixture.run(&["clean"]);
    assert_success(&cleaned);
    assert_eq!(fixture.recorded_args(), "--console=plain clean");
}

#[test]
fn a_one_off_engine_flag_delegates_without_editing_the_manifest() {
    let fixture = Fixture::new("jx");
    std::fs::write(fixture.path().join("pom.xml"), "<project/>\n").unwrap();
    fixture.write_wrapper("maven", 0);

    let output = fixture.run(&["--engine", "maven", "build"]);
    assert_success(&output);
    assert!(
        stdout(&output).contains("delegating to Maven: mvnw"),
        "{}",
        stdout(&output)
    );
    assert_eq!(fixture.recorded_args(), "-B compile");
}

#[test]
fn offline_build_passes_the_engines_offline_flag() {
    let fixture = Fixture::new("maven");
    std::fs::write(fixture.path().join("pom.xml"), "<project/>\n").unwrap();
    fixture.write_wrapper("maven", 0);

    let output = fixture.run(&["--offline", "build"]);
    assert_success(&output);
    assert_eq!(fixture.recorded_args(), "-B -o compile");
}

#[test]
fn a_failing_build_tool_exit_code_is_propagated() {
    let fixture = Fixture::new("maven");
    std::fs::write(fixture.path().join("pom.xml"), "<project/>\n").unwrap();
    fixture.write_wrapper("maven", 7);

    let output = fixture.run(&["build"]);
    assert_eq!(
        output.status.code(),
        Some(7),
        "stderr:\n{}",
        stderr(&output)
    );
}

#[test]
fn engine_selection_can_be_overridden_for_one_invocation() {
    let fixture = Fixture::new("maven");

    let refused = fixture.run(&["tree"]);
    assert!(!refused.status.success());
    assert!(
        stderr(&refused).contains("unavailable"),
        "{}",
        stderr(&refused)
    );
    assert!(
        stderr(&refused).contains("--engine jx"),
        "{}",
        stderr(&refused)
    );

    let manifest_view = fixture.run(&["--engine", "jx", "tree"]);
    assert_success(&manifest_view);
    assert!(
        stdout(&manifest_view).contains("no dependencies declared"),
        "{}",
        stdout(&manifest_view)
    );
}

#[test]
fn dependency_commands_and_exports_are_refused_under_delegation() {
    let fixture = Fixture::new("gradle");
    std::fs::write(fixture.path().join("build.gradle.kts"), "").unwrap();

    for (args, expected) in [
        (vec!["add", "com.google.guava:guava:33"], "jx add"),
        (vec!["export", "gradle"], "jx export gradle"),
        (vec!["run"], "jx run"),
        (vec!["package"], "jx package"),
        (vec!["image"], "jx image"),
    ] {
        let output = fixture.run(&args);
        assert!(!output.status.success(), "{args:?} should fail");
        let message = stderr(&output);
        assert!(message.contains(expected), "{args:?}: {message}");
        assert!(
            message.contains("delegates its build"),
            "{args:?}: {message}"
        );
    }
}

#[test]
fn test_filters_and_report_dirs_are_refused_under_delegation() {
    let fixture = Fixture::new("maven");
    std::fs::write(fixture.path().join("pom.xml"), "<project/>\n").unwrap();

    for args in [
        vec!["test", "--filter", "app.LibTest"],
        vec!["test", "--reports-dir", "reports"],
    ] {
        let output = fixture.run(&args);
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        let message = stderr(&output);
        assert!(
            message.contains("belong to jx's own test runner"),
            "{args:?}: {message}"
        );
        assert!(message.contains("--engine jx"), "{args:?}: {message}");
    }
}

#[test]
fn a_missing_delegated_build_file_is_named_with_a_remedy() {
    let fixture = Fixture::new("maven");

    let output = fixture.run(&["build"]);
    assert!(!output.status.success());
    let message = stderr(&output);
    assert!(message.contains("no pom.xml to delegate to"), "{message}");
    assert!(message.contains("export maven"), "{message}");
}

#[test]
fn clean_with_a_manifest_removes_the_project_target_from_a_subdirectory() {
    let fixture = Fixture::new("jx");
    let target = fixture.path().join("target");
    std::fs::create_dir_all(&target).unwrap();
    std::fs::write(target.join("stale.class"), "").unwrap();
    let nested = fixture.path().join("src");
    std::fs::create_dir_all(&nested).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_jx"))
        .args(["--color", "never", "clean"])
        .current_dir(&nested)
        .env("JX_HOME", fixture.home.path())
        .env("NO_COLOR", "1")
        .output()
        .expect("jx launches");
    assert_success(&output);
    assert!(!target.exists());
}

#[test]
fn clean_without_a_manifest_still_removes_target_below_the_cwd() {
    let home = TempDir::new().expect("jx home");
    let project = TempDir::new().expect("project dir");
    let target = project.path().join("target");
    std::fs::create_dir_all(&target).unwrap();
    std::fs::write(target.join("stale.class"), "").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_jx"))
        .args(["--color", "never", "clean"])
        .current_dir(project.path())
        .env("JX_HOME", home.path())
        .env("NO_COLOR", "1")
        .output()
        .expect("jx launches");
    assert_success(&output);
    assert!(!target.exists());
}
