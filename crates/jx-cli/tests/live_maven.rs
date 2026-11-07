//! Opt-in live Maven Central smoke test.
//!
//! Ignored so normal CI stays hermetic. This test has no fake repository:
//! every POM and jar comes from Maven Central.
//!
//! Run it with:
//! `cargo test -p jx-cli --test live_maven -- --ignored --nocapture`

use std::process::{Command, Output};

use tempfile::TempDir;

struct Fixture {
    home: TempDir,
    project: TempDir,
}

impl Fixture {
    fn new() -> Fixture {
        let home = TempDir::new().expect("jx home");
        let project = TempDir::new().expect("project dir");
        std::fs::write(
            home.path().join("config.toml"),
            "[repositories.central]\nurl = \"https://repo.maven.apache.org/maven2/\"\n",
        )
        .expect("config");
        std::fs::write(
            project.path().join("project.toml"),
            r#"[project]
name = "live-smoke"
version = "0.1.0"
jdk = "25"

[dependencies]
guava = "com.google.guava:guava:=33.4.0-jre"
lombok = "org.projectlombok:lombok:=1.18.36"

[processors]
lombok = "org.projectlombok:lombok:=1.18.36"
mapstruct = "org.mapstruct:mapstruct-processor:=1.6.3"
"#,
        )
        .expect("manifest");
        Fixture { home, project }
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

    fn ok(&self, args: &[&str]) -> String {
        let output = self.run(args);
        assert!(
            output.status.success(),
            "jx {args:?} failed\nstdout:\n{}\nstderr:\n{}",
            stdout(&output),
            stderr(&output)
        );
        stdout(&output)
    }

    fn lock_text(&self) -> String {
        std::fs::read_to_string(self.project.path().join("jx.lock")).expect("jx.lock exists")
    }
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
#[ignore = "live Maven Central and network access; run with --ignored"]
fn live_maven_central_why_lock_and_offline_fetch() {
    let fixture = Fixture::new();

    let app = fixture.ok(&["why", "com.google.guava:guava"]);
    assert!(
        app.contains("com.google.guava:guava:33.4.0-jre (direct)"),
        "{app}"
    );
    assert!(app.contains("1 level from the manifest"), "{app}");
    assert!(!app.contains("annotation processors:"), "{app}");

    let transitive = fixture.ok(&["why", "com.google.guava:failureaccess"]);
    assert!(
        transitive.contains("com.google.guava:guava:33.4.0-jre (direct)"),
        "{transitive}"
    );
    assert!(
        transitive.contains("com.google.guava:failureaccess:1.0.2"),
        "{transitive}"
    );
    assert!(
        transitive.contains("2 levels from the manifest"),
        "{transitive}"
    );

    let shared = fixture.ok(&["why", "org.projectlombok:lombok"]);
    assert!(shared.contains("app graph:"), "{shared}");
    assert!(shared.contains("annotation processors:"), "{shared}");
    assert!(shared.contains("1 level from the manifest"), "{shared}");
    assert!(shared.contains("1 level from [processors]"), "{shared}");

    let processor_only = fixture.ok(&["why", "org.mapstruct:mapstruct-processor"]);
    assert!(
        processor_only.contains("annotation processors:"),
        "{processor_only}"
    );
    assert!(!processor_only.contains("app graph:"), "{processor_only}");
    assert!(
        processor_only.contains("1 level from [processors]"),
        "{processor_only}"
    );

    let wrong_version = fixture.run(&["why", "com.google.guava:guava:99.0"]);
    assert_eq!(wrong_version.status.code(), Some(1));
    assert!(
        stderr(&wrong_version).contains("resolves to 33.4.0-jre, not 99.0"),
        "{}",
        stderr(&wrong_version)
    );

    let missing = fixture.run(&["why", "com.google.guava:absent"]);
    assert_eq!(missing.status.code(), Some(1));
    assert!(
        stderr(&missing).contains("not in the app or annotation-processor graph"),
        "{}",
        stderr(&missing)
    );

    let locked = fixture.ok(&["lock"]);
    assert!(locked.contains("9 artifacts"), "{locked}");
    let lock = fixture.lock_text();
    assert!(lock.contains("version = 3"), "{lock}");
    assert_eq!(lock.matches("[[artifact]]").count(), 9, "{lock}");
    assert_eq!(lock.matches("app = false").count(), 1, "{lock}");
    assert_eq!(lock.matches("processor = true").count(), 2, "{lock}");

    let fetched = fixture.ok(&["--offline", "fetch"]);
    assert!(fetched.contains("9 artifacts ready"), "{fetched}");
    assert!(fetched.contains("9 cached, 0 downloaded"), "{fetched}");
}
