//! End-to-end tests of the effective-POM loader
//! ([`jx_maven::PomLoader`]) against a fake Maven remote: parent-chain
//! fetching and merging, nearest-defined coordinates, `${...}`
//! interpolation (including `project.version` resolving to the child inside
//! inherited fragments), BOM imports with recursion and cycles, and the
//! management-table's effect on dependency versions.
//!
//! ADR 0003 fixes the order: inheritance merge **before** interpolation,
//! then management consolidation with imports expanded — these tests pin
//! that order from the consumer side.

use jx_common::config::{Repository, Settings};
use jx_maven::{Client, Coordinate, PomLoader};
use jx_testsupport::{FakeRepo, ResponseSpec};

const PARENT_A: &str = r#"
<project>
  <groupId>ut</groupId>
  <artifactId>p</artifactId>
  <version>1.0</version>
  <properties>
    <x.v>2.3.1</x.v>
  </properties>
  <dependencyManagement>
    <dependencies>
      <dependency>
        <groupId>ut</groupId>
        <artifactId>lib</artifactId>
        <version>${x.v}</version>
      </dependency>
    </dependencies>
  </dependencyManagement>
</project>
"#;

/// The child names no groupId (inherited) and no dependency version
/// (managed). Path: ut/c/1.0/c-1.0.pom.
const CHILD_A: &str = r#"
<project>
  <parent>
    <groupId>ut</groupId>
    <artifactId>p</artifactId>
    <version>1.0</version>
  </parent>
  <artifactId>c</artifactId>
  <dependencies>
    <dependency>
      <groupId>ut</groupId>
      <artifactId>lib</artifactId>
    </dependency>
  </dependencies>
</project>
"#;

/// `${project.version}` inside a **parent's** management entry must resolve
/// to the child's version: inheritance merges before interpolation (ADR
/// 0003). Path: ut/p2/1.0/p2-1.0.pom.
const PARENT_B: &str = r#"
<project>
  <groupId>ut</groupId>
  <artifactId>p2</artifactId>
  <version>1.0</version>
  <dependencyManagement>
    <dependencies>
      <dependency>
        <groupId>ut</groupId>
        <artifactId>dep</artifactId>
        <version>${project.version}</version>
      </dependency>
    </dependencies>
  </dependencyManagement>
</project>
"#;

const CHILD_B: &str = r#"
<project>
  <parent>
    <groupId>ut</groupId>
    <artifactId>p2</artifactId>
    <version>1.0</version>
  </parent>
  <artifactId>c2</artifactId>
  <version>2.0</version>
  <dependencies>
    <dependency>
      <groupId>ut</groupId>
      <artifactId>dep</artifactId>
    </dependency>
  </dependencies>
</project>
"#;

/// A BOM: packaging `pom`, one managed coordinate. Path:
/// ut/bom/1.0/bom-1.0.pom.
const BOM: &str = r#"
<project>
  <groupId>ut</groupId>
  <artifactId>bom</artifactId>
  <version>1.0</version>
  <packaging>pom</packaging>
  <dependencyManagement>
    <dependencies>
      <dependency>
        <groupId>com.x</groupId>
        <artifactId>x</artifactId>
        <version>10.0</version>
      </dependency>
    </dependencies>
  </dependencyManagement>
</project>
"#;

const APP: &str = r#"
<project>
  <groupId>ut</groupId>
  <artifactId>app</artifactId>
  <version>1.0</version>
  <dependencyManagement>
    <dependencies>
      <dependency>
        <groupId>ut</groupId>
        <artifactId>bom</artifactId>
        <version>1.0</version>
        <type>pom</type>
        <scope>import</scope>
      </dependency>
    </dependencies>
  </dependencyManagement>
  <dependencies>
    <dependency>
      <groupId>com.x</groupId>
      <artifactId>x</artifactId>
    </dependency>
  </dependencies>
</project>
"#;

/// The child's parent does not exist on the remote.
const ORPHAN: &str = r#"
<project>
  <parent>
    <groupId>ut</groupId>
    <artifactId>missing</artifactId>
    <version>9.9</version>
  </parent>
  <artifactId>orphan</artifactId>
</project>
"#;

const CHILD_WITH_NO_VERSIONED_DEP: &str = r#"
<project>
  <groupId>ut</groupId>
  <artifactId>noversion</artifactId>
  <version>1.0</version>
  <dependencies>
    <dependency>
      <groupId>ut</groupId>
      <artifactId>nomanagement</artifactId>
    </dependency>
  </dependencies>
</project>
"#;

/// Self-import: `ut:loop:1.0`'s management imports itself.
const LOOPY_BOM: &str = r#"
<project>
  <groupId>ut</groupId>
  <artifactId>loop</artifactId>
  <version>1.0</version>
  <packaging>pom</packaging>
  <dependencyManagement>
    <dependencies>
      <dependency>
        <groupId>ut</groupId>
        <artifactId>loop</artifactId>
        <version>1.0</version>
        <type>pom</type>
        <scope>import</scope>
      </dependency>
    </dependencies>
  </dependencyManagement>
</project>
"#;

fn pom(path: &str, text: &str) -> (String, ResponseSpec) {
    (
        path.to_string(),
        ResponseSpec::Sha256(text.as_bytes().to_vec()),
    )
}

struct Harness {
    _home: tempfile::TempDir,
    _repo: FakeRepo,
    client: Client,
}

fn harness(specs: Vec<(String, ResponseSpec)>) -> Harness {
    let repo = FakeRepo::spawn(specs);
    let mut settings = Settings::default();
    settings.repositories.push(Repository {
        id: "central".to_string(),
        url: repo.url(),
        auth: None,
    });
    let home = tempfile::tempdir().expect("temp jx home");
    let client = Client::new(home.path(), settings, false).expect("client built");
    Harness {
        _home: home,
        _repo: repo,
        client,
    }
}

fn coord(text: &str) -> Coordinate {
    Coordinate::parse(text).expect("literal coordinate")
}

#[test]
fn parent_supplies_group_and_managed_versions() {
    let t = harness(vec![
        pom("ut/p/1.0/p-1.0.pom", PARENT_A),
        pom("ut/c/1.0/c-1.0.pom", CHILD_A),
    ]);
    let effective = PomLoader::new(&t.client)
        .load(&coord("ut:c:1.0"))
        .expect("effective POM");
    // Coordinates: the child's own artifactId, the parent's group + version.
    assert_eq!(effective.coords.group_id, "ut");
    assert_eq!(effective.coords.artifact_id, "c");
    assert_eq!(effective.coords.version, "1.0");
    // The property expanded inside the management entry, and the dependency
    // picked its version up from management.
    assert_eq!(
        effective
            .managed("ut", "lib")
            .and_then(|d| d.version.clone()),
        Some("2.3.1".to_string())
    );
    assert_eq!(effective.dependencies.len(), 1);
    assert_eq!(effective.dependencies[0].version.as_deref(), Some("2.3.1"));
}

#[test]
fn local_pom_with_unrelated_relative_parent_falls_back_to_the_repository() {
    let t = harness(vec![pom("ut/p/1.0/p-1.0.pom", PARENT_A)]);
    let root = tempfile::tempdir().expect("project dir");
    let app = root.path().join("app");
    std::fs::create_dir_all(&app).unwrap();
    std::fs::write(
        root.path().join("pom.xml"),
        "<project><groupId>other</groupId><artifactId>unrelated</artifactId><version>9</version></project>",
    )
    .unwrap();
    std::fs::write(
        app.join("pom.xml"),
        r#"<project>
  <parent><groupId>ut</groupId><artifactId>p</artifactId><version>1.0</version></parent>
  <artifactId>local-app</artifactId>
</project>"#,
    )
    .unwrap();

    let effective = PomLoader::new(&t.client)
        .load_file(&app.join("pom.xml"))
        .expect("remote parent after local mismatch");
    assert_eq!(effective.coords.group_id, "ut");
    assert_eq!(effective.coords.artifact_id, "local-app");
    assert_eq!(effective.coords.version, "1.0");
    assert_eq!(
        effective
            .managed("ut", "lib")
            .and_then(|dependency| dependency.version.as_deref()),
        Some("2.3.1"),
        "the remote parent's management table was merged"
    );
}

#[test]
fn parent_project_version_resolves_to_the_child() {
    let t = harness(vec![
        pom("ut/p2/1.0/p2-1.0.pom", PARENT_B),
        pom("ut/c2/2.0/c2-2.0.pom", CHILD_B),
    ]);
    let effective = PomLoader::new(&t.client)
        .load(&coord("ut:c2:2.0"))
        .expect("effective POM");
    assert_eq!(
        effective
            .managed("ut", "dep")
            .and_then(|d| d.version.clone()),
        Some("2.0".to_string()),
        "${{project.version}} in the parent's management entry must name the child"
    );
    assert_eq!(effective.dependencies[0].version.as_deref(), Some("2.0"));
}

#[test]
fn bom_import_supplies_the_dependency_version() {
    let t = harness(vec![
        pom("ut/bom/1.0/bom-1.0.pom", BOM),
        pom("ut/app/1.0/app-1.0.pom", APP),
    ]);
    let effective = PomLoader::new(&t.client)
        .load(&coord("ut:app:1.0"))
        .expect("effective POM");
    // The BOM's entry was spliced into the consolidated management table...
    assert_eq!(
        effective
            .managed("com.x", "x")
            .and_then(|d| d.version.clone()),
        Some("10.0".to_string())
    );
    // ...and the bare dependency picked the version up.
    assert_eq!(effective.dependencies[0].version.as_deref(), Some("10.0"));
}

#[test]
fn import_without_the_pom_type_is_rejected() {
    let no_type = r#"
<project>
  <groupId>ut</groupId>
  <artifactId>notyp</artifactId>
  <version>1.0</version>
  <dependencyManagement>
    <dependencies>
      <dependency>
        <groupId>ut</groupId>
        <artifactId>bom</artifactId>
        <version>1.0</version>
        <scope>import</scope>
      </dependency>
    </dependencies>
  </dependencyManagement>
</project>
"#;
    let t = harness(vec![
        pom("ut/bom/1.0/bom-1.0.pom", BOM),
        pom("ut/notyp/1.0/notyp-1.0.pom", no_type),
    ]);
    let error = PomLoader::new(&t.client)
        .load(&coord("ut:notyp:1.0"))
        .expect_err("type-less import must be diagnosed");
    assert!(error.message.contains("<type>pom</type>"), "{error}");
}

#[test]
fn self_importing_bom_is_a_diagnosed_cycle() {
    let t = harness(vec![pom("ut/loop/1.0/loop-1.0.pom", LOOPY_BOM)]);
    let error = PomLoader::new(&t.client)
        .load(&coord("ut:loop:1.0"))
        .expect_err("a BOM importing itself must stop");
    assert!(error.message.contains("cycle"), "{error}");
    assert_eq!(
        error.location.as_deref(),
        Some("ut:loop:1.0"),
        "the diagnostic names the cycling BOM coordinate"
    );
}

#[test]
fn missing_parent_pom_is_diagnosed_with_the_chain_context() {
    let t = harness(vec![pom("ut/orphan/1.0/orphan-1.0.pom", ORPHAN)]);
    let error = PomLoader::new(&t.client)
        .load(&coord("ut:orphan:1.0"))
        .expect_err("the parent does not exist remotely");
    assert!(error.message.contains("artifact not found"), "{error}");
}

#[test]
fn version_dependency_without_any_management_stays_none() {
    let t = harness(vec![pom(
        "ut/noversion/1.0/noversion-1.0.pom",
        CHILD_WITH_NO_VERSIONED_DEP,
    )]);
    let effective = PomLoader::new(&t.client)
        .load(&coord("ut:noversion:1.0"))
        .expect("effective POM");
    assert_eq!(effective.dependencies.len(), 1);
    assert_eq!(
        effective.dependencies[0].version, None,
        "no management anywhere supplies a version; the resolver diagnoses it against the resolution coordinate"
    );
}

#[test]
fn repositories_from_the_chain_merge_parents_first() {
    let parent_with_repo = r#"
<project>
  <groupId>ut</groupId>
  <artifactId>rp</artifactId>
  <version>1.0</version>
  <repositories>
    <repository>
      <id>parent-repo</id>
      <url>https://parent.example/maven</url>
    </repository>
  </repositories>
</project>
"#;
    let child_with_repo = r#"
<project>
  <parent>
    <groupId>ut</groupId>
    <artifactId>rp</artifactId>
    <version>1.0</version>
  </parent>
  <artifactId>rc</artifactId>
  <repositories>
    <repository>
      <id>child-repo</id>
      <url>https://child.example/maven</url>
    </repository>
    <repository>
      <id>parent-repo</id>
      <url>https://parent.example/maven</url>
    </repository>
  </repositories>
</project>
"#;
    let t = harness(vec![
        pom("ut/rp/1.0/rp-1.0.pom", parent_with_repo),
        pom("ut/rc/1.0/rc-1.0.pom", child_with_repo),
    ]);
    let effective = PomLoader::new(&t.client)
        .load(&coord("ut:rc:1.0"))
        .expect("effective POM");
    let ids: Vec<&str> = effective
        .repositories
        .iter()
        .map(|r| r.id.as_str())
        .collect();
    assert_eq!(
        ids,
        ["parent-repo", "child-repo"],
        "parent first, then the child's own; duplicate parent entries are not repeated"
    );
}
