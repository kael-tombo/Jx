//! End-to-end tests for `jx script`. A fake JBang executable records the
//! invocation, so the tests need neither JBang nor a JDK download.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tempfile::TempDir;

struct Fixture {
    home: TempDir,
    project: TempDir,
}

impl Fixture {
    fn new() -> Fixture {
        Fixture {
            home: TempDir::new().expect("jx home"),
            project: TempDir::new().expect("project dir"),
        }
    }

    fn path(&self) -> &Path {
        self.project.path()
    }

    fn run(&self, args: &[&str], jbang: Option<&Path>, log: Option<&Path>) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_jx"));
        command
            .args(["--color", "never"])
            .args(args)
            .current_dir(self.project.path())
            .env("JX_HOME", self.home.path())
            .env("NO_COLOR", "1");
        match jbang {
            Some(path) => {
                command.env("JX_JBANG", path);
            }
            None => {
                command.env_remove("JX_JBANG");
            }
        }
        if let Some(log) = log {
            command.env("JX_TEST_LOG", log);
        }
        command.output().expect("jx launches")
    }
}

fn write_script(dir: &Path) -> PathBuf {
    let script = dir.join("hello.java");
    fs::write(
        &script,
        "class Hello { public static void main(String[] a) {} }\n",
    )
    .unwrap();
    script
}

fn write_project(dir: &Path, feature: u32) {
    fs::write(
        dir.join("project.toml"),
        format!("[project]\nname = \"demo\"\nversion = \"0.1.0\"\njdk = \"{feature}\"\n"),
    )
    .unwrap();
}

fn write_fake_jdk(home: &Path, version: &str) -> PathBuf {
    let jdk = home.join("jdks").join(format!("temurin-{version}"));
    fs::create_dir_all(jdk.join("bin")).unwrap();
    fs::write(jdk.join("release"), "JAVA_VERSION=25\n").unwrap();
    jdk
}

#[cfg(unix)]
fn write_fake_jbang(dir: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let tool = dir.join("jbang");
    fs::write(
        &tool,
        "#!/bin/sh\n{\n  printf 'JAVA_HOME=%s\\n' \"$JAVA_HOME\"\n  printf '%s\\n' \"$@\"\n} > \"$JX_TEST_LOG\"\nexit 3\n",
    )
    .unwrap();
    let mut permissions = fs::metadata(&tool).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&tool, permissions).unwrap();
    tool
}

#[cfg(windows)]
fn write_fake_jbang(dir: &Path) -> PathBuf {
    let tool = dir.join("jbang.cmd");
    fs::write(
        &tool,
        "@echo off\r\necho JAVA_HOME=%JAVA_HOME% > \"%JX_TEST_LOG%\"\r\necho %* >> \"%JX_TEST_LOG%\"\r\nexit /b 3\r\n",
    )
    .unwrap();
    tool
}

#[test]
fn script_is_handed_to_jbang_with_arguments_and_exit_code() {
    let fixture = Fixture::new();
    write_script(fixture.path());
    let log = fixture.path().join("jbang.log");
    let tool = write_fake_jbang(fixture.home.path());

    let output = fixture.run(
        &["script", "hello.java", "alpha", "beta"],
        Some(&tool),
        Some(&log),
    );
    assert_eq!(
        output.status.code(),
        Some(3),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("delegating"), "{stderr}");

    let recorded = fs::read_to_string(&log).expect("fake JBang log");
    assert!(recorded.contains("hello.java"), "{recorded}");
    assert!(recorded.contains("alpha"), "{recorded}");
    assert!(recorded.contains("beta"), "{recorded}");
}

#[test]
fn a_missing_script_is_diagnosed_before_jbang_is_started() {
    let fixture = Fixture::new();
    let output = fixture.run(&["script", "missing.java"], None, None);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("does not exist"), "{stderr}");
    assert!(stderr.contains("JBang"), "{stderr}");
}

#[test]
fn a_missing_jbang_is_diagnosed_before_a_jdk_download() {
    let fixture = Fixture::new();
    write_script(fixture.path());
    write_project(fixture.path(), 25);

    let output = fixture.run(
        &["--offline", "script", "hello.java"],
        Some(&fixture.home.path().join("missing-jbang")),
        None,
    );
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("JX_JBANG"), "{stderr}");
    assert!(
        !stderr.contains("no jx JDK"),
        "the JDK was touched first:\n{stderr}"
    );
    assert!(
        !fixture.home.path().join("jdks").exists(),
        "a missing JBang must not create the JDK store"
    );
}

#[test]
fn the_project_above_the_script_supplies_the_pinned_jdk() {
    let fixture = Fixture::new();
    let nested = fixture.path().join("nested");
    fs::create_dir_all(&nested).unwrap();
    write_script(&nested);
    write_project(&nested, 25);
    let jdk = write_fake_jdk(fixture.home.path(), "25.0.1");
    let log = fixture.path().join("jbang.log");
    let tool = write_fake_jbang(fixture.home.path());

    let output = fixture.run(&["script", "nested/hello.java"], Some(&tool), Some(&log));
    assert_eq!(
        output.status.code(),
        Some(3),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let recorded = fs::read_to_string(&log).expect("fake JBang log");
    assert!(
        recorded.contains(&format!("JAVA_HOME={}", jdk.display())),
        "{recorded}"
    );
    assert!(recorded.contains("hello.java"), "{recorded}");
}
