//! Shared test fixtures for the jx workspace.
//!
//! [`FakeRepo`] is a minimal Maven-layout repository server: it answers
//! `GET /<group path>/<artifact>/<version>/<file>` requests from a spec map,
//! serving checksum sidecars (`.sha256` / `.sha1`), checksum headers and
//! Basic/Bearer auth like a real remote. It logs every path it was asked
//! for, which mirror-routing assertions read back.
//!
//! It has no dependencies beyond `base64` and the workspace's hashing
//! crates: the bind socket is `127.0.0.1:0`, each connection is served by a
//! short-lived thread, and responses are HTTP/1.1 with `Content-Length`,
//! closed after each request.
//!
//! See `docs/decisions/0003-repository-client-and-pom-model.md` for the
//! repository-client behaviour this drives.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::io::{Read as _, Write as _};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use base64::Engine as _;
use sha1::Digest as _;

/// The credential a [`ResponseSpec::Gated`] path demands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Gate {
    /// `Basic base64(user:pass)`.
    Basic { user: String, pass: String },
    /// `Bearer token`, the raw token in the header.
    Bearer { token: String },
}

impl Gate {
    /// The Authorization header value that satisfies this gate.
    fn accepted_header(&self) -> String {
        match self {
            Gate::Basic { user, pass } => format!(
                "Basic {}",
                base64::engine::general_purpose::STANDARD.encode(format!("{user}:{pass}"))
            ),
            Gate::Bearer { token } => format!("Bearer {token}"),
        }
    }

    fn rejection_status(&self) -> String {
        match self {
            Gate::Basic { .. } => "401 Unauthorized".to_string(),
            Gate::Bearer { .. } => "401 Unauthorized".to_string(),
        }
    }
}

/// The kind of answer one repository path receives: bytes with a checksum
/// state, an auth gate over one of those, or a rejection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResponseSpec {
    /// The bytes are served exactly as-is, with no published checksum — a
    /// request for them fails the checksum policy hard.
    Content(Vec<u8>),
    /// The bytes, with a **valid** `.sha256` sidecar published for them.
    Sha256(Vec<u8>),
    /// The bytes, with a **valid** `.sha1` sidecar (older remotes only).
    Sha1(Vec<u8>),
    /// The bytes with a Central-style `x-checksum-sha256` response header
    /// and no sidecars, exercising the header-before-sidecar ordering.
    Sha256Header(Vec<u8>),
    /// The bytes, but the published `.sha1` sidecar describes garbage.
    BrokenSha1(Vec<u8>),
    /// Demands [`Gate`] credentials first; accepted requests are answered
    /// with the inner spec, rejected ones with a bare 401 and no body.
    /// Sidecars of a checksummed inner spec are gated identically.
    Gated {
        gate: Gate,
        inner: Box<ResponseSpec>,
    },
    /// A 404 (used for fallback-to-next-repository checks).
    Missing,
}

/// The published sidecar of a spec: its extension, the sidecar bytes, and
/// the gate those bytes live behind (the spec's own, when it is gated).
type Sidecar = ((&'static str, Vec<u8>), Option<Gate>);

impl ResponseSpec {
    /// `(path sidecar extension, sidecar spec)` when this spec publishes a
    /// checksum sidecar for itself.
    fn sidecar(&self) -> Option<Sidecar> {
        match self {
            ResponseSpec::Sha256(bytes) => {
                Some(((".sha256", sidecar_bytes(".sha256", bytes)), None))
            }
            ResponseSpec::Sha1(bytes) => Some(((".sha1", sidecar_bytes(".sha1", bytes)), None)),
            ResponseSpec::BrokenSha1(_) => Some((
                // 40 hex digits, but not the digest of anything real.
                (
                    ".sha1",
                    b"0000000000000000000000000000000000000000  corrupt\n".to_vec(),
                ),
                None,
            )),
            ResponseSpec::Gated { gate, inner } => inner
                .sidecar()
                .map(|((extension, sidecar), _)| ((extension, sidecar), Some(gate.clone()))),
            _ => None,
        }
    }
}

/// One spec per absolute path (no leading `/`); the checksummed variants
/// automatically register their sidecar path too.
#[derive(Default)]
struct SpecTable {
    map: Mutex<HashMap<String, ResponseSpec>>,
    seen: Mutex<Vec<String>>,
    served: AtomicUsize,
    shutdown: AtomicBool,
}

fn sidecar_bytes(hash: &str, bytes: &[u8]) -> Vec<u8> {
    let digest = if hash == ".sha256" {
        hex::encode(sha2::Sha256::digest(bytes))
    } else {
        hex::encode(sha1::Sha1::digest(bytes))
    };
    format!("{digest}  fake-artifact\n").into_bytes()
}

/// A test double of a Maven remote. Records every requested path.
pub struct FakeRepo {
    addr: SocketAddr,
    thread: Option<JoinHandle<()>>,
    table: Arc<SpecTable>,
}

impl FakeRepo {
    /// Start the server. `specs` keys are paths **without** a leading `/`.
    pub fn spawn(specs: Vec<(String, ResponseSpec)>) -> FakeRepo {
        let mut map: HashMap<String, ResponseSpec> = HashMap::new();
        for (path, spec) in specs {
            if let Some(((extension, sidecar), gate)) = spec.sidecar() {
                let sidecar_spec = match gate {
                    Some(gate) => ResponseSpec::Gated {
                        gate: gate.clone(),
                        inner: Box::new(ResponseSpec::Content(sidecar)),
                    },
                    None => ResponseSpec::Content(sidecar),
                };
                map.insert(format!("{path}{extension}"), sidecar_spec);
            }
            map.insert(path, spec);
        }
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind a test port");
        let addr = listener.local_addr().expect("bound");
        let table = Arc::new(SpecTable {
            map: Mutex::new(map),
            seen: Mutex::new(Vec::new()),
            served: AtomicUsize::new(0),
            shutdown: AtomicBool::new(false),
        });
        let worker_table = table.clone();
        let thread = std::thread::spawn(move || {
            // Non-blocking accept loop: blocking `incoming()` would keep the
            // join in `Drop` waiting for a connection that never comes.
            let _ = listener.set_nonblocking(true);
            while !worker_table.shutdown.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let table = worker_table.clone();
                        std::thread::spawn(move || serve(stream, table));
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(std::time::Duration::from_millis(2));
                    }
                    Err(_) => return,
                }
            }
        });
        FakeRepo {
            addr,
            thread: Some(thread),
            table,
        }
    }

    /// The base URL to put in config.toml `url`, no trailing slash.
    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// Every path the server was asked for, in arrival order, annotated
    /// with the Authorization header value like `path auth="Bearer tkn"`.
    pub fn served_paths(&self) -> Vec<String> {
        self.table.seen.lock().expect("seen").clone()
    }

    /// How many responses were sent (auth rejections included).
    pub fn served_count(&self) -> usize {
        self.table.served.load(Ordering::SeqCst)
    }
}

impl Drop for FakeRepo {
    fn drop(&mut self) {
        self.table.shutdown.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Answer one request, then close.
fn serve(mut stream: TcpStream, table: Arc<SpecTable>) {
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(5)));
    // Request line, then headers up to the blank line, byte-at-a-time to
    // keep the framing trivial; requests are tiny by construction.
    let mut raw = Vec::new();
    let mut byte = [0u8; 1];
    for _ in 0..64 * 1024 {
        match stream.read(&mut byte) {
            Ok(0) | Err(_) => return,
            Ok(_) => {
                raw.push(byte[0]);
                if raw.ends_with(b"\r\n\r\n") {
                    break;
                }
            }
        }
    }
    if !raw.ends_with(b"\r\n\r\n") {
        return; // Timed out or malformed; drop the connection without a trace.
    }
    let text = String::from_utf8_lossy(&raw);
    let Some(request_line) = text.lines().next() else {
        return;
    };
    let Some(path) = request_line.split(' ').nth(1) else {
        return;
    };
    let path = path.trim_start_matches('/').to_string();
    let authorization = text
        .lines()
        .skip(1)
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            (name.trim().eq_ignore_ascii_case("authorization")).then(|| value.trim().to_string())
        })
        .unwrap_or_default();

    table
        .seen
        .lock()
        .expect("seen")
        .push(format!("{path} auth={authorization:?}"));
    table.served.fetch_add(1, Ordering::SeqCst);

    let spec = table.map.lock().expect("spec map").get(&path).cloned();
    let response = match spec {
        Some(ResponseSpec::Gated { gate, inner }) => {
            if authorization == gate.accepted_header() {
                answer(&inner)
            } else {
                status_body(&gate.rejection_status(), &[], &[])
            }
        }
        Some(spec) => answer(&spec),
        // Unregistered paths answer 404 like Maven Central would.
        None => status_body("404 Not Found", &[], &[]),
    };
    let _ = stream.write_all(&response);
    let _ = stream.flush();
    let _ = stream.shutdown(std::net::Shutdown::Both);
}

/// The response bytes for one (ungated) spec; `None` only for 404 specs.
fn answer(spec: &ResponseSpec) -> Vec<u8> {
    match spec {
        ResponseSpec::Content(bytes)
        | ResponseSpec::Sha256(bytes)
        | ResponseSpec::Sha1(bytes)
        | ResponseSpec::BrokenSha1(bytes) => ok_body(bytes, &[]),
        ResponseSpec::Sha256Header(bytes) => ok_body(
            bytes,
            &[(
                "x-checksum-sha256".to_string(),
                hex::encode(sha2::Sha256::digest(bytes)),
            )],
        ),
        ResponseSpec::Missing => status_body("404 Not Found", &[], &[]),
        ResponseSpec::Gated { .. } => unreachable!("gates are unwrapped by `serve`"),
    }
}

/// `200 OK` + `Content-Length` + `Connection: close` framing.
fn ok_body(bytes: &[u8], headers: &[(String, String)]) -> Vec<u8> {
    body("200 OK", headers, bytes)
}

fn status_body(status: &str, headers: &[(String, String)], bytes: &[u8]) -> Vec<u8> {
    body(status, headers, bytes)
}

fn body(status: &str, headers: &[(String, String)], bytes: &[u8]) -> Vec<u8> {
    let mut head = format!("HTTP/1.1 {status}\r\n");
    for (name, value) in headers {
        let _ = write!(head, "{name}: {value}\r\n");
    }
    let _ = write!(head, "Content-Length: {}\r\n", bytes.len());
    head.push_str("Connection: close\r\n\r\n");
    let mut response = head.into_bytes();
    response.extend_from_slice(bytes);
    response
}
