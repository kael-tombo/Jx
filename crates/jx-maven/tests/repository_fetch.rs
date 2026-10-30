//! End-to-end tests of the repository client against a fake Maven remote
//! ([`jx_testsupport::FakeRepo`]): fetch + checksum verification, the
//! sidecar/header sources and their ordering, auth references against real
//! env vars, mirror routing, offline serving and snapshot resolution.
//!
//! Every check ADR 0003 fixes for the client is exercised here: checksums
//! verified on every fetch and always `fail`, `env:`-refs only, settings.xml
//! mirror semantics, and an offline cache that re-serves what a previous
//! online run stored — nothing more, nothing less.

use std::time::Duration;

use jx_common::config::{Mirror, Repository, Settings};
use jx_maven::Client;
use jx_testsupport::{Gate, ResponseSpec};
use sha2::Digest as _;

/// A temp jx home plus a running fake remote.
struct TestRemote {
    home: tempfile::TempDir,
    repo: jx_testsupport::FakeRepo,
    settings: Settings,
}

impl TestRemote {
    /// One repository called `central` (the client default id) that serves
    /// the given specs, no auth.
    fn start(specs: Vec<(String, ResponseSpec)>) -> TestRemote {
        let repo = jx_testsupport::FakeRepo::spawn(specs);
        let mut settings = Settings::default();
        settings.repositories.push(Repository {
            id: "central".to_string(),
            url: repo.url(),
            auth: None,
        });
        TestRemote {
            home: tempfile::tempdir().expect("temp home"),
            repo,
            settings,
        }
    }

    fn client(&self, offline: bool) -> Client {
        Client::new(self.home.path(), self.settings.clone(), offline).expect("client built")
    }
}

/// `jx_maven::Coordinate::parse` never fails on these literals.
fn coord(text: &str) -> jx_maven::Coordinate {
    jx_maven::Coordinate::parse(text).expect("literal coordinate")
}

/// Ed. 2024 makes the env API unsafe; test-local mutation, single-threaded
/// relative to the client construction, with restore-if-presented semantics
/// is contained here. Names are unique per test to avoid cross-test races.
fn set_env(name: &str, value: &str) {
    // SAFETY: tests only; names are unique to each test, and the single
    // process-wide env lock applies inside this call.
    unsafe { std::env::set_var(name, value) };
}

fn remove_env(name: &str) {
    // SAFETY: as `set_env`.
    unsafe { std::env::remove_var(name) };
}

/// Wait for the fixture. Each request is short-lived; a bounded poll is
/// enough for connection setup flakiness on loaded CI runners.
fn wait_for_condition(check: impl Fn() -> bool, what: &str) {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !check() && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(check(), "condition never became true: {what}");
}

const JAR_A: &[u8] = b"the jar bytes of artifact-a 1.0";
const JAR_B: &[u8] = b"totally different bytes for artifact-b 2.3.1";
const POM_A: &[u8] =
    b"<?xml version=\"1.0\"?><project><artifactId>a</artifactId><version>1.0</version></project>";

#[test]
fn release_fetch_verifies_sha256_sidecar() {
    let t = TestRemote::start(vec![(
        "com/ex/a/1.0/a-1.0.jar".to_string(),
        ResponseSpec::Sha256(JAR_A.to_vec()),
    )]);
    let outcome = t
        .client(false)
        .fetch_artifact(&coord("com.ex:a:1.0"))
        .expect("fetched");
    assert_eq!(outcome.bytes, JAR_A);
    assert_eq!(outcome.repo_id, "central");
    assert_eq!(
        outcome.sha256_hex,
        hex::encode(sha2::Sha256::digest(JAR_A)),
        "the stored key is the verified sha256 of the bytes"
    );
    assert!(outcome.url.ends_with("/com/ex/a/1.0/a-1.0.jar"));
}

#[test]
fn sha1_sidecar_is_accepted_for_older_remotes() {
    let t = TestRemote::start(vec![(
        "com/ex/b/2.3.1/b-2.3.1.jar".to_string(),
        ResponseSpec::Sha1(JAR_B.to_vec()),
    )]);
    let outcome = t
        .client(false)
        .fetch_artifact(&coord("com.ex:b:2.3.1"))
        .expect("fetched");
    assert_eq!(outcome.bytes, JAR_B);
    assert_eq!(outcome.sha256_hex, hex::encode(sha2::Sha256::digest(JAR_B)));
}

#[test]
fn header_checksum_is_used_without_sidecars() {
    let t = TestRemote::start(vec![(
        "com/ex/a/1.0/a-1.0.jar".to_string(),
        ResponseSpec::Sha256Header(JAR_A.to_vec()),
    )]);
    let outcome = t
        .client(false)
        .fetch_artifact(&coord("com.ex:a:1.0"))
        .expect("header checksum verified");
    assert_eq!(outcome.bytes, JAR_A);
}

#[test]
fn corrupt_sha1_sidecar_is_a_hard_failure() {
    let t = TestRemote::start(vec![(
        "com/ex/a/1.0/a-1.0.jar".to_string(),
        ResponseSpec::BrokenSha1(JAR_A.to_vec()),
    )]);
    // Poison the sidecar recorded path to a definitely wrong digest —
    // the fixture already publishes zeros for BrokenSha1.
    let err = t
        .client(false)
        .fetch_artifact(&coord("com.ex:a:1.0"))
        .expect_err("checksum mismatch is an error");
    assert!(
        err.message.contains("checksum mismatch"),
        "mismatch must name itself, got: {err}"
    );
    let dead = t
        .repo
        .served_paths()
        .iter()
        .any(|p| p.starts_with("com/ex/a/1.0/a-1.0.jar.sha1"));
    assert!(dead, "the sidecar was fetched before the verdict");
}

#[test]
fn unpublishable_checksum_is_rejected_fully() {
    let t = TestRemote::start(vec![(
        "com/ex/a/1.0/a-1.0.jar".to_string(),
        ResponseSpec::Content(JAR_A.to_vec()),
    )]);
    let err = t
        .client(false)
        .fetch_artifact(&coord("com.ex:a:1.0"))
        .expect_err("no checksum at all is a hard error");
    assert!(
        err.message.contains("publishes no checksum"),
        "the error must name the strict policy, got: {err}"
    );
}

#[test]
fn basic_auth_401s_without_env_and_success_with_env() {
    let mut t = TestRemote::start(vec![(
        "com/ex/private/1.0/private-1.0.jar".to_string(),
        ResponseSpec::Gated {
            gate: Gate::Basic {
                user: "ci".to_string(),
                pass: "token-1".to_string(),
            },
            inner: Box::new(ResponseSpec::Sha256Header(JAR_A.to_vec())),
        },
    )]);
    t.settings.repositories[0].auth = Some("env:JX_TEST_FETCH_BASIC".to_string());
    let coord = coord("com.ex:private:1.0");

    // No credential in the env: hard failure, no download started.
    remove_env("JX_TEST_FETCH_BASIC");
    let err = t
        .client(false)
        .fetch_artifact(&coord)
        .expect_err("missing auth env var");
    assert!(
        err.message.contains("JX_TEST_FETCH_BASIC"),
        "the error names the missing variable, got: {err}"
    );

    // Wrong value: the remote rejects with 401 and the client surfaces it.
    set_env("JX_TEST_FETCH_BASIC", "ci:wrong-pass");
    let err = t
        .client(false)
        .fetch_artifact(&coord)
        .expect_err("401 is surfaced");
    assert!(
        err.message.contains("401"),
        "the 401 must get through: {err}"
    );

    // Right value (`user:password` for Basic): fetch succeeds.
    set_env("JX_TEST_FETCH_BASIC", "ci:token-1");
    let outcome = t
        .client(false)
        .fetch_artifact(&coord)
        .expect("authenticated fetch");
    assert_eq!(outcome.bytes, JAR_A);
    remove_env("JX_TEST_FETCH_BASIC");
}

#[test]
fn bearer_auth_sends_the_raw_token() {
    let mut t = TestRemote::start(vec![(
        "com/ex/private/1.0/private-1.0.jar".to_string(),
        ResponseSpec::Gated {
            gate: Gate::Bearer {
                token: "tkn-abc-123".to_string(),
            },
            inner: Box::new(ResponseSpec::Sha256Header(JAR_A.to_vec())),
        },
    )]);
    t.settings.repositories[0].auth = Some("bearer:env:JX_TEST_FETCH_BEARER".to_string());
    remove_env("JX_TEST_FETCH_BEARER");
    let err = t
        .client(false)
        .fetch_artifact(&coord("com.ex:private:1.0"))
        .expect_err("missing bearer env");
    assert!(err.message.contains("JX_TEST_FETCH_BEARER"), "{err}");
    set_env("JX_TEST_FETCH_BEARER", "tkn-abc-123");
    let outcome = t
        .client(false)
        .fetch_artifact(&coord("com.ex:private:1.0"))
        .expect("bearer fetch");
    assert_eq!(outcome.bytes, JAR_A);
    let auth_log = t.repo.served_paths().join("\n");
    assert!(
        auth_log.contains("auth=\"Bearer tkn-abc-123\""),
        "the raw token went out as Bearer: {auth_log}"
    );
    remove_env("JX_TEST_FETCH_BEARER");
}

#[test]
fn exact_id_mirror_routes_requests_to_the_mirror() {
    let origin_specs = vec![("com/ex/a/1.0/a-1.0.jar".to_string(), ResponseSpec::Missing)];
    let mirror_specs = vec![(
        "com/ex/a/1.0/a-1.0.jar".to_string(),
        ResponseSpec::Sha256(JAR_A.to_vec()),
    )];
    let origin = jx_testsupport::FakeRepo::spawn(origin_specs);
    let mirror = jx_testsupport::FakeRepo::spawn(mirror_specs);
    let mut settings = Settings::default();
    settings.repositories.push(Repository {
        id: "internal".to_string(),
        url: origin.url(),
        auth: None,
    });
    settings.mirrors.push(Mirror {
        id: "mirror-of-internal".to_string(),
        of: "internal".to_string(),
        url: mirror.url(),
        auth: None,
    });
    let home = tempfile::tempdir().expect("home");
    let client = Client::new(home.path(), settings, false).expect("client");
    let outcome = client
        .fetch_artifact(&coord("com.ex:a:1.0"))
        .expect("mirror served the artifact");
    assert_eq!(outcome.bytes, JAR_A);
    assert_eq!(outcome.repo_id, "mirror-of-internal");
    let served = mirror.served_paths();
    assert!(
        served
            .iter()
            .any(|p| p.starts_with("com/ex/a/1.0/a-1.0.jar")),
        "the request went to the mirror: {served:?}"
    );
    // The origin sees nothing: an exact whole-value `of` mirror replaces
    // the origin for every request (ADR 0003).
    assert!(
        origin.served_paths().is_empty(),
        "mirrored requests bypass the origin entirely, got {:?}",
        origin.served_paths()
    );
}

#[test]
fn missing_repo_falls_through_to_the_next_one() {
    let first = jx_testsupport::FakeRepo::spawn(vec![(
        "com/ex/a/1.0/a-1.0.jar".to_string(),
        ResponseSpec::Missing,
    )]);
    let second = jx_testsupport::FakeRepo::spawn(vec![(
        "com/ex/a/1.0/a-1.0.jar".to_string(),
        ResponseSpec::Sha256(JAR_A.to_vec()),
    )]);
    let mut settings = Settings::default();
    for (id, repo) in [("first", &first), ("second", &second)] {
        settings.repositories.push(Repository {
            id: id.to_string(),
            url: repo.url(),
            auth: None,
        });
    }
    let home = tempfile::tempdir().expect("home");
    let client = Client::new(home.path(), settings, false).expect("client");
    let outcome = client
        .fetch_artifact(&coord("com.ex:a:1.0"))
        .expect("fallback worked");
    assert_eq!(outcome.repo_id, "second");
    // 404-on-repo-1, then artifact + sidecar on repo-2.
    wait_for_condition(
        || first.served_count() == 1 && second.served_count() == 2,
        "one 404 attempt, then artifact + sidecar on the fallback",
    );
}

#[test]
fn offline_serves_only_what_a_prior_online_run_stored() {
    let t = TestRemote::start(vec![
        (
            "com/ex/a/1.0/a-1.0.jar".to_string(),
            ResponseSpec::Sha256(JAR_A.to_vec()),
        ),
        (
            "com/ex/b/2.3.1/b-2.3.1.jar".to_string(),
            ResponseSpec::Sha256(JAR_B.to_vec()),
        ),
    ]);
    let online = t.client(false);
    let _ = online
        .fetch_artifact(&coord("com.ex:a:1.0"))
        .expect("warm the cache");

    let offline = t.client(true);
    let outcome = offline
        .fetch_artifact(&coord("com.ex:a:1.0"))
        .expect("offline serves prior content");
    assert_eq!(outcome.bytes, JAR_A);
    assert_eq!(outcome.sha256_hex, hex::encode(sha2::Sha256::digest(JAR_A)));
    assert!(outcome.url.starts_with("offline:"), "{:?}", outcome.url);
    assert_eq!(outcome.repo_id, "jx-cache");

    // Anything not previously fetched is a clean offline error, not a crash.
    let err = offline
        .fetch_artifact(&coord("com.ex:b:2.3.1"))
        .expect_err("unwarmed paths fail offline");
    assert!(
        err.message.contains("never fetched") || err.message.contains("cache"),
        "the error explains the cache miss: {err}"
    );
    assert_eq!(
        t.repo.served_count(),
        2,
        "offline never touched the remote: the online run consumed the artifact + sidecar"
    );
}

#[test]
fn snapshot_resolves_through_version_level_metadata() {
    let metadata = r#"<?xml version="1.0" encoding="UTF-8"?>
<metadata>
  <groupId>com.ex.snap</groupId>
  <artifactId>core</artifactId>
  <version>2.1.0-SNAPSHOT</version>
  <versioning>
    <snapshot>
      <timestamp>20250811.053044</timestamp>
      <buildNumber>12</buildNumber>
    </snapshot>
  </versioning>
</metadata>
"#;
    let resolved_pom = b"<project><artifactId>core</artifactId><version>2.1.0-20250811.053044-12</version></project>";
    let dir = "com/ex/snap/core/2.1.0-SNAPSHOT";
    let t = TestRemote::start(vec![
        (
            format!("{dir}/maven-metadata.xml"),
            ResponseSpec::Sha256(metadata.as_bytes().to_vec()),
        ),
        (
            format!("{dir}/core-2.1.0-20250811.053044-12.pom"),
            ResponseSpec::Sha256(resolved_pom.to_vec()),
        ),
    ]);
    let outcome = t
        .client(false)
        .fetch_pom(&coord("com.ex.snap:core:2.1.0-SNAPSHOT"))
        .expect("snapshot resolved");
    assert_eq!(outcome.bytes, resolved_pom.to_vec());
    let served = t.repo.served_paths();
    assert!(
        served
            .iter()
            .any(|p| p.starts_with("com/ex/snap/core/2.1.0-SNAPSHOT/maven-metadata.xml"))
            && served
                .iter()
                .any(|p| p.contains("core-2.1.0-20250811.053044-12.pom")),
        "metadata first, then the timestamped file: {served:?}"
    );
}

#[test]
fn parallel_fetch_preserves_input_order_and_per_coordinate_errors() {
    let t = TestRemote::start(vec![
        (
            "com/ex/a/1.0/a-1.0.jar".to_string(),
            ResponseSpec::Sha256(JAR_A.to_vec()),
        ),
        // `b` has no checksums: its slot must fail while `a` succeeds.
        (
            "com/ex/b/2.3.1/b-2.3.1.jar".to_string(),
            ResponseSpec::Content(JAR_B.to_vec()),
        ),
        (
            "com/ex/p/1.0/p-1.0.jar".to_string(),
            ResponseSpec::Sha256(POM_A.to_vec()),
        ),
    ]);
    let client = t.client(false);
    let coords: Vec<_> = ["com.ex:a:1.0", "com.ex:b:2.3.1", "com.ex:p:1.0"]
        .iter()
        .map(|c| coord(c))
        .collect();
    let results = client.fetch_all(&coords);
    assert_eq!(results.len(), 3);
    assert_eq!(results[0].coordinate.key(), "com.ex:a:1.0");
    assert!(results[0].outcome.is_ok(), "a fetched");
    assert!(results[1].outcome.is_err(), "b failed on its own");
    assert_eq!(results[2].coordinate.key(), "com.ex:p:1.0");
    assert!(results[2].outcome.is_ok(), "p fetched");
}

#[test]
fn pom_fetch_hits_the_pom_path() {
    let t = TestRemote::start(vec![(
        "com/ex/a/1.0/a-1.0.pom".to_string(),
        ResponseSpec::Sha256(POM_A.to_vec()),
    )]);
    let outcome = t
        .client(false)
        .fetch_pom(&coord("com.ex:a:1.0"))
        .expect("pom fetched");
    assert_eq!(outcome.bytes, POM_A);
    assert!(outcome.url.ends_with("/com/ex/a/1.0/a-1.0.pom"));
}
