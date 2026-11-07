//! Framework guards that fail before compilation or a JDK install.
//!
//! Each test uses the local fake Maven remote, so the lockfile can be built
//! without network access. The assertions target configuration errors that
//! must surface before jx compiles anything.

use std::process::{Command, Output};

use jx_testsupport::{FakeRepo, ResponseSpec};
use tempfile::TempDir;

fn pom(group: &str, artifact: &str, version: &str) -> Vec<u8> {
    format!(
        "<project>\n  <modelVersion>4.0.0</modelVersion>\n  <groupId>{group}</groupId>\n  <artifactId>{artifact}</artifactId>\n  <version>{version}</version>\n</project>\n"
    )
    .into_bytes()
}

fn spec(group: &str, artifact: &str, version: &str) -> Vec<(String, ResponseSpec)> {
    let path = group.replace('.', "/");
    vec![
        (
            format!("{path}/{artifact}/{version}/{artifact}-{version}.pom"),
            ResponseSpec::Sha256(pom(group, artifact, version)),
        ),
        (
            format!("{path}/{artifact}/{version}/{artifact}-{version}.jar"),
            ResponseSpec::Sha256(b"framework-test-bytes".to_vec()),
        ),
    ]
}

struct Fixture {
    home: TempDir,
    project: TempDir,
    _repo: FakeRepo,
}

fn setup_bare(specs: Vec<(String, ResponseSpec)>) -> Fixture {
    let repo = FakeRepo::spawn(specs);
    let home = TempDir::new().expect("jx home");
    std::fs::write(
        home.path().join("config.toml"),
        format!("[repositories.fake]\nurl = \"{}\"\n", repo.url()),
    )
    .expect("config written");
    let project = TempDir::new().expect("project dir");
    Fixture {
        home,
        project,
        _repo: repo,
    }
}

fn setup(manifest: &str, specs: Vec<(String, ResponseSpec)>) -> Fixture {
    let fixture = setup_bare(specs);
    std::fs::write(fixture.project.path().join("project.toml"), manifest)
        .expect("manifest written");
    fixture
}

fn manifest(feature: &str, framework: &str, dependency: &str) -> String {
    format!(
        "[project]\nname = \"app\"\nversion = \"0.1.0\"\njdk = \"{feature}\"\n\n[package]\nframework = \"{framework}\"\n\n[dependencies]\ndep = \"{dependency}\"\n"
    )
}

impl Fixture {
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
}

#[test]
fn quarkus_run_is_refused_before_execution() {
    let fixture = setup(
        &manifest("25", "quarkus", "io.quarkus:quarkus-core:=3.20.0"),
        spec("io.quarkus", "quarkus-core", "3.20.0"),
    );
    fixture.ok(&["lock"]);
    let failed = fixture.fail(&["run"]);
    assert!(
        failed.contains("before augmentation"),
        "Quarkus needs its refusal, not a JVM launch: {failed}"
    );
}

#[test]
fn imported_quarkus_pom_is_a_quarkus_app_at_run() {
    let fixture = setup_bare(spec("io.quarkus", "quarkus-core", "3.20.0"));
    std::fs::write(
        fixture.project.path().join("pom.xml"),
        "<project>\n  <modelVersion>4.0.0</modelVersion>\n  <groupId>com.example</groupId>\n  <artifactId>quarkus-app</artifactId>\n  <version>1.0.0</version>\n  <properties>\n    <maven.compiler.release>17</maven.compiler.release>\n  </properties>\n  <dependencies>\n    <dependency>\n      <groupId>io.quarkus</groupId>\n      <artifactId>quarkus-core</artifactId>\n      <version>3.20.0</version>\n    </dependency>\n  </dependencies>\n</project>\n",
    )
    .expect("pom written");
    let imported = fixture.ok(&["import", "pom.xml"]);
    assert!(
        imported.contains("detected framework"),
        "the import must say the framework was detected: {imported}"
    );
    let manifest = std::fs::read_to_string(fixture.project.path().join("project.toml"))
        .expect("manifest written");
    assert!(
        manifest.contains("[package]\nframework = \"quarkus\""),
        "the generated manifest carries the framework: {manifest}"
    );
    fixture.ok(&["lock"]);
    let failed = fixture.fail(&["run"]);
    assert!(
        failed.contains("before augmentation"),
        "an imported Quarkus app needs the refusal, not a JVM launch: {failed}"
    );
}

#[test]
fn spring_boot_4_packaging_needs_a_modern_jdk() {
    let fixture = setup(
        &manifest(
            "11",
            "spring-boot",
            "org.springframework.boot:spring-boot-loader:=4.0.0",
        ),
        spec("org.springframework.boot", "spring-boot-loader", "4.0.0"),
    );
    fixture.ok(&["lock"]);
    let failed = fixture.fail(&["package"]);
    assert!(
        failed.contains("JDK 17"),
        "Boot 4 must fail on its baseline before compiling: {failed}"
    );
}

#[test]
fn vertx_packaging_needs_a_verticle() {
    let fixture = setup(
        &manifest("25", "vertx", "io.vertx:vertx-core:=4.5.0"),
        spec("io.vertx", "vertx-core", "4.5.0"),
    );
    fixture.ok(&["lock"]);
    let failed = fixture.fail(&["package"]);
    assert!(
        failed.contains("main verticle"),
        "a launcher entry cannot be inferred: {failed}"
    );
}

#[test]
fn executable_frameworks_are_not_images() {
    let fixture = setup(
        &manifest("25", "micronaut", "ut:app:=1.0"),
        spec("ut", "app", "1.0"),
    );
    fixture.ok(&["lock"]);
    let failed = fixture.fail(&["image"]);
    assert!(
        failed.contains("thin `target/dist` layout"),
        "an image needs a thin jar, not a merged executable: {failed}"
    );
}

#[test]
fn javafx_packaging_needs_a_launcher() {
    let fixture = setup(
        &manifest("25", "javafx", "org.openjfx:javafx-controls:=21.0.6"),
        spec("org.openjfx", "javafx-controls", "21.0.6"),
    );
    fixture.ok(&["lock"]);
    let failed = fixture.fail(&["package"]);
    assert!(
        failed.contains("launcher class"),
        "an Application subclass is not enough: {failed}"
    );
}
