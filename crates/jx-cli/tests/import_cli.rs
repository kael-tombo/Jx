//! End-to-end tests of `jx import` against local builds. No network is used:
//! every parent POM is on disk and every dependency carries a version.

use std::path::Path;
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

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_jx"))
            .args(["--color", "never", "--offline"])
            .args(args)
            .current_dir(self.project.path())
            .env("JX_HOME", self.home.path())
            .env("NO_COLOR", "1")
            .output()
            .expect("jx launches")
    }
}

fn write_local_parent_chain(root: &Path) {
    std::fs::create_dir_all(root.join("app")).unwrap();
    std::fs::write(
        root.join("pom.xml"),
        r#"<project>
  <modelVersion>4.0.0</modelVersion>
  <groupId>com.example</groupId>
  <artifactId>parent</artifactId>
  <version>1.0.0</version>
  <packaging>pom</packaging>
  <properties>
    <maven.compiler.release>17</maven.compiler.release>
    <lombok.version>1.18.36</lombok.version>
  </properties>
  <build>
    <plugins>
      <plugin>
        <groupId>org.apache.maven.plugins</groupId>
        <artifactId>maven-compiler-plugin</artifactId>
        <configuration>
          <annotationProcessorPaths>
            <path>
              <groupId>org.projectlombok</groupId>
              <artifactId>lombok</artifactId>
              <version>${lombok.version}</version>
            </path>
          </annotationProcessorPaths>
        </configuration>
      </plugin>
    </plugins>
  </build>
  <dependencyManagement>
    <dependencies>
      <dependency>
        <groupId>com.google.guava</groupId>
        <artifactId>guava</artifactId>
        <version>33.4.8-jre</version>
      </dependency>
    </dependencies>
  </dependencyManagement>
</project>"#,
    )
    .unwrap();
    std::fs::write(
        root.join("app/pom.xml"),
        r#"<project>
  <modelVersion>4.0.0</modelVersion>
  <parent>
    <groupId>com.example</groupId>
    <artifactId>parent</artifactId>
    <version>1.0.0</version>
    <relativePath>../pom.xml</relativePath>
  </parent>
  <artifactId>app</artifactId>
  <dependencies>
    <dependency>
      <groupId>com.google.guava</groupId>
      <artifactId>guava</artifactId>
    </dependency>
    <dependency>
      <groupId>org.junit.jupiter</groupId>
      <artifactId>junit-jupiter</artifactId>
      <version>5.11.4</version>
      <scope>test</scope>
    </dependency>
    <dependency>
      <groupId>jakarta.servlet</groupId>
      <artifactId>jakarta.servlet-api</artifactId>
      <version>6.0.0</version>
      <scope>provided</scope>
    </dependency>
  </dependencies>
</project>"#,
    )
    .unwrap();
}

#[test]
fn import_uses_local_parent_management_and_writes_a_valid_manifest() {
    let fixture = Fixture::new();
    write_local_parent_chain(fixture.path());

    let output = fixture.run(&["import", "app/pom.xml"]);
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("imported"), "{stdout}");
    assert!(stdout.contains("note:"), "{stdout}");
    assert!(stdout.contains("provided"), "{stdout}");

    let manifest_path = fixture.path().join("project.toml");
    let manifest = jx_manifest::parse(&manifest_path).expect("generated manifest parses");
    assert_eq!(manifest.name, "app");
    assert_eq!(manifest.version, "1.0.0");
    assert_eq!(manifest.jdk, "17");
    assert_eq!(manifest.dependencies.len(), 2);
    let guava = manifest
        .dependencies
        .iter()
        .find(|dependency| dependency.artifact_id == "guava")
        .expect("guava imported");
    assert_eq!(guava.group_id, "com.google.guava");
    assert_eq!(
        guava
            .wanted
            .as_ref()
            .map(jx_manifest::WantedVersion::as_str),
        Some("33.4.8-jre")
    );
    assert!(
        manifest
            .dependencies
            .iter()
            .any(|dependency| dependency.artifact_id == "jakarta.servlet-api"),
        "provided scope is imported with a note, not dropped"
    );
    assert_eq!(manifest.dev_dependencies.len(), 1);
    // The parent POM's compiler-plugin processor path is inherited and its
    // ${lombok.version} property interpolated before it reaches [processors].
    assert_eq!(manifest.processors.len(), 1);
    assert_eq!(manifest.processors[0].artifact_id, "lombok");
    assert_eq!(
        manifest.processors[0]
            .wanted
            .as_ref()
            .map(jx_manifest::WantedVersion::as_str),
        Some("1.18.36")
    );
    assert_eq!(manifest.dev_dependencies[0].artifact_id, "junit-jupiter");
}

#[test]
fn existing_manifest_needs_force_but_dry_run_still_prints() {
    let fixture = Fixture::new();
    write_local_parent_chain(fixture.path());
    let manifest_path = fixture.path().join("project.toml");
    std::fs::write(&manifest_path, "sentinel").unwrap();

    let refused = fixture.run(&["import", "app/pom.xml"]);
    assert!(!refused.status.success());
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("already exists"),
        "{}",
        String::from_utf8_lossy(&refused.stderr)
    );
    assert_eq!(std::fs::read_to_string(&manifest_path).unwrap(), "sentinel");

    let dry = fixture.run(&["import", "app/pom.xml", "--dry-run"]);
    assert!(dry.status.success());
    assert!(String::from_utf8_lossy(&dry.stdout).contains("[project]"));
    assert_eq!(std::fs::read_to_string(&manifest_path).unwrap(), "sentinel");

    let forced = fixture.run(&["import", "app/pom.xml", "--force"]);
    assert!(forced.status.success());
    let written = std::fs::read_to_string(&manifest_path).unwrap();
    assert!(written.contains("name = \"app\""), "{written}");
    assert_ne!(written, "sentinel");
}

#[test]
fn gradle_build_is_detected_and_imported() {
    let fixture = Fixture::new();
    std::fs::write(
        fixture.path().join("settings.gradle.kts"),
        "rootProject.name = \"catalog\"\n",
    )
    .unwrap();
    std::fs::write(
        fixture.path().join("build.gradle.kts"),
        r#"
plugins {
    application
}

version = "2.1.0"

java {
    toolchain {
        languageVersion = JavaLanguageVersion.of(21)
    }
}

dependencies {
    implementation("com.google.guava:guava:33.4.8-jre")
    annotationProcessor("org.example:processor:1.0")
    testImplementation("org.junit.jupiter:junit-jupiter:5.11.4")
}

application {
    mainClass = "com.example.Catalog"
}
"#,
    )
    .unwrap();

    let output = fixture.run(&["import"]);
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("build.gradle.kts"), "{stdout}");

    let manifest = jx_manifest::parse(&fixture.path().join("project.toml"))
        .expect("generated manifest parses");
    assert_eq!(manifest.name, "catalog");
    assert_eq!(manifest.version, "2.1.0");
    assert_eq!(manifest.jdk, "21");
    assert_eq!(manifest.main.as_deref(), Some("com.example.Catalog"));
    assert_eq!(manifest.dependencies.len(), 1);
    assert_eq!(manifest.dev_dependencies.len(), 1);
    assert_eq!(manifest.processors.len(), 1);
    assert_eq!(manifest.processors[0].artifact_id, "processor");
    assert_eq!(
        manifest.processors[0]
            .wanted
            .as_ref()
            .map(jx_manifest::WantedVersion::as_str),
        Some("1.0")
    );
    assert_eq!(
        manifest.dependencies[0]
            .wanted
            .as_ref()
            .map(jx_manifest::WantedVersion::as_str),
        Some("33.4.8-jre")
    );
}

#[test]
fn gradle_dynamic_version_fails_without_writing() {
    let fixture = Fixture::new();
    std::fs::write(
        fixture.path().join("build.gradle"),
        r#"
dependencies {
    implementation "com.google.guava:guava:$guavaVersion"
}
"#,
    )
    .unwrap();

    let output = fixture.run(&["import"]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("interpolates"), "{stderr}");
    assert!(!fixture.path().join("project.toml").exists());
}

#[test]
fn imports_a_jot_project_and_its_version_catalog() {
    let fixture = Fixture::new();
    std::fs::write(
        fixture.path().join("jot.toml"),
        r#"
[project]
name = "jot-demo"
version = "1.2.3"
main-class = "com.example.Main"
layout = "maven"

[toolchains]
java = "21"

[dependencies]
guava = "com.google.guava:guava:33.4.8-jre"

[test-dependencies]
junit = { catalog = "junit" }
"#,
    )
    .unwrap();
    std::fs::write(
        fixture.path().join("libs.versions.toml"),
        r#"
[versions]
junit = "5.11.4"

[libraries]
junit = { module = "org.junit.jupiter:junit-jupiter", version.ref = "junit" }
"#,
    )
    .unwrap();

    let output = fixture.run(&["import"]);
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("jot.toml"), "{stdout}");
    assert!(stdout.contains("libs.versions.toml"), "{stdout}");

    let manifest = jx_manifest::parse(&fixture.path().join("project.toml"))
        .expect("generated manifest parses");
    assert_eq!(manifest.name, "jot-demo");
    assert_eq!(manifest.version, "1.2.3");
    assert_eq!(manifest.jdk, "21");
    assert_eq!(manifest.main.as_deref(), Some("com.example.Main"));
    assert_eq!(manifest.dependencies.len(), 1);
    assert_eq!(manifest.dev_dependencies.len(), 1);
    assert_eq!(
        manifest.dev_dependencies[0]
            .wanted
            .as_ref()
            .map(jx_manifest::WantedVersion::as_str),
        Some("5.11.4")
    );
}

#[test]
fn imports_the_real_jot_java_web_sample_processors() {
    let fixture = Fixture::new();
    std::fs::write(
        fixture.path().join("jot.toml"),
        r#"
[project]
name = "webserver"
version = "0.1.0"
main-class = "com.jot.samples.webserver.ServerMain"
layout = "maven"
[toolchains]
java = "21"
[dependencies]
mapstruct = "org.mapstruct:mapstruct:1.6.3"
[processors]
mapstruct = "org.mapstruct:mapstruct-processor:1.6.3"
[test-dependencies]
junit = "org.junit.jupiter:junit-jupiter:5.11.0"
"#,
    )
    .unwrap();

    let output = fixture.run(&["import"]);
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("1 processors"), "{stdout}");

    let manifest = jx_manifest::parse(&fixture.path().join("project.toml"))
        .expect("generated manifest parses");
    assert_eq!(manifest.dependencies.len(), 1);
    assert_eq!(manifest.dev_dependencies.len(), 1);
    assert_eq!(manifest.processors.len(), 1);
    assert_eq!(manifest.processors[0].group_id, "org.mapstruct");
    assert_eq!(manifest.processors[0].artifact_id, "mapstruct-processor");
    assert_eq!(
        manifest.processors[0]
            .wanted
            .as_ref()
            .map(jx_manifest::WantedVersion::as_str),
        Some("1.6.3")
    );
}

#[test]
fn jot_workspace_member_uses_the_root_catalog() {
    let fixture = Fixture::new();
    std::fs::write(
        fixture.path().join("jot.toml"),
        "[workspace]\nmembers = [\"api\"]\n\n[toolchains]\njava = \"21\"\n",
    )
    .unwrap();
    std::fs::write(
        fixture.path().join("libs.versions.toml"),
        r#"
[versions]
guava = "33.4.8-jre"

[libraries]
guava = { module = "com.google.guava:guava", version.ref = "guava" }
"#,
    )
    .unwrap();
    std::fs::create_dir_all(fixture.path().join("api")).unwrap();
    std::fs::write(
        fixture.path().join("api/jot.toml"),
        r#"
[project]
name = "api"
version = "1.0.0"
layout = "maven"

[toolchains]
java = "21"

[dependencies]
guava = { catalog = "guava" }
"#,
    )
    .unwrap();

    let output = fixture.run(&["import", "api/jot.toml"]);
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("libs.versions.toml"), "{stdout}");

    let manifest = jx_manifest::parse(&fixture.path().join("project.toml"))
        .expect("generated manifest parses");
    assert_eq!(manifest.name, "api");
    assert_eq!(manifest.dependencies.len(), 1);
    assert_eq!(
        manifest.dependencies[0]
            .wanted
            .as_ref()
            .map(jx_manifest::WantedVersion::as_str),
        Some("33.4.8-jre")
    );
}

#[test]
fn unversioned_dependency_fails_without_writing() {
    let fixture = Fixture::new();
    std::fs::write(
        fixture.path().join("pom.xml"),
        r#"<project>
  <modelVersion>4.0.0</modelVersion>
  <groupId>com.example</groupId>
  <artifactId>broken</artifactId>
  <version>1.0</version>
  <dependencies>
    <dependency><groupId>g</groupId><artifactId>no-version</artifactId></dependency>
  </dependencies>
</project>"#,
    )
    .unwrap();

    let output = fixture.run(&["import"]);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("has no version"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!fixture.path().join("project.toml").exists());
}
