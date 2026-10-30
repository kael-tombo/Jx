//! The Maven repository client.
//!
//! Behaviour is the one recorded in `docs/decisions/0003-repository-client-and-pom-model.md`:
//!
//! - **Repositories:** config-declared, in order; when none are declared jx
//!   uses Maven's default repo, `central` (`https://repo.maven.apache.org/maven2`).
//! - **Mirrors:** settings.xml semantics (`mirrorOf` grammar, not a glob);
//!   an exact whole-value `of` wins, otherwise the first matching mirror in
//!   declaration order. The mirror's own credentials are used.
//! - **Proxies:** config proxy first, then `HTTPS_PROXY`/`HTTP_PROXY`
//!   environment; `no_proxy` suffix patterns disable proxying per host.
//!   Selection happens against the **mirror-selected** URL.
//! - **Checksums:** verified on every fetch — Central-style response
//!   headers first, then sidecars, `.sha256` before `.sha1` — always `fail`
//!   on mismatch, and a hard error when a repository publishes no checksum.
//! - **Cache:** bytes land in the jx content cache keyed by their sha256,
//!   plus a path→content index so offline mode can re-serve exactly what a
//!   previous run fetched.
//! - **Auth:** credential references (`env:` / `bearer:env:`) only; secrets
//!   exist solely in the environment.

use std::collections::HashMap;
use std::io::Read as _;
use std::path::Path;

use base64::Engine as _;
use jx_common::config::{AuthRef, Mirror, Repository, Settings};
use jx_common::error::{JxError, Kind};
use sha2::{Digest, Sha256};

/// The body and lowercased headers of one GET response; [`HttpGet::body`] is
/// `None` when the repository answered 404.
#[derive(Debug, Default)]
struct HttpGet {
    body: Option<Vec<u8>>,
    headers: Vec<(String, String)>,
}
use ureq::Agent;

use crate::coords::Coordinate;
use crate::xml::{self as model, ArtifactMetadata};

/// The result of fetching one repository path.
#[derive(Debug, Clone)]
pub struct FetchOutcome {
    pub bytes: Vec<u8>,
    /// Sha256 of `bytes`, the content-cache key.
    pub sha256_hex: String,
    /// The URL it came from, mirror-resolved, for diagnostics.
    pub url: String,
    /// The id of the repository (mirror) that served the request.
    pub repo_id: String,
}

/// The coordinate/outcome pair produced by parallel fetching.
#[derive(Debug)]
pub struct FetchResult {
    pub coordinate: Coordinate,
    pub outcome: Result<FetchOutcome, JxError>,
}

/// A repository as chosen for one request: mirror-resolved.
#[derive(Debug, Clone)]
pub struct SelectedRepo {
    pub id: String,
    pub url: String,
    pub auth: Option<AuthRef>,
}

pub struct Client {
    settings: Settings,
    /// Effective repository order (config order, `central` default).
    repositories: Vec<Repository>,
    /// Content cache under the jx home.
    cache: jx_common::cache::Cache,
    /// Agent per distinct proxy URL; the empty key is the direct agent.
    agents: HashMap<String, Agent>,
    /// Effective `no_proxy` patterns (config wins over the environment).
    no_proxy: Vec<String>,
    offline: bool,
}

impl Client {
    /// Build the client for a jx home and settings. Timeouts match the
    /// toolchain downloader; proxies are configured per distinct proxy URL;
    /// request-time selection runs against the mirror-selected URL.
    pub fn new(home: &Path, settings: Settings, offline: bool) -> Result<Client, JxError> {
        let no_proxy = effective_no_proxy(&settings);
        let repositories = if settings.repositories.is_empty() {
            vec![central_repo()]
        } else {
            settings.repositories.clone()
        };
        let mut agents: HashMap<String, Agent> = HashMap::new();
        for proxy_url in candidate_proxies(&settings) {
            let proxy = ureq::Proxy::new(&proxy_url).map_err(|e| {
                JxError::new(Kind::Network, "invalid proxy URL")
                    .at(proxy_url.clone())
                    .caused_by(e)
                    .hint("proxy URLs look like `http://host:port`")
            })?;
            agents.insert(proxy_url.clone(), make_agent().proxy(proxy).build());
        }
        agents.insert(String::new(), make_agent().build());
        Ok(Client {
            settings,
            repositories,
            cache: jx_common::cache::Cache::new(home),
            agents,
            no_proxy,
            offline,
        })
    }

    /// repositories as declared (central when none are), for diagnostics.
    pub fn repositories(&self) -> &[Repository] {
        &self.repositories
    }

    /// Mirror-resolve one declared repository per settings.xml semantics.
    pub fn selected(&self, repo: &Repository) -> SelectedRepo {
        let mirrors: &[Mirror] = &self.settings.mirrors;
        // An exact whole-value match of one id wins first (ADR 0003).
        let chosen = mirrors.iter().find(|m| m.of == repo.id).or_else(|| {
            mirrors
                .iter()
                .find(|m| pattern_matches(&m.of, &repo.id, &repo.url))
        });
        match chosen {
            Some(m) => SelectedRepo {
                id: m.id.clone(),
                url: join_base(&m.url),
                auth: m.auth.as_deref().and_then(|a| AuthRef::parse(a).ok()),
            },
            None => SelectedRepo {
                id: repo.id.clone(),
                url: join_base(&repo.url),
                auth: repo.auth.as_deref().and_then(|a| AuthRef::parse(a).ok()),
            },
        }
    }

    /// Fetch a POM for a coordinate (snapshots resolve through metadata).
    pub fn fetch_pom(&self, coord: &Coordinate) -> Result<FetchOutcome, JxError> {
        self.fetch_one(coord, true)
    }

    /// Fetch the artifact (jar/…) for a coordinate, checksums verified.
    pub fn fetch_artifact(&self, coord: &Coordinate) -> Result<FetchOutcome, JxError> {
        self.fetch_one(coord, false)
    }

    /// Fetch many coordinates in parallel (at most 8 threads), preserving
    /// input order; each failure stays tied to its coordinate.
    pub fn fetch_all(&self, coords: &[Coordinate]) -> Vec<FetchResult> {
        if coords.is_empty() {
            return Vec::new();
        }
        let threads = std::cmp::min(coords.len(), 8);
        let mut slots: Vec<Option<FetchResult>> = (0..coords.len()).map(|_| None).collect();
        let this = &self;
        std::thread::scope(|scope| {
            let mut handles = Vec::new();
            for thread_index in 0..threads {
                let start = coords.len() * thread_index / threads;
                let end = coords.len() * (thread_index + 1) / threads;
                if start == end {
                    continue;
                }
                handles.push(scope.spawn(move || {
                    coords[start..end]
                        .iter()
                        .map(|coord| {
                            let outcome = this.fetch_artifact(coord);
                            FetchResult {
                                coordinate: coord.clone(),
                                outcome,
                            }
                        })
                        .collect::<Vec<FetchResult>>()
                }));
            }
            for handle in handles {
                if let Ok(results) = handle.join() {
                    for result in results {
                        if let Some(index) = coords.iter().position(|c| *c == result.coordinate) {
                            slots[index] = Some(result);
                        }
                    }
                }
            }
        });
        slots
            .into_iter()
            .zip(coords.iter())
            .map(|(slot, _)| slot.expect("every coordinate has a result"))
            .collect()
    }

    /// Dispatch a single coordinate: snapshot flows fetch metadata first.
    /// `pom` selects the `.pom` file; otherwise the artifact itself is
    /// fetched (classifier and extension honoured).
    fn fetch_one(&self, coord: &Coordinate, pom: bool) -> Result<FetchOutcome, JxError> {
        if coord.is_snapshot() {
            self.fetch_snapshot(coord, pom)
        } else {
            self.fetch_release(coord, pom)
        }
    }

    /// Release flow: try each repository in order, checksum-verified.
    fn fetch_release(&self, coord: &Coordinate, pom: bool) -> Result<FetchOutcome, JxError> {
        let path = if pom {
            coord.pom_path()
        } else {
            coord.artifact_path()
        };
        if self.offline {
            return self.fetch_cached(&path);
        }
        let mut last_missing: Option<JxError> = None;
        for repo in &self.repositories {
            let selected = self.selected(repo);
            let url = format!("{}{}", selected.url, path);
            match self.get_with_checksum(&selected, &path, &url) {
                Ok(Some(outcome)) => return Ok(outcome),
                Ok(None) => {
                    last_missing = Some(
                        JxError::new(Kind::Network, "artifact not found")
                            .at(coord.key())
                            .hint(
                                "check the version in project.toml or the repositories in config.toml",
                            ),
                    );
                }
                Err(e) => return Err(e),
            }
        }
        Err(last_missing.expect("at least one repository is configured, so a miss is recorded"))
    }

    /// Snapshot flow: version-level `maven-metadata.xml` from the first
    /// repository that has it, then the point-in-time concrete file. The
    /// file lives under the **snapshot** version directory (the `-SNAPSHOT`
    /// marker stays); the resolved version only shapes the file name.
    fn fetch_snapshot(&self, coord: &Coordinate, pom: bool) -> Result<FetchOutcome, JxError> {
        if self.offline {
            return Err(JxError::new(
                Kind::Network,
                "snapshot resolution needs the network; offline cache serving is releases-only",
            )
            .at(coord.key())
            .hint(
                "pin a release version, or run online once and let the lockfile record the timestamped form",
            ));
        }
        let metadata_path = coord.snapshot_metadata_path();
        let mut last_missing: Option<JxError> = None;
        for repo in &self.repositories {
            let selected = self.selected(repo);
            let metadata_url = format!("{}{}", selected.url, metadata_path);
            let metadata_bytes =
                match self.get_with_checksum(&selected, &metadata_path, &metadata_url) {
                    Ok(Some(outcome)) => outcome,
                    Ok(None) => {
                        last_missing = Some(
                            JxError::new(Kind::Network, "snapshot metadata not found")
                                .at(coord.key()),
                        );
                        continue;
                    }
                    Err(e) => return Err(e),
                };
            let metadata_text = String::from_utf8_lossy(&metadata_bytes.bytes).into_owned();
            let metadata: ArtifactMetadata = model::parse_metadata(&metadata_text, &metadata_path)
                .map_err(|e| e.at(metadata_url.clone()))?;
            let snapshot = metadata.snapshot.ok_or_else(|| {
                JxError::new(
                    Kind::Resolve,
                    format!(
                        "version-level metadata for {} has no <snapshot> block",
                        coord.key()
                    ),
                )
                .at(metadata_url.clone())
                .hint("the repository published a broken maven-metadata.xml for this snapshot")
            })?;
            let resolved_version = coord
                .resolved_snapshot_version(&snapshot)
                .map_err(|e| e.at(metadata_url.clone()))?;
            // The resolved version drives the file name; the directory stays
            // the snapshot directory Maven publishes files into.
            let resolved = coord.with_version(&resolved_version);
            let file = if pom {
                resolved.pom_file_name()
            } else {
                resolved.artifact_file_name()
            };
            let path = format!("{}/{}", coord.version_directory(), file);
            let url = format!("{}{}", selected.url, path);
            return match self.get_with_checksum(&selected, &path, &url) {
                Ok(Some(outcome)) => Ok(outcome),
                Ok(None) => Err(JxError::new(
                    Kind::Network,
                    format!(
                        "{} resolved to {} but the timestamped file is missing",
                        coord.key(),
                        resolved_version
                    ),
                )
                .at(url)
                .hint("the CI build that published the snapshot may still be running")),
                Err(e) => Err(e),
            };
        }
        Err(last_missing.expect("a miss or error is always recorded"))
    }

    /// Serve a previously fetched path from the cache (offline mode).
    pub fn fetch_cached(&self, repo_path: &str) -> Result<FetchOutcome, JxError> {
        let sha = self
            .path_index_lookup(repo_path)
            .ok_or_else(|| {
                JxError::new(
                    Kind::Network,
                    format!("`{repo_path}` was never fetched, so it is not in the cache"),
                )
                .hint("run once with network (remove offline mode) and retry offline after")
            })?
            .clone();
        let bytes = self.cache.get(&sha)?;
        let sha256_hex = self.cache.put(&bytes)?;
        Ok(FetchOutcome {
            bytes: bytes.clone(),
            sha256_hex,
            url: format!("offline:{repo_path}"),
            repo_id: "jx-cache".to_string(),
        })
    }

    /// Whether a repository path was fetched before (by-path cache index).
    pub fn is_cached(&self, repo_path: &str) -> bool {
        self.path_index_lookup(repo_path)
            .map(|sha| self.cache.has(&sha))
            .unwrap_or(false)
    }

    /// GET one path with checksum verification on success. `Ok(None)` is a
    /// 404 — the caller tries the next repository.
    fn get_with_checksum(
        &self,
        selected: &SelectedRepo,
        path: &str,
        url: &str,
    ) -> Result<Option<FetchOutcome>, JxError> {
        let response = self.http_get(url, selected.auth.as_ref())?;
        let Some(bytes) = response.body else {
            return Ok(None);
        };
        let expected = self.locate_checksum(selected, url, path, &response.headers)?;
        verify_checksum(url, &bytes, &expected)?;
        let sha256_hex = self.cache.put(&bytes)?;
        self.record_path_index(path, &sha256_hex)?;
        Ok(Some(FetchOutcome {
            bytes,
            sha256_hex,
            url: url.to_string(),
            repo_id: selected.id.clone(),
        }))
    }

    /// Checksum sources in order: headers above sidecars, sha256 before
    /// sha1 (ADR 0003). Nothing at all is a hard error, never a warning.
    fn locate_checksum(
        &self,
        selected: &SelectedRepo,
        url: &str,
        path: &str,
        headers: &[(String, String)],
    ) -> Result<ExpectedChecksum, JxError> {
        for (header, kind) in [
            ("x-checksum-sha256", ChecksumKind::Sha256),
            ("x-checksum-sha1", ChecksumKind::Sha1),
        ] {
            if let Some(hex) = header_checksum(headers, header, kind) {
                return Ok(kind.into_expected(hex));
            }
        }
        for (name, kind) in [
            ("sha256", ChecksumKind::Sha256),
            ("sha1", ChecksumKind::Sha1),
        ] {
            let sidecar_path = format!("{path}.{name}");
            let sidecar_url = format!("{}{}", selected.url, sidecar_path);
            let sidecar = self.http_get(&sidecar_url, selected.auth.as_ref())?;
            if let Some(sidecar) = sidecar.body {
                let text = String::from_utf8_lossy(&sidecar);
                let token = text.split_whitespace().next().unwrap_or_default();
                if let Some(expected) = kind.accepts(token) {
                    return Ok(kind.into_expected(expected.to_string()));
                }
            }
        }
        Err(JxError::new(
            Kind::Network,
            "the repository publishes no checksum for this artifact",
        )
        .at(url)
        .hint(
            "jx never downloads unchecksummed artifacts; the repository (not jx) must publish .sha1/.sha256 sidecars",
        ))
    }

    fn http_get(&self, url: &str, auth: Option<&AuthRef>) -> Result<HttpGet, JxError> {
        let agent = self.agent_for(url);
        let mut request = agent.get(url);
        if let Some(header) = auth_header(auth)? {
            request = request.set("Authorization", &header);
        }
        let response = match request.call() {
            Ok(response) => response,
            Err(ureq::Error::Status(404, _)) => return Ok(HttpGet::default()),
            Err(ureq::Error::Status(status @ (401 | 403), _)) => {
                return Err(JxError::new(
                    Kind::Network,
                    format!("the repository rejected the request ({status})"),
                )
                .at(url)
                .hint(
                    "set the auth reference (`env:VAR`) in config.toml; the variable value is \
                         `user:password` for Basic auth, or prefix `bearer:` for a raw token",
                ));
            }
            Err(ureq::Error::Status(status, _)) => {
                return Err(JxError::new(
                    Kind::Network,
                    format!("unexpected HTTP status {status} from the repository"),
                )
                .at(url)
                .caused_by(format!("HTTP {status}"))
                .hint("check the repository is a Maven remote layout URL"));
            }
            Err(transport) => {
                return Err(JxError::new(Kind::Network, "cannot reach the repository")
                    .at(url)
                    .caused_by(transport)
                    .hint("check connectivity or the proxy settings in config.toml"));
            }
        };
        let headers: Vec<(String, String)> = response
            .headers_names()
            .into_iter()
            .filter_map(|name| {
                response
                    .header(&name)
                    .map(|value| (name.to_ascii_lowercase(), value.to_string()))
            })
            .collect();
        let mut bytes = Vec::new();
        response
            .into_reader()
            .take(64 * 1024 * 1024)
            .read_to_end(&mut bytes)
            .map_err(|e| {
                JxError::new(Kind::Network, "the repository response was cut short")
                    .at(url)
                    .caused_by(e)
                    .hint("retry once; repeated truncation usually means a broken proxy")
            })?;
        Ok(HttpGet {
            body: Some(bytes),
            headers,
        })
    }

    /// The agent honouring no_proxy and the config/env proxy for this URL.
    fn agent_for(&self, url: &str) -> &Agent {
        if no_proxy_matches(&self.no_proxy, url) {
            return direct_agent(&self.agents);
        }
        let key = self.proxy_url_for(url).unwrap_or_default();
        self.agents
            .get(&key)
            .unwrap_or_else(|| direct_agent(&self.agents))
    }

    /// Proxy source decision per ADR 0003: config first (with the scheme
    /// fallback), then environment; selected against the mirror URL.
    fn proxy_url_for(&self, url: &str) -> Option<String> {
        let https = url.starts_with("https:");
        if let Some(proxy) = &self.settings.proxy {
            let chosen = if https {
                proxy.https.clone().or_else(|| proxy.http.clone())
            } else {
                proxy.http.clone()
            };
            if chosen.is_some() {
                return chosen;
            }
            // The proxy table is set but has no entry for this scheme; fall
            // through to the environment rather than leaking direct traffic.
        }
        env_proxy(https)
    }
}

/// Every distinct proxy URL the settings and environment can produce, so
/// the client pre-builds one agent per proxy at construction time.
fn candidate_proxies(settings: &Settings) -> Vec<String> {
    let mut urls = Vec::new();
    if let Some(proxy) = &settings.proxy {
        for url in [&proxy.http, &proxy.https].into_iter().flatten() {
            urls.push(url.clone());
        }
    }
    for (upper, lower) in [("HTTPS_PROXY", "https_proxy"), ("HTTP_PROXY", "http_proxy")] {
        let from_env = std::env::var(upper)
            .or_else(|_| std::env::var(lower))
            .ok()
            .filter(|v| !v.trim().is_empty());
        if let Some(url) = from_env {
            urls.push(url);
        }
    }
    urls.dedup();
    urls
}

fn direct_agent(agents: &HashMap<String, Agent>) -> &Agent {
    &agents[&String::new()]
}

/// A checksum expectation discovered for one artifact.
#[derive(Clone, Copy)]
enum ChecksumKind {
    Sha256,
    Sha1,
}

impl ChecksumKind {
    /// Whether a header/sidecar value is a plausible hash of this kind.
    fn accepts(self, token: &str) -> Option<String> {
        let token = token.trim().to_lowercase();
        let ok_len = match self {
            ChecksumKind::Sha256 => 64,
            ChecksumKind::Sha1 => 40,
        };
        (token.len() == ok_len && token.bytes().all(|b| b.is_ascii_hexdigit())).then_some(token)
    }

    fn into_expected(self, hex: String) -> ExpectedChecksum {
        match self {
            ChecksumKind::Sha256 => ExpectedChecksum::Sha256(hex),
            ChecksumKind::Sha1 => ExpectedChecksum::Sha1(hex),
        }
    }
}

enum ExpectedChecksum {
    Sha256(String),
    Sha1(String),
}

impl ExpectedChecksum {
    fn actual_hex(&self, bytes: &[u8]) -> String {
        match self {
            ExpectedChecksum::Sha256(_) => hex::encode(Sha256::digest(bytes)).to_lowercase(),
            ExpectedChecksum::Sha1(_) => hex::encode(sha1::Sha1::digest(bytes)).to_lowercase(),
        }
    }
}

fn verify_checksum(url: &str, bytes: &[u8], expected: &ExpectedChecksum) -> Result<(), JxError> {
    let actual = expected.actual_hex(bytes);
    let expected_hex = match expected {
        ExpectedChecksum::Sha256(hex) | ExpectedChecksum::Sha1(hex) => hex.to_lowercase(),
    };
    if actual != expected_hex {
        return Err(JxError::new(
            Kind::Network,
            format!("checksum mismatch: expected {expected_hex}, got {actual}"),
        )
        .at(url)
        .hint(
            "the bytes jx received are not what the repository says it wrote; retry once \
             from a different network before reporting the artifact",
        ));
    }
    Ok(())
}

fn header_checksum(headers: &[(String, String)], name: &str, kind: ChecksumKind) -> Option<String> {
    headers
        .iter()
        .find(|(header, _)| header == name)
        .and_then(|(_, value)| kind.accepts(value))
}

fn central_repo() -> Repository {
    Repository {
        id: "central".to_string(),
        url: "https://repo.maven.apache.org/maven2".to_string(),
        auth: None,
    }
}

/// Trim a base URL into the shared `…/` join form.
fn join_base(base: &str) -> String {
    if base.ends_with('/') {
        base.to_string()
    } else {
        format!("{base}/")
    }
}

/// Parse a host out of a repository URL (bare enough for `external:` checks
/// and no_proxy matching; config urls are already scheme-validated).
fn host_of(url: &str) -> Option<String> {
    let rest = url.split_once("://")?.1;
    let authority = rest.split(['/', '?', '#']).next()?;
    let authority = authority
        .rsplit_once('@')
        .map(|(_userinfo, host)| host)
        .unwrap_or(authority);
    let host = authority
        .split(':')
        .next()?
        .to_lowercase()
        .trim_matches(['[', ']'])
        .to_string();
    (!host.is_empty()).then_some(host)
}

/// `file:` and localhost-bound urls are never "external" (ADR 0003 list).
fn is_external(url: &str) -> bool {
    if url.starts_with("file:") {
        return false;
    }
    match host_of(url) {
        Some(host) => !matches!(host.as_str(), "localhost" | "127.0.0.1" | "[::1]"),
        None => false,
    }
}

/// One `mirrorOf` pattern token: exact id, `*`, `external:*`,
/// `external:http:*`. **Not a glob**: `repo*` is a literal id and matches
/// nothing but an id spelled `repo*`.
fn token_matches(token: &str, repo_id: &str, url: &str) -> bool {
    if token == repo_id {
        return true;
    }
    match token {
        "*" => true,
        "external:*" => is_external(url),
        "external:http:*" => is_external(url) && url.starts_with("http:"),
        _ => false,
    }
}

/// Maven's DefaultMirrorSelector shape: `!id` exclusions veto the whole
/// pattern even after a positive token matched; positives accumulate.
pub fn pattern_matches(of: &str, repo_id: &str, url: &str) -> bool {
    let mut matched = false;
    for token in of.split(',') {
        if let Some(rest) = token.strip_prefix('!') {
            if token_matches(rest, repo_id, url) {
                return false;
            }
        } else if token_matches(token, repo_id, url) {
            matched = true;
        }
    }
    matched
}

/// no_proxy decision: suffix matching against the host (case-insensitive),
/// with `*`, leading `*` and leading-dot forms tolerated per the standard
/// environment grammar and ADR 0003.
pub fn no_proxy_matches(patterns: &[String], url: &str) -> bool {
    let Some(host) = host_of(url) else {
        return false;
    };
    patterns.iter().any(|pattern| {
        let p = pattern.trim().trim_start_matches(['*', '.']).to_lowercase();
        !p.is_empty() && (host == p || host.ends_with(&format!(".{p}")))
    })
}

/// The environment proxy for a scheme; `HTTPS_PROXY`/`HTTP_PROXY` plus the
/// lowercase forms Unix tools conventionally honour.
fn env_proxy(https: bool) -> Option<String> {
    let (upper, lower) = if https {
        ("HTTPS_PROXY", "https_proxy")
    } else {
        ("HTTP_PROXY", "http_proxy")
    };
    [upper, lower]
        .iter()
        .find_map(|name| std::env::var(name).ok().filter(|v| !v.trim().is_empty()))
}

/// Config `no_proxy` first, then the environment (`NO_PROXY`/`no_proxy`).
fn effective_no_proxy(settings: &Settings) -> Vec<String> {
    if let Some(list) = settings
        .proxy
        .as_ref()
        .and_then(|proxy| proxy.no_proxy.as_ref())
    {
        return list.split(',').map(str::to_string).collect();
    }
    std::env::var("NO_PROXY")
        .or_else(|_| std::env::var("no_proxy"))
        .map(|list| list.split(',').map(str::to_string).collect())
        .unwrap_or_default()
}

/// Resolve a credential reference against the environment: the only place a
/// secret enters the process.
fn auth_header(auth: Option<&AuthRef>) -> Result<Option<String>, JxError> {
    let Some(auth) = auth else {
        return Ok(None);
    };
    let value = std::env::var(auth.env_var()).map_err(|_| {
        JxError::new(
            Kind::Network,
            format!(
                "auth variable {} is not set in the environment",
                auth.env_var()
            ),
        )
        .hint(
            "export it: `user:password` for Basic auth, the raw token when the auth reference \
             reads `bearer:env:VAR`",
        )
    })?;
    Ok(Some(match auth {
        AuthRef::Basic { .. } => format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode(value)
        ),
        AuthRef::Bearer { .. } => format!("Bearer {value}"),
    }))
}

fn make_agent() -> ureq::AgentBuilder {
    ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(30))
        .timeout_read(std::time::Duration::from_secs(120))
}

/// sha256 of a path, the by-path cache index file name.
fn path_index_name(repo_path: &str) -> String {
    hex::encode(Sha256::digest(repo_path.as_bytes()))
}

impl Client {
    /// The stored sha256 of a previously fetched path.
    fn path_index_lookup(&self, repo_path: &str) -> Option<String> {
        let entry = self
            .cache
            .root()
            .join("paths")
            .join(path_index_name(repo_path));
        std::fs::read_to_string(entry)
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }

    /// Record a fetched path → sha256 mapping for offline serving.
    fn record_path_index(&self, repo_path: &str, sha256_hex: &str) -> Result<(), JxError> {
        let dir = self.cache.root().join("paths");
        std::fs::create_dir_all(&dir).map_err(|e| {
            JxError::new(Kind::Io, "cannot create the path-index directory")
                .at(dir.display().to_string())
                .caused_by(e)
        })?;
        std::fs::write(dir.join(path_index_name(repo_path)), sha256_hex).map_err(|e| {
            JxError::new(Kind::Io, "cannot update the path index")
                .at(dir.display().to_string())
                .caused_by(e)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jx_common::config::Mirror;

    #[test]
    fn mirror_of_is_not_a_glob() {
        // `repo*` is a literal id; it matches nothing but itself.
        assert!(pattern_matches("repo*", "repo*", "https://r.example/maven"));
        assert!(!pattern_matches(
            "repo*",
            "repo1",
            "https://r.example/maven"
        ));
    }

    #[test]
    fn mirror_of_grammar() {
        let url = "https://central.example/maven";
        assert!(pattern_matches("central", "central", url));
        assert!(pattern_matches("*", "anything", url));
        assert!(pattern_matches("external:*", "a", url));
        assert!(!pattern_matches(
            "external:*",
            "a",
            "http://localhost/maven"
        ));
        assert!(!pattern_matches("external:*", "a", "file:///opt/repo"));
        assert!(pattern_matches(
            "external:http:*",
            "a",
            "http://central.example/maven"
        ));
        assert!(!pattern_matches("external:http:*", "a", url)); // https
    }

    #[test]
    fn exclusions_veto_the_whole_pattern() {
        let url = "https://central.example/maven";
        assert!(!pattern_matches("*,!central", "central", url));
        assert!(pattern_matches("*,!central", "other", url));
    }

    #[test]
    fn no_whitespace_trimming_in_the_pattern_list() {
        // Documented Maven behaviour: no trimming; ` repo` is not `repo`.
        assert!(!pattern_matches(
            " repo",
            "repo",
            "https://central.example/maven"
        ));
    }

    #[test]
    fn localhost_forms_never_count_as_external() {
        assert!(!is_external("https://localhost/maven"));
        assert!(!is_external("https://127.0.0.1:8080/maven"));
        assert!(is_external("https://internal.corp/maven"));
    }

    #[test]
    fn host_parsing() {
        assert_eq!(host_of("https://a.b:8080/path"), Some("a.b".to_string()));
        assert_eq!(host_of("http://a.b/"), Some("a.b".to_string()));
        assert_eq!(host_of("file:///x"), None);
    }

    #[test]
    fn no_proxy_suffix_matching() {
        let patterns = vec!["*.corp.com".to_string(), "localhost".to_string()];
        assert!(no_proxy_matches(&patterns, "https://a.corp.com/maven"));
        assert!(no_proxy_matches(&patterns, "https://corp.com/maven"));
        assert!(no_proxy_matches(&patterns, "http://localhost/x"));
        assert!(!no_proxy_matches(&patterns, "https://notcorp.com/maven"));
        // Case-insensitive.
        assert!(no_proxy_matches(
            &["Corp.Com".to_string()],
            "https://A.CORP.COM/m"
        ));
    }

    #[test]
    fn base_url_joining() {
        assert_eq!(
            join_base("https://r.example/maven"),
            "https://r.example/maven/"
        );
        assert_eq!(
            join_base("https://r.example/maven/"),
            "https://r.example/maven/"
        );
    }

    #[test]
    fn central_is_used_when_no_repositories_are_declared() {
        let dir = tempfile::tempdir().unwrap();
        let client = Client::new(dir.path(), Settings::default(), false).unwrap();
        assert_eq!(client.repositories().len(), 1);
        assert_eq!(client.repositories()[0].id, "central");
        // And an exact-id mirror selection against it reflects the mirror.
        let mut settings = Settings::default();
        settings.mirrors.push(Mirror {
            id: "m".to_string(),
            of: "central".to_string(),
            url: "https://mirror.example.com/maven".to_string(),
            auth: None,
        });
        let client = Client::new(dir.path(), settings, false).unwrap();
        let selected = client.selected(&client.repositories()[0]);
        assert_eq!(selected.id, "m");
        assert_eq!(selected.url, "https://mirror.example.com/maven/");
    }
}
