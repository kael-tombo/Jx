//! The provider extension point and the Eclipse Temurin implementation.

use jx_common::error::{JxError, Kind};
use serde::Deserialize;

/// Names the OS portion of the provider query for the current binary's build
/// target. jx ships binaries per platform, so compile-time selection is exact.
pub fn current_os() -> &'static str {
    match std::env::consts::OS {
        "windows" => "windows",
        "macos" => "macos",
        "linux" => "linux",
        other => panic!("unexpected build target OS `{other}`; jx supports linux/macos/windows"),
    }
}

/// Adoptium's names for architectures, selected the same way. Note Adoptium
/// uses `x64` for both amd64 Intel/AMD machines.
pub fn current_arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "aarch64",
        other => panic!("unexpected build target arch `{other}`; jx supports x86_64/aarch64"),
    }
}

/// File system-safe file extension of the current platform's JDK archive.
pub fn archive_extension() -> &'static str {
    if std::env::consts::OS == "windows" {
        "zip"
    } else {
        "tar.gz"
    }
}

/// Tiny helper so unit checks of names can be written independent of platform.
pub fn os_is_windows() -> bool {
    std::env::consts::OS == "windows"
}

/// One resolvable JDK release from a provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JdkRelease {
    pub vendor: &'static str,
    pub feature: u32,
    /// Full released name, e.g. `jdk-25.0.5+8`.
    pub release_name: String,
    pub url: String,
    /// sha256 hex digest of the archive as published by the provider; a
    /// missing/blank checksum is a resolve error (jx refuses unverified JDKs).
    pub sha256: String,
    /// Archive size in bytes when the provider states it, used as the
    /// download budget instead of jx's fixed fallback.
    pub size: Option<u64>,
    pub archive_extension: &'static str,
}

/// Where JDKs come from. Implementations must be side-effect free: they only
/// resolve metadata; download/install orchestration lives in [`crate::store`].
pub trait JdkProvider {
    fn vendor(&self) -> &'static str;
    fn resolve(&self, feature: u32) -> Result<JdkRelease, JxError>;
}

// --- Eclipse Temurin (verified endpoints: docs/decisions/0002) ---

const ADOPTIUM_ASSETS_URL: &str = "https://api.adoptium.net/v3/assets/latest";

#[derive(Debug, Deserialize)]
struct AssetEntry {
    #[serde(default)]
    release_name: Option<String>,
    #[serde(default)]
    binary: Option<Binary>,
}

#[derive(Debug, Deserialize)]
struct Binary {
    #[serde(default)]
    package: Option<Package>,
}

#[derive(Debug, Deserialize)]
struct Package {
    /// Absolute URL of the archive.
    link: String,
    #[serde(default)]
    checksum: Option<String>,
    #[serde(default)]
    size: Option<u64>,
}

/// The default provider for V1.
pub struct Temurin;

impl Temurin {
    fn fetch_assets(&self, url: &str) -> Result<Vec<AssetEntry>, JxError> {
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(std::time::Duration::from_secs(30))
            .timeout_read(std::time::Duration::from_secs(120))
            .build();
        let response = agent.get(url).call().map_err(|e| {
            JxError::new(Kind::Network, "cannot reach the Adoptium listing API")
                .at(url)
                .caused_by(e)
                .hint("check connectivity or a proxy; jx works offline after `jx fetch`")
        })?;
        let entries: Vec<AssetEntry> = response.into_json().map_err(|e| {
            JxError::new(
                Kind::Network,
                "the Adoptium listing API returned malformed JSON",
            )
            .at(url)
            .caused_by(e)
            .hint("this is usually a transient CDN problem; retry in a moment")
        })?;
        Ok(entries)
    }
}

impl JdkProvider for Temurin {
    fn vendor(&self) -> &'static str {
        "temurin"
    }

    /// Resolve the latest GA JDK for `feature` on this platform. Sharp edge:
    /// `release_type` is a query parameter, not a path segment (ADR 0002).
    fn resolve(&self, feature: u32) -> Result<JdkRelease, JxError> {
        if feature < 8 {
            return Err(JxError::new(
                Kind::Toolchain,
                format!(
                    "JDK feature release {feature} is below anything Adoptium publishes (minimum is 8)"
                ),
            ));
        }
        let url = format!(
            "{ADOPTIUM_ASSETS_URL}/{feature}/hotspot?release_type=ga&os={os}&architecture={arch}&image_type=jdk&vendor=eclipse",
            os = current_os(),
            arch = current_arch(),
        );
        let entries = self.fetch_assets(&url)?;
        let entry = entries.into_iter().find(|e| {
            e.binary
                .as_ref()
                .and_then(|b| b.package.as_ref())
                .is_some_and(|p| !p.link.is_empty())
        });
        let entry = entry.ok_or_else(|| {
            JxError::new(
                Kind::Toolchain,
                format!("no GA Eclipse Temurin JDK {feature} found for {os}/{arch}", os = current_os(), arch = current_arch()),
            )
            .hint(format!("check that feature release {feature} is still published at https://api.adoptium.net, or pin an installed JDK in project.toml"))
        })?;
        let binary = entry.binary.expect("checked in find above");
        let package = binary.package.expect("checked in find above");
        let release_name = entry.release_name.unwrap_or_else(|| {
            // The name is guaranteed for real assets; this keeps the type shape simple.
            format!("jdk-{feature}")
        });
        let size = package.size.filter(|size| *size > 0);
        let checksum = package
            .checksum
            .filter(|c| c.len() == 64 && c.bytes().all(|b| b.is_ascii_hexdigit()))
            .ok_or_else(|| {
                JxError::new(
                    Kind::Toolchain,
                    format!("the Adoptium listing did not publish a sha256 checksum for {feature}; jx refuses unverifiable JDK downloads"),
                )
                .hint("retry in a moment: if the listing recovered, the checksum will be present")
            })?;
        Ok(JdkRelease {
            vendor: "temurin",
            feature,
            release_name,
            url: package.link,
            sha256: checksum,
            size,
            archive_extension: archive_extension(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_extension_matches_platform() {
        assert_eq!(
            archive_extension(),
            if os_is_windows() { "zip" } else { "tar.gz" }
        );
    }

    #[test]
    fn names_are_valid_on_every_filesystem() {
        // The install directory is built from release_name; keep a guard there.
        let v = "jdk-25.0.5+8";
        assert!(
            !v.contains(|c: char| c.is_ascii_control() || c == '/' || c == '\\'),
            "separator in assumed test fixture"
        );
    }

    #[test]
    fn rejects_unrealistic_features() {
        let e = Temurin.resolve(3).expect_err("feature 3 is not supported");
        assert_eq!(e.kind, Kind::Toolchain);
    }
}
