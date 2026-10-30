//! `jx doctor`: diagnose cache directory, config, network, JDK and disk space,
//! with a fix hint per finding. Checks are read-only except for creating jx's
//! own state directories under $JX_HOME (never the user's system Java).

use std::net::{TcpStream, ToSocketAddrs};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use jx_common::MIN_FREE_DISK_BYTES;
use jx_common::cache::Cache;
use jx_common::config;
use jx_common::error::JxError;
use jx_common::paths;

use crate::Cli;

enum State {
    Ok,
    Warn,
    Fail,
    Info,
}

struct Finding {
    area: &'static str,
    state: State,
    text: String,
}

struct Report {
    findings: Vec<Finding>,
    failures: usize,
    warnings: usize,
}

impl Report {
    fn push(&mut self, area: &'static str, state: State, text: impl Into<String>) {
        if matches!(state, State::Fail) {
            self.failures += 1;
        }
        if matches!(state, State::Warn) {
            self.warnings += 1;
        }
        self.findings.push(Finding {
            area,
            state,
            text: text.into(),
        });
    }
}

pub fn run(cli: &Cli, colors: bool) -> Result<ExitCode, JxError> {
    let p = Palette { on: colors };
    println!(
        "jx {} — checking jx's environment (errors, warnings and info follow)",
        env!("CARGO_PKG_VERSION")
    );
    let mut r = Report {
        findings: Vec::new(),
        failures: 0,
        warnings: 0,
    };

    check_home_and_cache(&mut r);
    check_config(&mut r);
    check_network(&mut r);
    check_java(&mut r);
    check_disk(&mut r);

    for f in &r.findings {
        if cli.quiet && matches!(f.state, State::Ok | State::Info) {
            continue;
        }
        let (mark, style) = match f.state {
            State::Ok => ("ok", p.ok()),
            State::Warn => ("warn", p.warn()),
            State::Fail => ("fail", p.error()),
            State::Info => ("info", p.dim()),
        };
        println!("  {style}{mark:>4}{}  {:<8} {}", p.reset(), f.area, f.text);
    }
    println!(
        "doctor: {} ok, {} warning{}, {} problem{}",
        r.findings.len() - r.warnings - r.failures,
        r.warnings,
        if r.warnings == 1 { "" } else { "s" },
        r.failures,
        if r.failures == 1 { "" } else { "s" }
    );
    if r.failures > 0 {
        return Ok(ExitCode::from(1));
    }
    Ok(ExitCode::SUCCESS)
}

fn check_home_and_cache(r: &mut Report) {
    match paths::jx_home() {
        Ok(home) => {
            let cache = Cache::new(&home);
            match cache.ensure_layout() {
                Ok(()) => {
                    r.push(
                        "home",
                        State::Ok,
                        format!("{} (created/verified)", home.display()),
                    );
                    r.push(
                        "cache",
                        State::Ok,
                        format!("{} writable", cache.root().display()),
                    );
                }
                Err(e) => {
                    r.push("home", State::Ok, format!("{}", home.display()));
                    r.push("cache", State::Fail, e.to_string());
                }
            }
        }
        Err(e) => r.push("home", State::Fail, e.to_string()),
    }
}

fn check_config(r: &mut Report) {
    match paths::jx_home().map(|home| config::load(&home)) {
        Ok(Ok(settings)) => {
            if settings.repositories.is_empty() {
                r.push("config", State::Ok, "no config.toml (defaults OK)");
            } else {
                r.push(
                    "config",
                    State::Ok,
                    format!(
                        "{} custom repositor{}",
                        settings.repositories.len(),
                        if settings.repositories.len() == 1 {
                            "y"
                        } else {
                            "ies"
                        }
                    ),
                );
            }
        }
        Ok(Err(e)) => r.push("config", State::Warn, e.to_string()),
        Err(_) => r.push("config", State::Info, "skipped (home undetermined)"),
    }
}

fn check_network(r: &mut Report) {
    const CENTRAL_HOST: &str = "repo.maven.apache.org";
    let addrs: Vec<std::net::SocketAddr> = match (CENTRAL_HOST, 443_u16).to_socket_addrs() {
        Ok(it) => it.collect(),
        Err(e) => {
            r.push(
                "network",
                State::Warn,
                format!(
                    "cannot resolve {CENTRAL_HOST}: {e}\n         hint: check connectivity — DNS failures behind a proxy are exercised by the M2 fixtures",
                ),
            );
            return;
        }
    };
    let start = Instant::now();
    let mut last_err: Option<std::io::Error> = None;
    for addr in addrs {
        match TcpStream::connect_timeout(&addr, Duration::from_secs(5)) {
            Ok(_) => {
                r.push(
                    "network",
                    State::Ok,
                    format!("{CENTRAL_HOST}:443 reachable in {} ms (TCP; TLS transfers are opened by the M2 repository client)", start.elapsed().as_millis()),
                );
                return;
            }
            Err(e) => last_err = Some(e),
        }
    }
    if let Some(e) = last_err {
        r.push(
            "network",
            State::Warn,
            format!("{CENTRAL_HOST}:443 unreachable: {e}\n         hint: check connectivity or a proxy (proxy support arrives with the M2 repository client) — installs still work offline after `jx fetch`"),
        );
    }
}

fn check_java(r: &mut Report) {
    match probe_java() {
        Some(version) => r.push(
            "jdk",
            State::Info,
            format!("system java {version} on PATH — jx never modifies it; jx-managed JDKs live in ~/.jx/jdks"),
        ),
        None => r.push(
            "jdk",
            State::Info,
            "no java on PATH — fine: jx downloads its own JDK on first use",
        ),
    }
}

/// Best-effort probe of a system java; existence is never an error for jx.
fn probe_java() -> Option<String> {
    let out = std::process::Command::new("java")
        .arg("-version")
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    let line = stderr.lines().next()?;
    let open = line.find('"')?;
    let close = line[open + 1..].find('"')? + open + 1;
    Some(line[open + 1..close].to_string())
}

fn check_disk(r: &mut Report) {
    let target = paths::jx_home();
    if let Ok(home) = target {
        // fs4 wants a directory on the right filesystem/drive.
        let probe_dir = if home.exists() {
            Some(home)
        } else {
            home.parent().map(std::path::PathBuf::from)
        };
        if let Some(dir) = probe_dir {
            match            fs4::free_space(&dir) {
                Ok(free) if free >= MIN_FREE_DISK_BYTES => r.push("disk", State::Ok, bytes_human(free)),
                Ok(free) => r.push(
                    "disk",
                    State::Warn,
                    format!(
                        "{} free — a JDK is ~200 MB and the cache grows; free some space before `jx jdk install`",
                        bytes_human(free)
                    ),
                ),
                Err(e) => r.push("disk", State::Warn, format!("could not query free space: {e}")),
            }
        } else {
            r.push(
                "disk",
                State::Warn,
                "could not determine a filesystem to query",
            );
        }
    } else {
        r.push("disk", State::Info, "skipped (home undetermined)");
    }
}

fn bytes_human(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "kiB", "MiB", "GiB", "TiB"];
    let mut v = bytes as f64;
    let mut unit = 0;
    while v >= 1024.0 && unit < UNITS.len() - 1 {
        v /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{v:.1} {}", UNITS[unit])
    }
}

struct Palette {
    on: bool,
}

impl Palette {
    fn ok(&self) -> &'static str {
        self.codes("\x1b[32m")
    }
    fn warn(&self) -> &'static str {
        self.codes("\x1b[33;1m")
    }
    fn error(&self) -> &'static str {
        self.codes("\x1b[31;1m")
    }
    fn dim(&self) -> &'static str {
        self.codes("\x1b[2m")
    }
    fn reset(&self) -> &'static str {
        self.codes("\x1b[0m")
    }
    fn codes(&self, code: &'static str) -> &'static str {
        if self.on { code } else { "" }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_formatting_is_plain() {
        assert_eq!(bytes_human(0), "0 B");
        assert_eq!(bytes_human(2048), "2.0 kiB");
        assert_eq!(bytes_human(3 * 1024 * 1024 * 1024), "3.0 GiB");
    }

    #[test]
    fn palette_off_is_empty() {
        let p = Palette { on: false };
        assert_eq!(p.ok(), "");
        assert_eq!(p.reset(), "");
    }
}
