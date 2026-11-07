//! End-to-end tests for the optional-adapter section of `jx doctor`.
//!
//! Each test runs the real binary with `JX_HOME` pointing at an empty temp
//! directory and `PATH` pointing at a directory this test controls, so the
//! JBang and package-manager findings are deterministic on every machine.
//! `doctor` is diagnostic only: none of these states may fail the run.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tempfile::TempDir;

struct Fixture {
    home: TempDir,
    bin: TempDir,
}

impl Fixture {
    fn new() -> Fixture {
        Fixture {
            home: TempDir::new().expect("jx home"),
            bin: TempDir::new().expect("fake bin dir"),
        }
    }

    /// Run `jx doctor` with an empty PATH and no inherited JBang override.
    fn doctor(&self) -> Output {
        self.doctor_with(|_| {})
    }

    fn doctor_with(&self, arrange: impl FnOnce(&mut Command)) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_jx"));
        command
            .args(["--color", "never", "doctor"])
            .env("JX_HOME", self.home.path())
            .env("PATH", self.bin.path())
            .env("NO_COLOR", "1")
            .env_remove("JX_JBANG");
        arrange(&mut command);
        command.output().expect("jx launches")
    }
}

fn stdout(output: &Output) -> String {
    assert_eq!(
        output.status.code(),
        Some(0),
        "doctor is diagnostic and must not fail; stdout:\\n{}\\nstderr:\\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn adapters_line(text: &str, area: &str) -> String {
    text.lines()
        .find(|line| line.contains(area))
        .unwrap_or_else(|| panic!("no {area} finding in:\\n{text}"))
        .to_string()
}

#[test]
fn absent_tools_are_reported_as_optional_and_do_not_fail() {
    let fixture = Fixture::new();
    let text = stdout(&fixture.doctor());

    let jbang = adapters_line(&text, "jbang");
    assert!(jbang.contains("not on PATH"), "{jbang}");
    assert!(jbang.contains("optional"), "{jbang}");
    assert!(jbang.contains("jx script"), "{jbang}");
    // The install hint names this platform's package manager, not just a URL.
    let expected_hint = if cfg!(windows) {
        "choco install jbang"
    } else {
        "brew install jbang"
    };
    assert!(text.contains(expected_hint), "{text}");

    let packages = adapters_line(&text, "packages");
    let expected = if cfg!(windows) {
        "winget, choco or scoop not found"
    } else {
        "brew not found"
    };
    assert!(
        packages.contains(expected),
        "{packages}\\nexpected: {expected}"
    );
    assert!(
        packages.contains("jx only renders their manifests"),
        "{packages}"
    );
}

#[cfg(unix)]
fn fake_tool(dir: &Path, name: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let tool = dir.join(name);
    fs::write(&tool, "#!/bin/sh\\nexit 0\\n").unwrap();
    let mut permissions = fs::metadata(&tool).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&tool, permissions).unwrap();
    tool
}

#[cfg(windows)]
fn fake_tool(dir: &Path, name: &str) -> PathBuf {
    let tool = dir.join(format!("{name}.cmd"));
    fs::write(&tool, "@echo off\\r\\nexit /b 0\\r\\n").unwrap();
    tool
}

#[test]
fn an_on_path_jbang_is_named() {
    let fixture = Fixture::new();
    let tool = fake_tool(fixture.bin.path(), "jbang");

    let text = stdout(&fixture.doctor());
    let jbang = adapters_line(&text, "jbang");
    assert!(jbang.contains(&tool.display().to_string()), "{jbang}");
    assert!(jbang.contains("will delegate to it"), "{jbang}");
}

#[test]
fn a_valid_override_is_used_and_preferred() {
    let fixture = Fixture::new();
    // Keep the override name from starting with `jbang`: on Unix the fake
    // PATH tool is `<dir>/jbang`, which would be a string prefix of
    // `<dir>/jbang-override` and trip the "PATH did not win" assertion.
    let tool = fake_tool(fixture.bin.path(), "alternate-jbang");
    let on_path = fake_tool(fixture.bin.path(), "jbang");

    let text = stdout(&fixture.doctor_with(|command| {
        command.env("JX_JBANG", &tool);
    }));
    let jbang = adapters_line(&text, "jbang");
    assert!(jbang.contains("JX_JBANG="), "{jbang}");
    assert!(jbang.contains(&tool.display().to_string()), "{jbang}");
    assert!(jbang.contains("will use it"), "{jbang}");
    assert!(
        !jbang.contains(&on_path.display().to_string()),
        "PATH should not win over a valid override: {jbang}"
    );
}

#[test]
fn a_broken_override_warns_but_still_succeeds() {
    let fixture = Fixture::new();
    let missing = fixture.home.path().join("no-such-jbang");

    let text = stdout(&fixture.doctor_with(|command| {
        command.env("JX_JBANG", &missing);
    }));
    let jbang = adapters_line(&text, "jbang");
    assert!(jbang.contains("warn"), "{jbang}");
    assert!(jbang.contains(&missing.display().to_string()), "{jbang}");
    assert!(jbang.contains("not a file"), "{jbang}");
    // The fix hint is a continuation line below the finding.
    assert!(text.contains("hint:"), "{text}");
    assert!(text.contains("unset JX_JBANG"), "{text}");
}

#[test]
fn quiet_keeps_warnings_and_drops_information() {
    let fixture = Fixture::new();
    let missing = fixture.home.path().join("no-such-jbang");

    let output = fixture.doctor_with(|command| {
        command.arg("--quiet");
        command.env("JX_JBANG", &missing);
    });
    assert_eq!(output.status.code(), Some(0));
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(
        adapters_line(&text, "jbang").contains("not a file"),
        "{text}"
    );
    assert!(
        !text.lines().any(|line| line.contains("packages")),
        "quiet must hide the informational package summary:\\n{text}"
    );
}
