//! Integration tests of the resolver against a fake Maven remote: nearest-
//! wins conflict reporting with subtree alignment, BOM-supplied versions,
//! prefix selection with pre-release exclusion, overrides and exclusion
//! rules. Every test drives a real `[Client]` + `[PomLoader]` pair like the
//! CLI will.

use std::collections::HashMap;

use jx_common::config::{Repository, Settings};
use jx_manifest::deps::{DependencySpec, OverrideSpec, WantedVersion};
use jx_maven::{Client, PomLoader};
use jx_resolve::resolve;
use jx_testsupport::{FakeRepo, ResponseSpec};

/// A minimal POM text for the spec table.
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

struct Harness {
    _home: tempfile::TempDir,
    _repo: FakeRepo,
    client: Client,
}

impl Harness {
    fn serve(specs: Vec<(String, ResponseSpec)>) -> Harness {
        let repo = FakeRepo::spawn(specs);
        let mut settings = Settings::default();
        settings.repositories.push(Repository {
            id: "fake".to_string(),
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

    fn loader(&self) -> PomLoader<'_> {
        PomLoader::new(&self.client)
    }
}

fn dep(alias: &str, coordinate: &str) -> DependencySpec {
    let (group_id, artifact_id, version) = {
        let parts: Vec<&str> = coordinate.split(':').collect();
        (
            parts[0].to_string(),
            parts[1].to_string(),
            parts.get(2).map(|v| v.to_string()),
        )
    };
    let wanted = version.map(|text| match text.strip_prefix('=') {
        Some(pinned) => WantedVersion::Exact(pinned.to_string()),
        None => WantedVersion::Prefix(text),
    });
    DependencySpec {
        alias: alias.to_string(),
        group_id,
        artifact_id,
        classifier: None,
        wanted,
        platform: false,
    }
}

fn override_spec(key: &str, version: &str) -> OverrideSpec {
    let mut parts = key.split(':');
    let group_id = parts.next().expect("group").to_string();
    let artifact_id = parts.next().expect("artifact").to_string();
    OverrideSpec {
        group_id,
        artifact_id,
        wanted: WantedVersion::Exact(version.to_string()),
    }
}

fn versions(meta_list: &[&str]) -> Vec<u8> {
    let mut text = String::from("<metadata><versioning><versions>");
    for version in meta_list {
        text.push_str(&format!("<version>{version}</version>"));
    }
    text.push_str("</versions></versioning></metadata>");
    text.into_bytes()
}

/// app depends on lib 1.0 and mid 1.0; mid depends on lib 2.0:
/// nearest wins (lib 1.0), fork recorded.
#[test]
fn nearest_wins_and_the_fork_is_reported() {
    let specs = vec![
        (
            "ut/app/1.0/app-1.0.pom".to_string(),
            ResponseSpec::Sha256(pom(r#"
  <groupId>ut</groupId>
  <artifactId>app</artifactId>
  <version>1.0</version>
  <dependencies>
    <dependency>
      <groupId>ut</groupId>
      <artifactId>lib</artifactId>
      <version>1.0</version>
    </dependency>
    <dependency>
      <groupId>ut</groupId>
      <artifactId>mid</artifactId>
      <version>1.0</version>
    </dependency>
  </dependencies>"#)),
        ),
        (
            "ut/mid/1.0/mid-1.0.pom".to_string(),
            ResponseSpec::Sha256(pom(r#"
  <groupId>ut</groupId>
  <artifactId>mid</artifactId>
  <version>1.0</version>
  <dependencies>
    <dependency>
      <groupId>ut</groupId>
      <artifactId>lib</artifactId>
      <version>2.0</version>
    </dependency>
  </dependencies>"#)),
        ),
        (
            "ut/lib/1.0/lib-1.0.pom".to_string(),
            ResponseSpec::Sha256(pom(
                "<groupId>ut</groupId>\n  <artifactId>lib</artifactId>\n  <version>1.0</version>",
            )),
        ),
        (
            "ut/lib/2.0/lib-2.0.pom".to_string(),
            ResponseSpec::Sha256(pom(
                "<groupId>ut</groupId>\n  <artifactId>lib</artifactId>\n  <version>2.0</version>",
            )),
        ),
    ];
    let harness = Harness::serve(specs);

    let resolution = resolve(
        &harness.loader(),
        &harness.client,
        &[dep("app", "ut:app:=1.0")],
        &[],
        &[],
    )
    .expect("resolved");

    let mut seen: HashMap<(String, String), String> = HashMap::new();
    for node in &resolution.nodes {
        seen.insert(
            (node.group_id.clone(), node.artifact_id.clone()),
            node.version.clone(),
        );
    }
    assert_eq!(seen.get(&("ut".into(), "lib".into())).unwrap(), "1.0");
    assert_eq!(seen.get(&("ut".into(), "mid".into())).unwrap(), "1.0");
    assert_eq!(resolution.conflicts.len(), 1, "the 2.0 fork is visible");
    let conflict = &resolution.conflicts[0];
    assert_eq!(conflict.winner_version, "1.0");
    assert_eq!(conflict.winner_depth, 2);
    assert_eq!(conflict.loser_version, "2.0");
    assert_eq!(conflict.loser_depth, 3);

    // The parent edges are what `jx tree`/`jx why` walk. Root edges have no
    // parent and are keyed by identity, never by version.
    assert!(
        resolution
            .edges
            .iter()
            .any(|edge| edge.parent.is_none() && edge.key() == "ut:app")
    );
    assert!(
        resolution
            .edges
            .iter()
            .any(|edge| { edge.parent.as_deref() == Some("ut:app") && edge.key() == "ut:lib" })
    );
    assert!(
        resolution
            .edges
            .iter()
            .any(|edge| { edge.parent.as_deref() == Some("ut:app") && edge.key() == "ut:mid" })
    );
    assert!(
        resolution
            .edges
            .iter()
            .any(|edge| { edge.parent.as_deref() == Some("ut:mid") && edge.key() == "ut:lib" })
    );
}

/// ut:bom manages ut:lib 3.1; the manifest's versionless `lib` takes that.
#[test]
fn a_platform_bom_supplies_versionless_entries() {
    let mut lib = dep("lib", "ut:lib");
    lib.wanted = None;
    let mut bom = dep("jackson", "ut:bom:1");
    bom.platform = true;
    let specs = vec![
        (
            "ut/bom/1/bom-1.pom".to_string(),
            ResponseSpec::Sha256(pom(r#"
  <groupId>ut</groupId>
  <artifactId>bom</artifactId>
  <version>1</version>
  <packaging>pom</packaging>
  <dependencyManagement>
    <dependencies>
      <dependency>
        <groupId>ut</groupId>
        <artifactId>lib</artifactId>
        <version>3.1</version>
      </dependency>
    </dependencies>
  </dependencyManagement>"#)),
        ),
        (
            "ut/lib/3.1/lib-3.1.pom".to_string(),
            ResponseSpec::Sha256(pom(
                "<groupId>ut</groupId>\n  <artifactId>lib</artifactId>\n  <version>3.1</version>",
            )),
        ),
        // The BOM's own wanted version is a prefix, so the resolver reads
        // its listing like any other wanted pin.
        (
            "ut/bom/maven-metadata.xml".to_string(),
            ResponseSpec::Sha256(versions(&["1"])),
        ),
    ];
    let harness = Harness::serve(specs);

    let resolution =
        resolve(&harness.loader(), &harness.client, &[bom, lib], &[], &[]).expect("resolved");
    let lib = resolution
        .nodes
        .iter()
        .find(|n| n.artifact_id == "lib")
        .expect("lib resolved");
    assert_eq!(lib.version, "3.1");
    // The BOM itself supplies versions only: it must not appear as a node.
    assert!(
        !resolution.nodes.iter().any(|n| n.artifact_id == "bom"),
        "the BOM is not a build artifact"
    );
}

/// wanted prefix `2` on ut:lib whose listing offers 2.0, 2.0.5, 2.1-RC1:
/// 2.0.5 wins (pre-release excluded), then the walk proceeds.
#[test]
fn prefix_expansion_reads_the_version_listing() {
    let specs = vec![
        (
            "ut/lib/maven-metadata.xml".to_string(),
            ResponseSpec::Sha256(versions(&["1.9", "2.0", "2.0.5", "2.1-RC1"])),
        ),
        (
            "ut/lib/2.0.5/lib-2.0.5.pom".to_string(),
            ResponseSpec::Sha256(pom(
                "<groupId>ut</groupId>\n  <artifactId>lib</artifactId>\n  <version>2.0.5</version>\n  <dependencies>\n    <dependency>\n      <groupId>ut</groupId>\n      <artifactId>helper</artifactId>\n      <version>1.0</version>\n    </dependency>\n  </dependencies>",
            )),
        ),
        (
            "ut/helper/1.0/helper-1.0.pom".to_string(),
            ResponseSpec::Sha256(pom(
                "<groupId>ut</groupId>\n  <artifactId>helper</artifactId>\n  <version>1.0</version>",
            )),
        ),
    ];
    let harness = Harness::serve(specs);

    let resolution = resolve(
        &harness.loader(),
        &harness.client,
        &[dep("lib", "ut:lib:2")],
        &[],
        &[],
    )
    .expect("resolved");
    let lib = resolution
        .nodes
        .iter()
        .find(|n| n.artifact_id == "lib")
        .expect("lib present");
    assert_eq!(lib.version, "2.0.5");
    assert!(resolution.nodes.iter().any(|n| n.artifact_id == "helper"));
}

/// No stable version extends the prefix: the report says what is offered.
#[test]
fn a_prefix_with_only_pre_releases_fails_naming_them() {
    let specs = vec![(
        "ut/lib/maven-metadata.xml".to_string(),
        ResponseSpec::Sha256(versions(&["1.0", "2.0-M1", "2.0-beta1"])),
    )];
    let harness = Harness::serve(specs);

    let error = resolve(
        &harness.loader(),
        &harness.client,
        &[dep("lib", "ut:lib:2")],
        &[],
        &[],
    )
    .expect_err("no candidate exists");
    assert!(error.message.contains("no stable version"), "{error}");
    assert!(
        error
            .hint
            .as_deref()
            .is_some_and(|hint| hint.contains("2.0-M1, 2.0-beta1")),
        "{error}"
    );
}

/// An override replaces the declared version everywhere and is reported.
#[test]
fn overrides_apply_and_replace() {
    let specs = vec![
        (
            "ut/app/1.0/app-1.0.pom".to_string(),
            ResponseSpec::Sha256(pom(r#"
  <groupId>ut</groupId>
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
            "ut/lib/1.5/lib-1.5.pom".to_string(),
            ResponseSpec::Sha256(pom(
                "<groupId>ut</groupId>\n  <artifactId>lib</artifactId>\n  <version>1.5</version>",
            )),
        ),
    ];
    let harness = Harness::serve(specs);

    let resolution = resolve(
        &harness.loader(),
        &harness.client,
        &[dep("app", "ut:app:=1.0")],
        &[],
        &[override_spec("ut:lib", "1.5")],
    )
    .expect("resolved");
    // lib 1.0 is replaced by 1.5 and only 1.5 is expanded.
    let lib = resolution
        .nodes
        .iter()
        .find(|n| n.artifact_id == "lib")
        .expect("lib present");
    assert_eq!(lib.version, "1.5");
    assert_eq!(resolution.replacements.len(), 1);
    assert_eq!(resolution.conflicts.len(), 0, "no fork, the pin wins");
}

/// app excludes ut:inner from lib; lib's dependence on inner resolves away.
#[test]
fn exclusions_apply_below_their_edge() {
    let specs = vec![
        (
            "ut/app/1.0/app-1.0.pom".to_string(),
            ResponseSpec::Sha256(pom(r#"
  <groupId>ut</groupId>
  <artifactId>app</artifactId>
  <version>1.0</version>
  <dependencies>
    <dependency>
      <groupId>ut</groupId>
      <artifactId>lib</artifactId>
      <version>1.0</version>
      <exclusions>
        <exclusion>
          <groupId>ut</groupId>
          <artifactId>inner</artifactId>
        </exclusion>
      </exclusions>
    </dependency>
  </dependencies>"#)),
        ),
        (
            "ut/lib/1.0/lib-1.0.pom".to_string(),
            ResponseSpec::Sha256(pom(r#"
  <groupId>ut</groupId>
  <artifactId>lib</artifactId>
  <version>1.0</version>
  <dependencies>
    <dependency>
      <groupId>ut</groupId>
      <artifactId>inner</artifactId>
      <version>7.9</version>
    </dependency>
  </dependencies>"#)),
        ),
    ];
    let harness = Harness::serve(specs);

    let resolution = resolve(
        &harness.loader(),
        &harness.client,
        &[dep("app", "ut:app:=1.0")],
        &[],
        &[],
    )
    .expect("resolved");
    assert!(
        !resolution.nodes.iter().any(|n| n.artifact_id == "inner"),
        "the excluded dependency is out of the graph"
    );
    assert!(
        resolution
            .skipped
            .iter()
            .any(|s| s.artifact_id == "inner" && s.reason.contains("excluded")),
        "the skip is named in the report"
    );
}

/// A transitive POM demanding a SNAPSHOT fails with an actionable message
/// (V1 resolves releases only — ADR 0004).
#[test]
fn transitive_snapshots_are_rejected_with_direction() {
    let specs = vec![
        (
            "ut/app/1.0/app-1.0.pom".to_string(),
            ResponseSpec::Sha256(pom(r#"
  <groupId>ut</groupId>
  <artifactId>app</artifactId>
  <version>1.0</version>
  <dependencies>
    <dependency>
      <groupId>ut</groupId>
      <artifactId>mid</artifactId>
      <version>1.0</version>
    </dependency>
  </dependencies>"#)),
        ),
        (
            "ut/mid/1.0/mid-1.0.pom".to_string(),
            ResponseSpec::Sha256(pom(r#"
  <groupId>ut</groupId>
  <artifactId>mid</artifactId>
  <version>1.0</version>
  <dependencies>
    <dependency>
      <groupId>ut</groupId>
      <artifactId>jit</artifactId>
      <version>9.9-SNAPSHOT</version>
    </dependency>
  </dependencies>"#)),
        ),
    ];
    let harness = Harness::serve(specs);

    let error = resolve(
        &harness.loader(),
        &harness.client,
        &[dep("app", "ut:app:=1.0")],
        &[],
        &[],
    )
    .expect_err("snapshots cannot resolve in V1");
    assert!(error.message.contains("SNAPSHOT"), "{error}");
}

/// Test-scoped transitive deps stay out; optional ones too — both named.
#[test]
fn test_and_optional_transitives_are_skipped_by_name() {
    let specs = vec![
        (
            "ut/app/1.0/app-1.0.pom".to_string(),
            ResponseSpec::Sha256(pom(r#"
  <groupId>ut</groupId>
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
            "ut/lib/1.0/lib-1.0.pom".to_string(),
            ResponseSpec::Sha256(pom(r#"
  <groupId>ut</groupId>
  <artifactId>lib</artifactId>
  <version>1.0</version>
  <dependencies>
    <dependency>
      <groupId>ut</groupId>
      <artifactId>junit</artifactId>
      <version>5.9</version>
      <scope>test</scope>
    </dependency>
    <dependency>
      <groupId>ut</groupId>
      <artifactId>lombok</artifactId>
      <version>1.18</version>
      <optional>true</optional>
    </dependency>
  </dependencies>"#)),
        ),
    ];
    let harness = Harness::serve(specs);

    let resolution = resolve(
        &harness.loader(),
        &harness.client,
        &[dep("app", "ut:app:=1.0")],
        &[],
        &[],
    )
    .expect("resolved");
    assert!(
        !resolution.nodes.iter().any(|n| n.artifact_id == "junit"),
        "test-scoped transitive deps stay out"
    );
    assert!(
        !resolution.nodes.iter().any(|n| n.artifact_id == "lombok"),
        "optional transitive deps stay out"
    );
    assert_eq!(resolution.skipped.len(), 2, "both skips named");
}
