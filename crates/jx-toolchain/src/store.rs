//! The install store under `<JX_HOME>/jdks`, and the ensure/list/remove API
//! used by the CLI. Installed layout: `jdks/temurin-<version>/…bin/java…`,
//! where `<version>` is the provider release name without its `jdk-` prefix.

use std::path::{Path, PathBuf};

use jx_common::error::{JxError, Kind};
use jx_common::paths;

use crate::extract;
use crate::provider::{JdkProvider as _, Temurin};

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
            &format!("temurin {version}"),
        )?;
    } else {
        verify_cached_archive(&archive, &release.sha256)?;
        println!("  temurin {version}: cache hit ({})", archive.display());
    }
    extract::extract_jdk(&archive, &jdks_dir(home), &format!("temurin-{version}"))?;
    ensure_jdk(home, feature)
}

/// Re-verify a cached archive before reuse: the checksum gate stays closed.
fn verify_cached_archive(archive: &Path, expected: &str) -> Result<(), JxError> {
    use sha2::Digest as _;
    let bytes = std::fs::read(archive).map_err(|e| {
        JxError::new(Kind::Io, "cannot read the cached JDK archive")
            .at(archive.display().to_string())
            .caused_by(e)
    })?;
    let actual = hex::encode(sha2::Sha256::digest(&bytes));
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
    }
}
