//! The install store under `<JX_HOME>/jdks`, and the ensure/list/remove API
//! used by the CLI. Installed layout: `jdks/temurin-<version>/…bin/java…`,
//! where `<version>` is the provider release name without its `jdk-` prefix.

use std::path::{Path, PathBuf};

use jx_common::error::{JxError, Kind};
use jx_common::paths;

use crate::extract;
use crate::provider::{JdkProvider as _, Temurin, release_feature};

/// A JDK installed by jx.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledJdk {
    /// Vendor, e.g. `temurin` (matches the directory prefix).
    pub vendor: String,
    /// Version without the `jdk-` prefix, e.g. `25.0.5+8`.
    pub version: String,
    /// Expanded directory: `<JX_HOME>/jdks/temurin-<version>`.
    pub dir: PathBuf,
}

impl InstalledJdk {
    pub fn java(&self) -> PathBuf {
        with_platform_bin(self.dir.clone(), "java")
    }

    pub fn javac(&self) -> PathBuf {
        with_platform_bin(self.dir.clone(), "javac")
    }

    /// The module analyser used by `jx image`.
    pub fn jdeps(&self) -> PathBuf {
        with_platform_bin(self.dir.clone(), "jdeps")
    }

    /// The runtime-image linker used by `jx image`.
    pub fn jlink(&self) -> PathBuf {
        with_platform_bin(self.dir.clone(), "jlink")
    }
}

fn with_platform_bin(dir: PathBuf, tool: &str) -> PathBuf {
    let mut dir = dir;
    dir.push("bin");
    dir.push(match std::env::consts::OS {
        "windows" => format!("{tool}.exe"),
        _ => tool.to_string(),
    });
    dir
}

/// Directory holding jx's installed JDKs.
pub fn jdks_dir(home: &Path) -> PathBuf {
    home.join("jdks")
}

fn strip_jdk_prefix(release_name: &str) -> Result<String, JxError> {
    let version = release_name.strip_prefix("jdk-").ok_or_else(|| {
        JxError::new(Kind::Toolchain, "unexpected JDK release naming")
            .at(release_name.to_string())
            .hint(
                "this provider normally names releases `jdk-<version>`; jx refuses ambiguous names",
            )
    })?;
    if version.is_empty() || !extract::entry_path_is_safe(version) {
        return Err(
            JxError::new(Kind::Toolchain, "refusing an unsafe JDK version name")
                .at(version.to_string()),
        );
    }
    Ok(version.to_string())
}

fn version_tokens(version: &str) -> Vec<u64> {
    version
        .split(['.', '+'])
        .map(|t| t.parse::<u64>().unwrap_or(0))
        .collect()
}

/// All installed JDKs, newest first.
pub fn list_installed(home: &Path) -> Result<Vec<InstalledJdk>, JxError> {
    let dir = jdks_dir(home);
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut found = Vec::new();
    for entry in std::fs::read_dir(&dir).map_err(|e| {
        JxError::new(Kind::Io, "cannot list the jx JDK directory")
            .at(dir.display().to_string())
            .caused_by(e)
    })? {
        let entry = entry
            .map_err(|e| JxError::new(Kind::Io, "cannot list the jx JDK directory").caused_by(e))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(version) = name.strip_prefix("temurin-") else {
            continue; // not ours; never touch foreign directories
        };
        let path = dir.join(&name);
        if path.join("release").is_file() || path.join("bin").is_dir() {
            found.push(InstalledJdk {
                vendor: "temurin".to_string(),
                version: version.to_string(),
                dir: path,
            });
        }
    }
    found.sort_by_key(|j| std::cmp::Reverse(version_tokens(&j.version)));
    Ok(found)
}

/// The newest installed JDK for a feature release, if any.
pub fn installed_for_feature(home: &Path, feature: u32) -> Result<Option<InstalledJdk>, JxError> {
    let best = list_installed(home)?.into_iter().find(|j| {
        version_tokens(&j.version)
            .first()
            .is_some_and(|f| *f as u32 == feature)
    });
    Ok(best)
}

/// The installed JDK matching an exact provider release, if any.
pub fn installed_exact(
    home: &Path,
    vendor: &str,
    release: &str,
) -> Result<Option<InstalledJdk>, JxError> {
    Ok(list_installed(home)?
        .into_iter()
        .find(|jdk| jdk.vendor == vendor && jdk.version == release))
}

/// A cached JDK install archive for `version`, if one is present. The
/// extension depends on the platform and provider, so match the version
/// prefix rather than guessing the suffix.
pub fn cached_archive(home: &Path, version: &str) -> Result<Option<PathBuf>, JxError> {
    let dir = home.join("cache").join("jdks");
    if !dir.is_dir() {
        return Ok(None);
    }
    let prefix = format!("{version}.");
    let mut found = Vec::new();
    for entry in std::fs::read_dir(&dir).map_err(|e| {
        JxError::new(Kind::Io, "cannot list the cached JDK archives")
            .at(dir.display().to_string())
            .caused_by(e)
    })? {
        let path = entry
            .map_err(|e| {
                JxError::new(Kind::Io, "cannot list the cached JDK archives").caused_by(e)
            })?
            .path();
        if path.is_file()
            && path
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with(&prefix))
        {
            found.push(path);
        }
    }
    found.sort();
    Ok(found.into_iter().next())
}

/// Install (or return cached) JDK for `feature`, streaming progress text.
pub fn ensure_jdk(home: &Path, feature: u32) -> Result<InstalledJdk, JxError> {
    if let Some(found) = installed_for_feature(home, feature)? {
        return Ok(found);
    }
    let release = Temurin.resolve(feature)?;
    let version = strip_jdk_prefix(&release.release_name)?;
    let archive = home
        .join("cache")
        .join("jdks")
        .join(format!("{version}.{}", release.archive_extension));
    std::fs::create_dir_all(archive.parent().expect("archive has a parent")).map_err(|e| {
        JxError::new(Kind::Io, "cannot create the JDK archive cache directory")
            .at(archive.display().to_string())
            .caused_by(e)
    })?;
    if !archive.exists() {
        extract::download(
            &release.url,
            &archive,
            &release.sha256,
            release.size,
            &format!("temurin {version}"),
        )?;
    } else {
        verify_cached_archive(&archive, &release.sha256)?;
        println!("  temurin {version}: cache hit ({})", archive.display());
    }
    extract::extract_jdk(&archive, &jdks_dir(home), &format!("temurin-{version}"))?;
    ensure_jdk(home, feature)
}

/// Restore a specific Temurin release recorded in `jx.lock`. Unlike
/// [`ensure_jdk`], this never substitutes a newer build of the same feature.
/// A matching installed JDK or cached archive is used without touching the
/// network; `--offline` refuses only when neither exists.
pub fn ensure_jdk_release(
    home: &Path,
    provider: &str,
    feature: u32,
    release: &str,
    expected_sha256: &str,
    offline: bool,
) -> Result<InstalledJdk, JxError> {
    if provider != "temurin" {
        return Err(JxError::new(
            Kind::Toolchain,
            format!("jx.lock pins provider `{provider}`, which this jx does not install"),
        )
        .hint("V1 installs Eclipse Temurin only; change the pin deliberately"));
    }
    if !is_sha256(expected_sha256) {
        return Err(JxError::new(
            Kind::Lock,
            "the [jdk] checksum in jx.lock is not a 64-character sha256",
        )
        .at("jx.lock")
        .hint("remove the [jdk] entry and re-lock, or restore a valid jx.lock"));
    }
    let version = exact_release_version(release)?;
    if release_feature(&version) != Some(feature) {
        return Err(JxError::new(
            Kind::Lock,
            format!("jx.lock pins JDK {feature}, but release `{version}` is a different feature"),
        )
        .at("jx.lock")
        .hint("remove the [jdk] entry and re-lock, or restore a valid jx.lock"));
    }
    if let Some(found) = installed_exact(home, provider, &version)? {
        return Ok(found);
    }

    if let Some(archive) = cached_archive(home, &version)? {
        let actual = extract::sha256_file(&archive)?;
        if actual.eq_ignore_ascii_case(expected_sha256) {
            extract::extract_jdk(&archive, &jdks_dir(home), &format!("temurin-{version}"))?;
            return finish_exact_install(home, provider, &version);
        }
        std::fs::remove_file(&archive).ok();
        if offline {
            return Err(JxError::new(
                Kind::Toolchain,
                "the cached JDK archive does not match the checksum pinned in jx.lock",
            )
            .at(archive.display().to_string())
            .hint("the corrupt cache file was removed; run `jx fetch` once online"));
        }
    } else if offline {
        return Err(JxError::new(
            Kind::Toolchain,
            format!(
                "jx.lock pins temurin {version}, which is not installed or cached, and --offline forbids downloading"
            ),
        )
        .hint("run `jx fetch` once online to install the exact locked JDK"));
    }

    let resolved = Temurin.resolve_exact(feature, &version)?;
    if resolved.release_name != format!("jdk-{version}") {
        return Err(JxError::new(
            Kind::Toolchain,
            format!(
                "the provider resolved `{}` instead of the exact locked release",
                resolved.release_name
            ),
        )
        .at(version));
    }
    if !resolved.sha256.eq_ignore_ascii_case(expected_sha256) {
        return Err(JxError::new(
            Kind::Toolchain,
            format!("the provider's checksum for temurin {version} does not match jx.lock"),
        )
        .at(version)
        .hint("the archive may have been replaced; re-lock deliberately if that is expected"));
    }

    let archive = home
        .join("cache")
        .join("jdks")
        .join(format!("{version}.{}", resolved.archive_extension));
    std::fs::create_dir_all(archive.parent().expect("archive has a parent")).map_err(|e| {
        JxError::new(Kind::Io, "cannot create the JDK archive cache directory")
            .at(archive.display().to_string())
            .caused_by(e)
    })?;
    if archive.exists() {
        verify_cached_archive(&archive, expected_sha256)?;
    } else {
        extract::download(
            &resolved.url,
            &archive,
            expected_sha256,
            resolved.size,
            &format!("temurin {version}"),
        )?;
    }
    extract::extract_jdk(&archive, &jdks_dir(home), &format!("temurin-{version}"))?;
    finish_exact_install(home, provider, &version)
}

fn exact_release_version(release: &str) -> Result<String, JxError> {
    if release.is_empty() || release.starts_with("jdk-") || !extract::entry_path_is_safe(release) {
        return Err(
            JxError::new(Kind::Toolchain, "refusing an unsafe exact JDK release name")
                .at(release)
                .hint("remove the [jdk] entry and re-lock, or restore a valid jx.lock"),
        );
    }
    Ok(release.to_string())
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn finish_exact_install(
    home: &Path,
    provider: &str,
    version: &str,
) -> Result<InstalledJdk, JxError> {
    installed_exact(home, provider, version)?.ok_or_else(|| {
        JxError::new(
            Kind::Internal,
            "the JDK was extracted but is not visible in the jx store",
        )
        .at(jdks_dir(home)
            .join(format!("temurin-{version}"))
            .display()
            .to_string())
        .hint("retry; if it persists, remove the partial directory and run `jx fetch`")
    })
}

/// Re-verify a cached archive before reuse: the checksum gate stays closed.
fn verify_cached_archive(archive: &Path, expected: &str) -> Result<(), JxError> {
    let actual = extract::sha256_file(archive)?;
    if !actual.eq_ignore_ascii_case(expected) {
        std::fs::remove_file(archive).ok();
        return Err(JxError::new(
            Kind::Toolchain,
            "checksum mismatch on the cached JDK archive",
        )
        .at(archive.display().to_string())
        .hint("the corrupt cache file was removed; retry"));
    }
    Ok(())
}

/// Remove an installed JDK by its version string (e.g. `25.0.5+8`).
/// Only directories matching `temurin-<safe-version>` inside the store are
/// touched; the user's system Java is out of reach by construction.
pub fn remove_installed(home: &Path, version: &str) -> Result<(), JxError> {
    if strip_jdk_prefix(&format!("jdk-{version}"))? != version {
        return Err(JxError::new(Kind::Toolchain, "invalid JDK version name").at(version));
    }
    if !extract::entry_path_is_safe(version) {
        return Err(JxError::new(Kind::Toolchain, "invalid JDK version name").at(version));
    }
    let target = jdks_dir(home).join(format!("temurin-{version}"));
    if !target.is_dir() {
        return Err(JxError::new(Kind::Toolchain, "no such jx JDK is installed")
            .at(version)
            .hint("`jx jdk list` shows the installed versions"));
    }
    std::fs::remove_dir_all(&target).map_err(|e| {
        JxError::new(Kind::Io, "cannot remove the requested JDK")
            .at(target.display().to_string())
            .caused_by(e)
            .hint("close any running command that uses it first")
    })?;
    Ok(())
}

/// Default feature release jx pins (`new`, installs without version argument):
/// the current LTS verified in ADR 0002. Bump it with the ADR.
pub const DEFAULT_FEATURE: u32 = 25;

pub fn default_home() -> Result<PathBuf, JxError> {
    paths::jx_home()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(windows)]
    use std::io::Write as _;

    fn write_fake_jdk_archive(path: &Path) {
        #[cfg(windows)]
        {
            let file = std::fs::File::create(path).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated)
                .unix_permissions(0o755);
            zip.start_file("jdk-25.0.5+8/bin/java.exe", options)
                .unwrap();
            zip.write_all(b"fake").unwrap();
            zip.finish().unwrap();
        }
        #[cfg(not(windows))]
        {
            let file = std::fs::File::create(path).unwrap();
            let gz = flate2::write::GzEncoder::new(file, flate2::Compression::default());
            let mut tar = tar::Builder::new(gz);
            let mut header = tar::Header::new_gnu();
            header.set_path("jdk-25.0.5+8/bin/java").unwrap();
            header.set_size(4);
            header.set_mode(0o755);
            header.set_cksum();
            tar.append(&header, &b"fake"[..]).unwrap();
            let gz = tar.into_inner().unwrap();
            gz.finish().unwrap();
        }
    }

    #[test]
    fn exact_release_installs_from_a_verified_cache_offline() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("cache").join("jdks");
        std::fs::create_dir_all(&cache).unwrap();
        let archive = cache.join(format!("25.0.5+8.{}", crate::provider::archive_extension()));
        write_fake_jdk_archive(&archive);
        let checksum = extract::sha256_file(&archive).unwrap();

        let jdk =
            ensure_jdk_release(dir.path(), "temurin", 25, "25.0.5+8", &checksum, true).unwrap();
        assert_eq!(jdk.version, "25.0.5+8");
        assert!(jdk.java().is_file());
        assert!(archive.is_file());
    }

    #[test]
    fn exact_release_offline_without_cache_is_actionable() {
        let dir = tempfile::tempdir().unwrap();
        let err = ensure_jdk_release(dir.path(), "temurin", 25, "25.0.5+8", &"a".repeat(64), true)
            .unwrap_err();
        assert_eq!(err.kind, Kind::Toolchain);
        assert!(err.message.contains("not installed or cached"), "{err:?}");
        assert!(
            err.hint.unwrap_or_default().contains("jx fetch"),
            "offline hint should name jx fetch"
        );
    }

    #[test]
    fn exact_release_installed_is_returned_without_network() {
        let dir = tempfile::tempdir().unwrap();
        let jdk = jdks_dir(dir.path()).join("temurin-25.0.5+8");
        std::fs::create_dir_all(jdk.join("bin")).unwrap();
        std::fs::write(jdk.join("release"), "JAVA_VERSION=25").unwrap();

        let found =
            ensure_jdk_release(dir.path(), "temurin", 25, "25.0.5+8", &"0".repeat(64), true)
                .unwrap();
        assert_eq!(found.version, "25.0.5+8");
    }

    #[test]
    fn exact_release_rejects_a_feature_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let err = ensure_jdk_release(dir.path(), "temurin", 21, "25.0.5+8", &"0".repeat(64), true)
            .unwrap_err();
        assert_eq!(err.kind, Kind::Lock);
        assert!(err.message.contains("different feature"), "{err:?}");
    }

    #[test]
    fn exact_release_removes_a_mismatched_cache_offline() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("cache").join("jdks");
        std::fs::create_dir_all(&cache).unwrap();
        let archive = cache.join(format!("25.0.5+8.{}", crate::provider::archive_extension()));
        std::fs::write(&archive, b"not the pinned bytes").unwrap();

        let err = ensure_jdk_release(dir.path(), "temurin", 25, "25.0.5+8", &"0".repeat(64), true)
            .unwrap_err();
        assert_eq!(err.kind, Kind::Toolchain);
        assert!(err.message.contains("cached JDK archive"), "{err:?}");
        assert!(cached_archive(dir.path(), "25.0.5+8").unwrap().is_none());
    }

    #[test]
    fn version_ordering_is_semverish() {
        assert!(version_tokens("25.0.5+8") > version_tokens("25.0.5+7"));
        assert!(version_tokens("26.0.0") > version_tokens("25.9.9"));
        assert_eq!(version_tokens("25.0.5+8"), vec![25, 0, 5, 8]);
        assert!(version_tokens("25.0.6") > version_tokens("25.0.5+9"));
    }

    #[test]
    fn empty_store_lists_nothing() {
        let dir = tempfile::tempdir().unwrap();
        assert!(list_installed(dir.path()).unwrap().is_empty());
        assert!(installed_for_feature(dir.path(), 25).unwrap().is_none());
    }

    #[test]
    fn store_listing_uses_release_files_and_ignores_foreign_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let store = jdks_dir(dir.path());
        let jdk = store.join("temurin-25.0.5+8");
        std::fs::create_dir_all(jdk.join("bin")).unwrap();
        std::fs::write(jdk.join("release"), "JAVA_VERSION=25").unwrap();
        let jdk2 = store.join("temurin-21.0.3+9");
        std::fs::create_dir_all(jdk2.join("bin")).unwrap();
        std::fs::write(jdk2.join("release"), "JAVA_VERSION=21").unwrap();
        let foreign = store.join("random-stuff");
        std::fs::create_dir_all(&foreign).unwrap();

        let list = list_installed(dir.path()).unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].version, "25.0.5+8"); // newest first
        assert_eq!(
            installed_for_feature(dir.path(), 25)
                .unwrap()
                .unwrap()
                .version,
            "25.0.5+8"
        );
        assert!(foreign.is_dir());
    }

    #[test]
    fn exact_lookup_ignores_other_builds() {
        let dir = tempfile::tempdir().unwrap();
        let store = jdks_dir(dir.path());
        let jdk = store.join("temurin-25.0.5+8");
        std::fs::create_dir_all(jdk.join("bin")).unwrap();
        std::fs::write(jdk.join("release"), "JAVA_VERSION=25").unwrap();

        assert_eq!(
            installed_exact(dir.path(), "temurin", "25.0.5+8")
                .unwrap()
                .unwrap()
                .version,
            "25.0.5+8"
        );
        assert!(
            installed_exact(dir.path(), "temurin", "25.0.5+7")
                .unwrap()
                .is_none()
        );
        assert!(
            installed_exact(dir.path(), "other", "25.0.5+8")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn cached_archive_matches_the_version_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("cache").join("jdks");
        std::fs::create_dir_all(&cache).unwrap();
        std::fs::write(cache.join("25.0.5+80.tar.gz"), b"not this one").unwrap();
        std::fs::write(cache.join("25.0.5+8.tar.gz"), b"this one").unwrap();

        let found = cached_archive(dir.path(), "25.0.5+8")
            .unwrap()
            .expect("archive for the exact version");
        assert_eq!(found.file_name().unwrap(), "25.0.5+8.tar.gz");
        assert!(cached_archive(dir.path(), "21.0.1").unwrap().is_none());
    }

    #[test]
    fn tool_paths_match_platform() {
        let dir = tempfile::tempdir().unwrap();
        let j = InstalledJdk {
            vendor: "temurin".into(),
            version: "25.0.1".into(),
            dir: dir.path().join("temurin-25.0.1"),
        };
        let expected = if std::env::consts::OS == "windows" {
            "java.exe"
        } else {
            "java"
        };
        assert!(j.java().ends_with(expected));
        let expected = if std::env::consts::OS == "windows" {
            "javac.exe"
        } else {
            "javac"
        };
        assert_eq!(j.javac().file_name().unwrap(), expected);
        let expected = if std::env::consts::OS == "windows" {
            "jdeps.exe"
        } else {
            "jdeps"
        };
        assert_eq!(j.jdeps().file_name().unwrap(), expected);
        let expected = if std::env::consts::OS == "windows" {
            "jlink.exe"
        } else {
            "jlink"
        };
        assert_eq!(j.jlink().file_name().unwrap(), expected);
    }
}
