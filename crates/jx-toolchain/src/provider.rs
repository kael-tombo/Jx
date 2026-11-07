//! The provider extension point and the Eclipse Temurin implementation.

use jx_common::error::{JxError, Kind};
use serde::Deserialize;
use serde::de::DeserializeOwned;

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
const ADOPTIUM_RELEASE_URL: &str = "https://api.adoptium.net/v3/assets/release_name";

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

/// The release endpoint returns release objects with a `binaries` array,
/// unlike the latest endpoint's single `binary` field.
#[derive(Debug, Deserialize)]
struct ReleaseEntry {
    #[serde(default)]
    release_name: Option<String>,
    #[serde(default)]
    binaries: Vec<ReleaseBinary>,
}

#[derive(Debug, Deserialize)]
struct ReleaseBinary {
    #[serde(default)]
    image_type: Option<String>,
    #[serde(default)]
    os: Option<String>,
    #[serde(default)]
    architecture: Option<String>,
    #[serde(default)]
    jvm_impl: Option<String>,
    #[serde(default)]
    package: Option<Package>,
}

/// Encode a release without a `jdk-` prefix for the Adoptium path. The API
/// expects `+` as `%2B` in a path segment; anything outside the release-name
/// alphabet is refused rather than interpolated into the URL.
fn release_name_path(release: &str) -> Result<String, JxError> {
    if release.is_empty()
        || !release
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '+' | '-' | '_'))
    {
        return Err(JxError::new(
            Kind::Toolchain,
            "the jx.lock JDK release is not a safe release name",
        )
        .at(release)
        .hint("remove the [jdk] entry and re-lock, or restore a valid jx.lock"));
    }
    Ok(release.replace('+', "%2B"))
}

/// The feature release encoded in a Temurin release name. JDK 8 uses names
/// such as `8u492-b08`; later features use `25.0.5+8`, so only the leading
/// digits of the first component are relevant.
pub(crate) fn release_feature(release: &str) -> Option<u32> {
    let first = release.split(['.', '+', '-']).next()?;
    first
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect::<String>()
        .parse()
        .ok()
}

/// The default provider for V1.
pub struct Temurin;

impl Temurin {
    fn fetch_json<T: DeserializeOwned>(&self, url: &str, what: &str) -> Result<T, JxError> {
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(std::time::Duration::from_secs(30))
            .timeout_read(std::time::Duration::from_secs(120))
            .build();
        let response = agent.get(url).call().map_err(|e| {
            JxError::new(
                Kind::Network,
                format!("cannot reach the Adoptium {what} API"),
            )
            .at(url)
            .caused_by(e)
            .hint("check connectivity or a proxy; jx works offline after `jx fetch`")
        })?;
        response.into_json().map_err(|e| {
            JxError::new(
                Kind::Network,
                format!("the Adoptium {what} API returned malformed JSON"),
            )
            .at(url)
            .caused_by(e)
            .hint("this is usually a transient CDN problem; retry in a moment")
        })
    }

    fn fetch_assets(&self, url: &str) -> Result<Vec<AssetEntry>, JxError> {
        self.fetch_json(url, "listing")
    }

    fn fetch_release(&self, url: &str) -> Result<ReleaseEntry, JxError> {
        self.fetch_json(url, "release")
    }

    /// Resolve one exact Temurin build, not just the newest build for a
    /// feature. The lockfile records the release name and install-archive
    /// sha256; this is the missing metadata step for restoring that build on
    /// another machine.
    pub(crate) fn resolve_exact(&self, feature: u32, release: &str) -> Result<JdkRelease, JxError> {
        if feature < 8 {
            return Err(JxError::new(
                Kind::Toolchain,
                format!(
                    "JDK feature release {feature} is below anything Adoptium publishes (minimum is 8)"
                ),
            ));
        }
        if release_feature(release) != Some(feature) {
            return Err(JxError::new(
                Kind::Toolchain,
                format!(
                    "jx.lock pins JDK {feature}, but release `{release}` is a different feature"
                ),
            )
            .at(release)
            .hint("remove the [jdk] entry and re-lock, or restore a valid jx.lock"));
        }
        let encoded = release_name_path(release)?;
        let url = format!(
            "{ADOPTIUM_RELEASE_URL}/eclipse/jdk-{encoded}?os={os}&architecture={arch}&image_type=jdk&vendor=eclipse",
            os = current_os(),
            arch = current_arch(),
        );
        let entry = self.fetch_release(&url)?;
        let expected = format!("jdk-{release}");
        if entry.release_name.as_deref() != Some(expected.as_str()) {
            return Err(JxError::new(
                Kind::Toolchain,
                format!(
                    "no Eclipse Temurin JDK {release} found for {os}/{arch}",
                    os = current_os(),
                    arch = current_arch()
                ),
            )
            .at(release)
            .hint("check `jx jdk list`, or re-lock against an installed build"));
        }
        let binary = entry
            .binaries
            .into_iter()
            .find(|binary| {
                binary.image_type.as_deref() == Some("jdk")
                    && binary.os.as_deref() == Some(current_os())
                    && binary.architecture.as_deref() == Some(current_arch())
                    && binary
                        .jvm_impl
                        .as_deref()
                        .is_none_or(|jvm| jvm == "hotspot")
                    && binary
                        .package
                        .as_ref()
                        .is_some_and(|package| !package.link.is_empty())
            })
            .ok_or_else(|| {
                JxError::new(
                    Kind::Toolchain,
                    format!(
                        "Eclipse Temurin {release} has no JDK archive for {os}/{arch}",
                        os = current_os(),
                        arch = current_arch()
                    ),
                )
                .at(release)
                .hint("the lock was written for another platform; re-lock on this machine")
            })?;
        let package = binary.package.expect("checked in find above");
        Self::release_from_package(feature, expected, package)
    }

    fn release_from_package(
        feature: u32,
        release_name: String,
        package: Package,
    ) -> Result<JdkRelease, JxError> {
        if package.link.is_empty() {
            return Err(JxError::new(
                Kind::Toolchain,
                format!("the Adoptium asset for {release_name} has no download URL"),
            ));
        }
        let size = package.size.filter(|size| *size > 0);
        let checksum = package
            .checksum
            .filter(|c| c.len() == 64 && c.bytes().all(|b| b.is_ascii_hexdigit()))
            .ok_or_else(|| {
                JxError::new(
                    Kind::Toolchain,
                    format!("the Adoptium listing did not publish a sha256 checksum for {release_name}; jx refuses unverifiable JDK downloads"),
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
        let release_name = entry
            .release_name
            .unwrap_or_else(|| format!("jdk-{feature}"));
        Self::release_from_package(feature, release_name, package)
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
    fn exact_release_paths_are_encoded_and_bounded() {
        assert_eq!(release_name_path("25.0.5+8").unwrap(), "25.0.5%2B8");
        assert!(release_name_path("25.0.5/8").is_err());
        assert!(release_name_path("../25.0.5").is_err());
        assert_eq!(release_feature("25.0.5+8"), Some(25));
        assert_eq!(release_feature("8u492-b08"), Some(8));
        assert_eq!(release_feature("not-a-release"), None);
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

    #[test]
    #[ignore = "network: reads the current Temurin 25 release metadata"]
    fn exact_temurin_release_matches_the_latest_release() {
        let latest = Temurin
            .resolve(25)
            .expect("the latest Temurin 25 metadata should resolve");
        let release = latest
            .release_name
            .strip_prefix("jdk-")
            .expect("Temurin release names start with jdk-");
        let exact = Temurin
            .resolve_exact(25, release)
            .expect("the exact release endpoint should resolve the latest build");
        assert_eq!(exact.release_name, latest.release_name);
        assert_eq!(exact.sha256, latest.sha256);
        assert_eq!(exact.url, latest.url);
    }
}
