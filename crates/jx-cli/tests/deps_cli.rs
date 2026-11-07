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
        (
            "ut/proc/1.0/proc-1.0.pom".to_string(),
            ResponseSpec::Sha256(pom(r#"  <groupId>ut</groupId>
  <artifactId>proc</artifactId>
  <version>1.0</version>
  <dependencies>
    <dependency>
      <groupId>ut</groupId>
      <artifactId>proc-lib</artifactId>
      <version>1.0</version>
    </dependency>
  </dependencies>"#)),
        ),
        (
            "ut/proc/1.0/proc-1.0.jar".to_string(),
            ResponseSpec::Sha256(b"proc-jar-bytes".to_vec()),
        ),
        (
            "ut/proc-lib/1.0/proc-lib-1.0.pom".to_string(),
            ResponseSpec::Sha256(pom(
                "<groupId>ut</groupId>\n  <artifactId>proc-lib</artifactId>\n  <version>1.0</version>",
            )),
        ),
        (
            "ut/proc-lib/1.0/proc-lib-1.0.jar".to_string(),
            ResponseSpec::Sha256(b"proc-lib-jar-bytes".to_vec()),
        ),
        (
            "ut/clash/1.0/clash-1.0.pom".to_string(),
            ResponseSpec::Sha256(pom(
                "<groupId>ut</groupId>\n  <artifactId>clash</artifactId>\n  <version>1.0</version>",
            )),
        ),
        (
            "ut/clash/1.0/clash-1.0.jar".to_string(),
            ResponseSpec::Sha256(b"clash-1-jar-bytes".to_vec()),
        ),
        (
            "ut/clash/2.0/clash-2.0.pom".to_string(),
            ResponseSpec::Sha256(pom(
                "<groupId>ut</groupId>\n  <artifactId>clash</artifactId>\n  <version>2.0</version>",
            )),
        ),
        (
            "ut/clash/2.0/clash-2.0.jar".to_string(),
            ResponseSpec::Sha256(b"clash-2-jar-bytes".to_vec()),
        ),
    ]
}

struct Fixture {
    home: TempDir,
    project: TempDir,
    _repo: FakeRepo,
}

fn setup() -> Fixture {
    setup_with_specs(base_specs(), manifest(""))
}

fn setup_with_specs(specs: Vec<(String, ResponseSpec)>, manifest_text: String) -> Fixture {
    let repo = FakeRepo::spawn(specs);
    let home = TempDir::new().expect("jx home");
    std::fs::write(
        home.path().join("config.toml"),
        format!("[repositories.fake]\nurl = \"{}\"\n", repo.url()),
    )
    .expect("config written");
    let project = TempDir::new().expect("project dir");
    std::fs::write(project.path().join("project.toml"), manifest_text).expect("manifest written");
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

fn update_manifest(extra: &str, lib: &str) -> String {
    format!(
        "[project]\nname = \"app\"\nversion = \"0.1.0\"\njdk = \"25\"\n\n[dependencies]\nextra = \"ut:extra:{extra}\"\nlib = \"ut:lib:{lib}\"\n"
    )
}

fn update_specs() -> Vec<(String, ResponseSpec)> {
    let mut specs = base_specs();
    specs.extend([
        (
            "ut/extra/2.1/extra-2.1.pom".to_string(),
            ResponseSpec::Sha256(pom(
                "<groupId>ut</groupId>\n  <artifactId>extra</artifactId>\n  <version>2.1</version>",
            )),
        ),
        (
            "ut/extra/2.1/extra-2.1.jar".to_string(),
            ResponseSpec::Sha256(b"extra-2.1-jar-bytes".to_vec()),
        ),
        (
            "ut/extra/maven-metadata.xml".to_string(),
            ResponseSpec::Sha256(
                b"<metadata><versioning><versions>\
                  <version>2.0</version><version>2.1</version>\
                  </versions></versioning></metadata>"
                    .to_vec(),
            ),
        ),
        (
            "ut/lib/1.1/lib-1.1.pom".to_string(),
            ResponseSpec::Sha256(pom(
                "<groupId>ut</groupId>\n  <artifactId>lib</artifactId>\n  <version>1.1</version>",
            )),
        ),
        (
            "ut/lib/1.1/lib-1.1.jar".to_string(),
            ResponseSpec::Sha256(b"lib-1.1-jar-bytes".to_vec()),
        ),
        (
            "ut/lib/maven-metadata.xml".to_string(),
            ResponseSpec::Sha256(
                b"<metadata><versioning><versions>\
                  <version>1.0</version><version>1.1</version>\
                  </versions></versioning></metadata>"
                    .to_vec(),
            ),
        ),
    ]);
    specs
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
    assert!(
        !lock.contains("[jdk]"),
        "no JDK is installed, so no pin can be derived: {lock}"
    );

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
fn update_refreshes_one_prefix_and_preserves_other_locked_roots() {
    let fixture = setup_with_specs(update_specs(), update_manifest("=2.0", "=1.0"));
    fixture.ok(&["lock"]);

    // Simulate the manifest moving from exact pins to prefixes. The lock still
    // records the old versions, so a selected update has something to keep.
    let manifest_path = fixture.path().join("project.toml");
    std::fs::write(&manifest_path, update_manifest("2", "1")).unwrap();

    let selected = fixture.ok(&["update", "extra"]);
    assert!(selected.contains("ut:extra 2.0 -> 2.1"), "{selected}");
    assert!(
        !selected.contains("ut:lib 1.0 -> 1.1"),
        "the non-target root must stay pinned: {selected}"
    );
    let lock = fixture.lock_text();
    assert!(lock.contains("version = \"2.1\""), "{lock}");
    assert!(lock.contains("version = \"1.0\""), "{lock}");
    assert!(!lock.contains("version = \"1.1\""), "{lock}");

    // No target refreshes every wanted prefix, including the one deliberately
    // preserved above.
    let all = fixture.ok(&["update"]);
    assert!(all.contains("ut:lib 1.0 -> 1.1"), "{all}");
    let lock = fixture.lock_text();
    assert!(lock.contains("version = \"2.1\""), "{lock}");
    assert!(lock.contains("version = \"1.1\""), "{lock}");
}

#[test]
fn update_refuses_exact_pins_and_locked_mode() {
    let fixture = setup();
    fixture.ok(&["lock"]);

    let exact = fixture.fail(&["update", "app"]);
    assert!(exact.contains("pinned exactly"), "{exact}");

    let locked = fixture.fail(&["--locked", "update"]);
    assert!(locked.contains("always writes jx.lock"), "{locked}");
}

#[test]
fn update_without_a_target_can_create_the_lockfile() {
    let fixture = setup();
    let output = fixture.ok(&["update"]);
    assert!(output.contains("ut:app added (1.0)"), "{output}");
    assert!(output.contains("2 version changes"), "{output}");
    assert_eq!(fixture.lock_text().matches("[[artifact]]").count(), 2);
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

#[test]
fn processors_lock_and_tree_separately_from_the_app_graph() {
    let fixture = setup();
    std::fs::write(
        fixture.path().join("project.toml"),
        manifest("\n[processors]\nproc = \"ut:proc:=1.0\"\n"),
    )
    .unwrap();

    let locked = fixture.ok(&["lock"]);
    assert!(locked.contains("4 artifacts"), "{locked}");
    let lock = fixture.lock_text();
    assert_eq!(lock.matches("[[artifact]]").count(), 4);
    assert!(lock.contains("version = 3"), "{lock}");
    // Processor-only entries never join the app classpath.
    assert_eq!(lock.matches("app = false").count(), 2, "{lock}");
    assert_eq!(lock.matches("processor = true").count(), 2, "{lock}");

    let tree = fixture.ok(&["tree"]);
    assert!(tree.contains("annotation processors:"), "{tree}");
    let processor_section = tree
        .split("annotation processors:")
        .nth(1)
        .expect("processor section");
    assert!(processor_section.contains("ut:proc:1.0"), "{tree}");
    assert!(
        processor_section
            .lines()
            .any(|line| line.starts_with("  ut:proc-lib:1.0")),
        "{tree}"
    );

    let fetched = fixture.ok(&["--offline", "fetch"]);
    assert!(fetched.contains("4 artifacts ready"), "{fetched}");
    assert!(fetched.contains("4 cached"), "{fetched}");

    let why = fixture.ok(&["why", "ut:proc"]);
    assert!(why.contains("annotation processors:"), "{why}");
    assert!(why.contains("ut:proc:1.0 (direct)"), "{why}");
    assert!(why.contains("1 level from [processors]"), "{why}");

    let transitive = fixture.ok(&["why", "ut:proc-lib"]);
    assert!(transitive.contains("ut:proc:1.0 (direct)"), "{transitive}");
    assert!(
        transitive
            .lines()
            .any(|line| line.starts_with("  ut:proc-lib:1.0")),
        "{transitive}"
    );
    assert!(
        transitive.contains("2 levels from [processors]"),
        "{transitive}"
    );

    let app = fixture.ok(&["why", "ut:app"]);
    assert!(!app.contains("annotation processors:"), "{app}");

    let error = fixture.fail(&["why", "ut:absent"]);
    assert!(
        error.contains("not in the app or annotation-processor graph"),
        "{error}"
    );
}

#[test]
fn why_reports_both_paths_when_an_artifact_serves_both_roles() {
    let fixture = setup();
    std::fs::write(
        fixture.path().join("project.toml"),
        manifest("\n[processors]\nlib = \"ut:lib:=1.0\"\n"),
    )
    .unwrap();

    let why = fixture.ok(&["why", "ut:lib"]);
    assert!(why.contains("app graph:"), "{why}");
    assert!(why.contains("annotation processors:"), "{why}");
    assert!(why.contains("2 levels from the manifest"), "{why}");
    assert!(why.contains("1 level from [processors]"), "{why}");
}

#[test]
fn processor_add_and_remove_use_the_processors_table() {
    let fixture = setup();
    fixture.ok(&["lock"]);

    let added = fixture.ok(&["add", "--processor", "ut:proc:=1.0"]);
    assert!(
        added.contains("added `proc` = \"ut:proc:=1.0\" to [processors]"),
        "{added}"
    );
    let manifest = std::fs::read_to_string(fixture.path().join("project.toml")).unwrap();
    assert!(manifest.contains("[processors]"), "{manifest}");
    assert!(manifest.contains("proc = \"ut:proc:=1.0\""), "{manifest}");
    assert_eq!(fixture.lock_text().matches("[[artifact]]").count(), 4);
    assert!(fixture.lock_text().contains("processor = true"));

    // Removing from the default table names the right flag instead of
    // silently doing nothing.
    let wrong_table = fixture.fail(&["remove", "proc"]);
    assert!(
        wrong_table.contains("declared in [processors]"),
        "{wrong_table}"
    );
    assert!(wrong_table.contains("--processor"), "{wrong_table}");

    let removed = fixture.ok(&["remove", "--processor", "proc"]);
    assert!(
        removed.contains("removed `proc` from [processors]"),
        "{removed}"
    );
    assert_eq!(fixture.lock_text().matches("[[artifact]]").count(), 2);
    let manifest = std::fs::read_to_string(fixture.path().join("project.toml")).unwrap();
    assert!(!manifest.contains("proc ="), "{manifest}");
}

#[test]
fn a_processor_version_clash_is_a_hard_error_that_overrides_can_fix() {
    let fixture = setup();
    let clashing = |overrides: &str| {
        format!(
            "[project]\nname = \"app\"\nversion = \"0.1.0\"\njdk = \"25\"\n\n\
             [dependencies]\napp = \"ut:app:=1.0\"\nclash = \"ut:clash:=1.0\"\n\n\
             [processors]\nclash = \"ut:clash:=2.0\"\n{overrides}"
        )
    };
    std::fs::write(fixture.path().join("project.toml"), clashing("")).unwrap();
    let error = fixture.fail(&["lock"]);
    assert!(
        error.contains("is 1.0 in the app graph and 2.0 in [processors]"),
        "{error}"
    );
    assert!(error.contains("[overrides]"), "{error}");

    // One override pins both graphs to one version, so the entry can merge
    // into a single artifact that serves both roles.
    std::fs::write(
        fixture.path().join("project.toml"),
        clashing("\n[overrides]\n\"ut:clash\" = \"1.0\"\n"),
    )
    .unwrap();
    fixture.ok(&["lock"]);
    let lock = fixture.lock_text();
    assert_eq!(lock.matches("[[artifact]]").count(), 3);
    let clash = lock
        .split("[[artifact]]")
        .find(|block| block.contains("artifact = \"clash\""))
        .expect("clash entry");
    assert!(clash.contains("processor = true"), "{lock}");
    assert!(!clash.contains("app = false"), "{lock}");
}

#[test]
fn lock_records_the_installed_jdk_pin() {
    let fixture = setup();
    // A JDK installed by jx: the store entry plus the archive it came from.
    let jdk = fixture.home.path().join("jdks").join("temurin-25.0.5+8");
    std::fs::create_dir_all(jdk.join("bin")).unwrap();
    std::fs::write(jdk.join("release"), "JAVA_VERSION=\"25.0.5\"\n").unwrap();
    let archive = fixture
        .home
        .path()
        .join("cache")
        .join("jdks")
        .join("25.0.5+8.tar.gz");
    std::fs::create_dir_all(archive.parent().unwrap()).unwrap();
    std::fs::write(&archive, b"jdk-archive").unwrap();

    fixture.ok(&["lock"]);
    let lock = fixture.lock_text();
    let pin = lock
        .split("[[artifact]]")
        .next()
        .expect("the lock always has text");
    assert!(pin.contains("[jdk]"), "{lock}");
    assert!(pin.contains("provider = \"temurin\""), "{lock}");
    assert!(pin.contains("feature = 25"), "{lock}");
    assert!(pin.contains("release = \"25.0.5+8\""), "{lock}");
    assert!(
        pin.contains(
            "sha256 = \"c50e2535b0ac0d30c33bc98c9f23815badde2c01ad0ba43a109ecc319207034d\""
        ),
        "the pin records the cached archive's checksum: {lock}"
    );

    // The pinned release is installed, so an offline fetch verifies it.
    let fetched = fixture.ok(&["--offline", "fetch"]);
    assert!(fetched.contains("2 artifacts ready"), "{fetched}");

    // A newer installed build replaces the pin on the next explicit lock.
    let newer = fixture.home.path().join("jdks").join("temurin-25.0.6+1");
    std::fs::create_dir_all(newer.join("bin")).unwrap();
    std::fs::write(newer.join("release"), "JAVA_VERSION=\"25.0.6\"\n").unwrap();
    let newer_archive = fixture
        .home
        .path()
        .join("cache")
        .join("jdks")
        .join("25.0.6+1.tar.gz");
    std::fs::write(&newer_archive, b"jdk-archive").unwrap();
    fixture.ok(&["lock"]);
    let relocked = fixture.lock_text();
    assert_eq!(relocked.matches("[jdk]").count(), 1, "{relocked}");
    assert!(
        relocked.contains("release = \"25.0.6+1\""),
        "the installed build becomes the pin: {relocked}"
    );
    assert!(
        relocked.contains(
            "sha256 = \"c50e2535b0ac0d30c33bc98c9f23815badde2c01ad0ba43a109ecc319207034d\""
        ),
        "the new pin records its archive: {relocked}"
    );

    // Re-locking the same toolchain is stable.
    let again = fixture.ok(&["lock"]);
    assert!(again.contains("up to date"), "{again}");
}
