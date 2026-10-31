//! End-to-end tests of the M3 commands against a fake Maven remote.
//!
//! Each test owns a project directory and a `JX_HOME`, so commands run like
//! a user would run them (`jx lock`, `jx tree`, …) while every byte comes
//! from the local fake repository. No network is used.

use std::path::Path;
use std::process::{Command, Output};

use jx_testsupport::{FakeRepo, ResponseSpec};
use tempfile::TempDir;

fn pom(body: &str) -> Vec<u8> {
    format!(
        r#"<project>
  <modelVersion>4.0.0</modelVersion>
{body}
</project>
"#
    )
    .into_bytes()
}

fn base_specs() -> Vec<(String, ResponseSpec)> {
    vec![
        (
            "ut/app/1.0/app-1.0.pom".to_string(),
            ResponseSpec::Sha256(pom(r#"  <groupId>ut</groupId>
  <artifactId>app</artifactId>
  <version>1.0</version>
  <dependencies>
    <dependency>
      <groupId>ut</groupId>
      <artifactId>lib</artifactId>
      <version>1.0</version>
    </dependency>
  </dependencies>"#)),
        ),
        (
            "ut/app/1.0/app-1.0.jar".to_string(),
            ResponseSpec::Sha256(b"app-jar-bytes".to_vec()),
        ),
        (
            "ut/lib/1.0/lib-1.0.pom".to_string(),
            ResponseSpec::Sha256(pom(
                "<groupId>ut</groupId>\n  <artifactId>lib</artifactId>\n  <version>1.0</version>",
            )),
        ),
        (
            "ut/lib/1.0/lib-1.0.jar".to_string(),
            ResponseSpec::Sha256(b"lib-jar-bytes".to_vec()),
        ),
        (
            "ut/extra/2.0/extra-2.0.pom".to_string(),
            ResponseSpec::Sha256(pom(
                "<groupId>ut</groupId>\n  <artifactId>extra</artifactId>\n  <version>2.0</version>",
            )),
        ),
        (
            "ut/extra/2.0/extra-2.0.jar".to_string(),
            ResponseSpec::Sha256(b"extra-jar-bytes".to_vec()),
        ),
    ]
}

struct Fixture {
    home: TempDir,
    project: TempDir,
    _repo: FakeRepo,
}

fn setup() -> Fixture {
    let repo = FakeRepo::spawn(base_specs());
    let home = TempDir::new().expect("jx home");
    std::fs::write(
        home.path().join("config.toml"),
        format!("[repositories.fake]\nurl = \"{}\"\n", repo.url()),
    )
    .expect("config written");
    let project = TempDir::new().expect("project dir");
    std::fs::write(project.path().join("project.toml"), manifest("")).expect("manifest written");
    Fixture {
        home,
        project,
        _repo: repo,
    }
}

fn manifest(extra: &str) -> String {
    format!(
        "[project]\nname = \"app\"\nversion = \"0.1.0\"\njdk = \"25\"\n\n[dependencies]\napp = \"ut:app:=1.0\"\n{extra}"
    )
}

impl Fixture {
    fn path(&self) -> &Path {
        self.project.path()
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
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    fn fail(&self, args: &[&str]) -> String {
        let output = self.run(args);
        assert!(
            !output.status.success(),
            "jx {args:?} unexpectedly succeeded\nstdout:\n{}",
            String::from_utf8_lossy(&output.stdout)
        );
        format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    }

    fn lock_text(&self) -> String {
        std::fs::read_to_string(self.project.path().join("jx.lock")).expect("jx.lock exists")
    }
}

#[test]
fn lock_tree_why_and_offline_fetch_form_one_flow() {
    let fixture = setup();
    let locked = fixture.ok(&["lock"]);
    assert!(locked.contains("2 artifacts"), "{locked}");
    let lock = fixture.lock_text();
    assert!(lock.contains("group = \"ut\""), "{lock}");
    assert!(lock.contains("artifact = \"app\""), "{lock}");
    assert!(lock.contains("artifact = \"lib\""), "{lock}");
    assert_eq!(lock.matches("[[artifact]]").count(), 2);

    let tree = fixture.ok(&["tree"]);
    assert!(tree.contains("ut:app:1.0"), "{tree}");
    assert!(tree.contains("ut:lib:1.0"), "{tree}");
    // The tree nests the transitive dependency.
    assert!(
        tree.lines().any(|line| line.starts_with("  ut:lib:1.0")),
        "{tree}"
    );

    let why = fixture.ok(&["why", "ut:lib"]);
    assert!(why.contains("ut:app:1.0 (direct)"), "{why}");
    assert!(why.contains("ut:lib:1.0"), "{why}");
    assert!(why.contains("2 levels from the manifest"), "{why}");

    // Every path was cached by `lock`; offline fetch must verify and pass.
    let fetched = fixture.ok(&["--offline", "fetch"]);
    assert!(fetched.contains("2 artifacts ready"), "{fetched}");
    assert!(fetched.contains("2 cached"), "{fetched}");
}

#[test]
fn add_and_remove_keep_the_manifest_and_lock_in_step() {
    let fixture = setup();
    fixture.ok(&["lock"]);

    let added = fixture.ok(&["add", "ut:extra:=2.0"]);
    assert!(added.contains("added `extra`"), "{added}");
    assert!(added.contains("3 artifacts"), "{added}");
    let manifest = std::fs::read_to_string(fixture.path().join("project.toml")).unwrap();
    assert!(manifest.contains("extra = \"ut:extra:=2.0\""), "{manifest}");
    assert_eq!(fixture.lock_text().matches("[[artifact]]").count(), 3);

    // Adding the same coordinate twice is refused before any write.
    let duplicate = fixture.fail(&["add", "ut:extra:=2.0"]);
    assert!(duplicate.contains("already declared"), "{duplicate}");

    let removed = fixture.ok(&["remove", "extra"]);
    assert!(removed.contains("removed `extra`"), "{removed}");
    assert_eq!(fixture.lock_text().matches("[[artifact]]").count(), 2);
    let manifest = std::fs::read_to_string(fixture.path().join("project.toml")).unwrap();
    assert!(!manifest.contains("extra ="), "{manifest}");
}

#[test]
fn a_corrupted_lock_checksum_is_a_hard_error() {
    let fixture = setup();
    fixture.ok(&["lock"]);
    let path = fixture.path().join("jx.lock");
    let lock = std::fs::read_to_string(&path).unwrap();
    let start = lock.find("sha256 = \"").expect("a checksum") + "sha256 = \"".len();
    let end = start + lock[start..].find('"').expect("a closing quote");
    let bad = format!("{}{}{}", &lock[..start], "0".repeat(64), &lock[end..]);
    std::fs::write(&path, bad).unwrap();
    let error = fixture.fail(&["fetch"]);
    assert!(
        error.contains("checksum mismatch") || error.contains("sha256"),
        "{error}"
    );
}

#[test]
fn locked_refuses_to_change_an_existing_lock() {
    let fixture = setup();
    fixture.ok(&["lock"]);
    // An up-to-date lock is fine under --locked.
    fixture.ok(&["--locked", "lock"]);

    // Adding a dependency through the manifest makes the lock stale; the
    // resolver needs extra's POM, which the fake repository serves.
    let path = fixture.path().join("project.toml");
    std::fs::write(&path, manifest("extra = \"ut:extra:=2.0\"\n")).unwrap();
    let error = fixture.fail(&["--locked", "lock"]);
    assert!(error.contains("would change jx.lock"), "{error}");
    assert_eq!(fixture.lock_text().matches("[[artifact]]").count(), 2);
}

#[test]
fn add_without_a_version_picks_the_latest_stable_release() {
    let mut specs = base_specs();
    specs.push((
        "ut/extra/maven-metadata.xml".to_string(),
        ResponseSpec::Sha256(
            b"<metadata><versioning><versions>\
              <version>1.0</version><version>2.0</version><version>3.0-RC1</version>\
              </versions></versioning></metadata>"
                .to_vec(),
        ),
    ));
    let repo = FakeRepo::spawn(specs);
    let home = TempDir::new().unwrap();
    std::fs::write(
        home.path().join("config.toml"),
        format!("[repositories.fake]\nurl = \"{}\"\n", repo.url()),
    )
    .unwrap();
    let project = TempDir::new().unwrap();
    std::fs::write(project.path().join("project.toml"), manifest("")).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_jx"))
        .args(["--color", "never", "add", "ut:extra"])
        .current_dir(project.path())
        .env("JX_HOME", home.path())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(stdout.contains("latest release"), "{stdout}");
    let manifest = std::fs::read_to_string(project.path().join("project.toml")).unwrap();
    assert!(manifest.contains("extra = \"ut:extra:2.0\""), "{manifest}");
}

#[test]
fn why_names_a_missing_artifact() {
    let fixture = setup();
    // no lock is needed for why; resolution is live against the fake repo
    let error = fixture.fail(&["why", "ut:absent"]);
    assert!(error.contains("not in the resolved graph"), "{error}");
}

#[test]
fn offline_lock_with_an_empty_cache_explains_the_cache_requirement() {
    let fixture = setup();
    let error = fixture.fail(&["--offline", "lock"]);
    assert!(
        error.contains("never fetched") || error.contains("offline cache"),
        "{error}"
    );
}
