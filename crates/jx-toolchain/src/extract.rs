//! Downloading and safely extracting JDK archives.
//!
//! Downloads stream through sha256 verification so nothing unverified is ever
//! kept; extraction rejects path traversal, special files and archives that
//! expand past a fixed budget before creating anything, and the install step
//! expects the JDK archive layout `…/bin/java[.exe]` after stripping one top
//! directory.

use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::time::Duration;

use jx_common::error::{JxError, Kind};

/// The largest JDK archive jx will download when the provider does not state a
/// size. Adoptium's archives are well under this; the cap turns a hostile or
/// broken endpoint into an error instead of an unbounded write.
const MAX_JDK_DOWNLOAD_BYTES: u64 = 512 * 1024 * 1024;

/// The largest extracted tree jx accepts. A normal JDK expands to well under
/// this; the budget stops decompression bombs from filling the disk.
const MAX_EXTRACTED_BYTES: u64 = 4 * 1024 * 1024 * 1024;

/// Entry-count budget, for the same reason as the byte budget.
const MAX_ARCHIVE_ENTRIES: u64 = 200_000;

/// Stream `url` into `dest` (via a `.part` sibling), then atomically rename.
/// Verifies the sha256 of the bytes that actually landed against
/// `expected_sha256`; a mismatch is a hard error and the file is discarded.
/// Progress lines go to stderr only when it is interactive.
pub fn download(
    url: &str,
    dest: &Path,
    expected_sha256: &str,
    expected_size: Option<u64>,
    label: &str,
) -> Result<(), JxError> {
    let part = dest.with_extension("part");
    let max_bytes = expected_size
        .filter(|size| *size > 0)
        .unwrap_or(MAX_JDK_DOWNLOAD_BYTES);
    let mut attempts = 0;
    let actual = loop {
        attempts += 1;
        match download_once(url, &part, label, max_bytes) {
            Ok(digest) => break digest,
            Err(e) if attempts < 3 => {
                std::thread::sleep(Duration::from_secs(1 << attempts));
                eprintln!("  {label} retry {} of 2 after: {e}", attempts);
            }
            Err(e) => {
                std::fs::remove_file(&part).ok();
                return Err(e);
            }
        }
    };
    if !actual.eq_ignore_ascii_case(expected_sha256) {
        std::fs::remove_file(&part).ok();
        return Err(JxError::new(
            Kind::Toolchain,
            "checksum mismatch on the downloaded JDK archive",
        )
        .at(url)
        .hint(format!(
            "expected {expected_sha256}, got {actual}; if this repeats, please report it — jx treats a changed checksum after publish as an error, never a warning"
        )));
    }
    std::fs::rename(&part, dest).map_err(|e| {
        JxError::new(Kind::Io, "cannot finalise the downloaded archive")
            .at(dest.display().to_string())
            .caused_by(e)
    })?;
    Ok(())
}

fn download_once(url: &str, part: &Path, label: &str, max_bytes: u64) -> Result<String, JxError> {
    use sha2::Digest as _;

    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(60))
        .timeout_read(Duration::from_secs(120))
        .build();
    let response = agent.get(url).call().map_err(|e| {
        JxError::new(Kind::Network, "cannot download the JDK archive")
            .at(url)
            .caused_by(e)
            .hint("check connectivity or a proxy; see `jx doctor`")
    })?;
    // A redirect must never take a TLS download down to plaintext. The
    // checksum still protects integrity, but the metadata and any bytes on
    // the wire should not become readable to the network.
    if url.starts_with("https://") && response.get_url().starts_with("http://") {
        return Err(JxError::new(
            Kind::Network,
            "the JDK download was redirected from HTTPS to plain HTTP",
        )
        .at(response.get_url())
        .hint("refusing the downgrade; check the provider or a proxy that rewrites URLs"));
    }
    let total = response
        .header("content-length")
        .and_then(|v| v.parse::<u64>().ok());
    if total.is_some_and(|total| total > max_bytes) {
        return Err(JxError::new(
            Kind::Network,
            format!(
                "the JDK archive is larger than the {} MiB budget",
                max_bytes / (1024 * 1024)
            ),
        )
        .at(url)
        .hint("the provider or a proxy may be serving the wrong file"));
    }
    let interactive = jx_common::color::enabled(jx_common::color::When::Auto);
    let mut reader = response.into_reader();
    let mut out = std::io::BufWriter::new(std::fs::File::create(part).map_err(|e| {
        JxError::new(
            Kind::Io,
            "cannot create a temporary file for the JDK download",
        )
        .at(part.display().to_string())
        .caused_by(e)
    })?);
    let mut hasher = sha2::Sha256::new();
    let mut downloaded: u64 = 0;
    let mut next_report = 16u64 * 1024 * 1024;
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = reader.read(&mut buf).map_err(|e| {
            JxError::new(Kind::Network, "the JDK download was interrupted")
                .at(url)
                .caused_by(e)
        })?;
        if n == 0 {
            break;
        }
        downloaded += n as u64;
        if downloaded > max_bytes {
            return Err(JxError::new(
                Kind::Network,
                format!(
                    "the JDK download passed the {} MiB budget",
                    max_bytes / (1024 * 1024)
                ),
            )
            .at(url)
            .hint("the provider or a proxy may be serving the wrong file"));
        }
        hasher.update(&buf[..n]);
        out.write_all(&buf[..n]).map_err(|e| {
            JxError::new(Kind::Io, "cannot write the JDK download")
                .at(part.display().to_string())
                .caused_by(e)
        })?;
        if interactive && downloaded >= next_report {
            next_report += 16 * 1024 * 1024;
            match total {
                Some(total) if total > 0 => eprint!(
                    "\r  {label} {} / {} MiB",
                    downloaded / (1024 * 1024),
                    total / (1024 * 1024)
                ),
                _ => eprint!("\r  {label} {} MiB", downloaded / (1024 * 1024)),
            }
        }
    }
    if interactive {
        eprint!("\r\x1b[K");
    }
    out.flush().ok();
    Ok(hex::encode(hasher.finalize()))
}

/// Streaming sha256 of a file, used to re-check cached JDK archives without
/// loading a whole JDK into memory.
pub fn sha256_file(path: &Path) -> Result<String, JxError> {
    use sha2::Digest as _;

    let file = std::fs::File::open(path).map_err(|e| {
        JxError::new(Kind::Io, "cannot read the cached JDK archive")
            .at(path.display().to_string())
            .caused_by(e)
    })?;
    let mut reader = std::io::BufReader::new(file);
    let mut hasher = sha2::Sha256::new();
    std::io::copy(&mut reader, &mut hasher).map_err(|e| {
        JxError::new(Kind::Io, "cannot read the cached JDK archive")
            .at(path.display().to_string())
            .caused_by(e)
    })?;
    Ok(hex::encode(hasher.finalize()))
}

/// Validate a name inside an archive: relative, no `..`, no backslashes, no
/// drive letters, no control characters.
pub fn entry_path_is_safe(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('/')
        && !name.contains("//")
        && !name.rsplit('/').any(|seg| seg == "..")
        && !name.contains('\\')
        && !name.contains(':')
        && !name.chars().any(|c| c.is_control())
}

/// Extract `archive` (zip on Windows, gzip tarball elsewhere) into a staging
/// directory under `dest_root`, strip a single top directory, verify the
/// `bin/java` binary exists, then move the result to
/// `dest_root/<final_dir_name>`. The final directory must not exist; on any
/// failure the staging tree is removed and `final_dir_name` is untouched.
pub fn extract_jdk(archive: &Path, dest_root: &Path, final_dir_name: &str) -> Result<(), JxError> {
    if !entry_path_is_safe(final_dir_name) {
        return Err(JxError::new(Kind::Internal, "refusing unsafe install name").at(final_dir_name));
    }
    let staging = dest_root.join(format!(".staging-{final_dir_name}"));
    let final_path = dest_root.join(final_dir_name);
    if final_path.exists() {
        return Err(JxError::new(
            Kind::Toolchain,
            "a JDK directory already occupies the target install location",
        )
        .at(final_path.display().to_string())
        .hint(
            "`jx jdk list` shows what is installed; remove it first or point project.toml at it",
        ));
    }
    std::fs::create_dir_all(&staging).map_err(|e| {
        JxError::new(
            Kind::Io,
            "cannot create the staging directory for extraction",
        )
        .at(staging.display().to_string())
        .caused_by(e)
    })?;
    let result = if std::env::consts::OS == "windows" {
        extract_zip(archive, &staging)
    } else {
        extract_tar_gz(archive, &staging)
    }
    .and_then(|()| strip_single_top_dir(&staging))
    .and_then(|()| verify_java_binary(&staging))
    .and_then(|()| {
        std::fs::rename(&staging, &final_path).map_err(|e| {
            JxError::new(Kind::Io, "cannot move the extracted JDK into place")
                .at(staging.display().to_string())
                .caused_by(e)
        })
    });
    if let Err(e) = result {
        std::fs::remove_dir_all(&staging).ok();
        return Err(e);
    }
    Ok(())
}

fn extract_tar_gz(archive: &Path, staging: &Path) -> Result<(), JxError> {
    let file = std::fs::File::open(archive).map_err(|e| {
        JxError::new(Kind::Io, "cannot open the downloaded JDK archive")
            .at(archive.display().to_string())
            .caused_by(e)
    })?;
    let gz = flate2::read::GzDecoder::new(std::io::BufReader::new(file));
    let mut archive_tar = tar::Archive::new(gz);
    let mut entries_seen: u64 = 0;
    let mut expanded: u64 = 0;
    for entry in archive_tar.entries().map_err(|e| {
        JxError::new(
            Kind::Toolchain,
            "the JDK tarball is corrupt and cannot be read",
        )
        .at(archive.display().to_string())
        .caused_by(e)
    })? {
        let mut entry = entry.map_err(|e| {
            JxError::new(Kind::Toolchain, "cannot read an entry of the JDK tarball")
                .at(archive.display().to_string())
                .caused_by(e)
        })?;
        entries_seen += 1;
        if entries_seen > MAX_ARCHIVE_ENTRIES {
            return Err(
                JxError::new(Kind::Toolchain, "the JDK archive contains too many entries")
                    .at(archive.display().to_string())
                    .hint("the archive is corrupt or not a JDK; remove the cached file and retry"),
            );
        }
        let kind = entry.header().entry_type();
        if !(kind.is_file() || kind.is_dir() || kind.is_symlink() || kind.is_hard_link()) {
            return Err(
                JxError::new(Kind::Toolchain, "the JDK archive contains a special file")
                    .at(archive.display().to_string())
                    .hint("jx extracts regular files, directories and links only"),
            );
        }
        expanded = expanded
            .checked_add(entry.size())
            .filter(|total| *total <= MAX_EXTRACTED_BYTES)
            .ok_or_else(|| {
                JxError::new(
                    Kind::Toolchain,
                    "the JDK archive expands past the 4 GiB budget",
                )
                .at(archive.display().to_string())
                .hint("the archive is corrupt or not a JDK; remove the cached file and retry")
            })?;
        let name = entry.path().map_err(|e| {
            JxError::new(
                Kind::Toolchain,
                "cannot read an entry path of the JDK tarball",
            )
            .caused_by(e)
        })?;
        let name = name.to_string_lossy().into_owned();
        if !entry_path_is_safe(&name) {
            return Err(JxError::new(
                Kind::Toolchain,
                "the JDK archive contains an unsafe entry path",
            )
            .at(name.clone())
            .hint("jx refuses archives with absolute or traversing paths (checklist item for M8);\n         the archive may be corrupt; remove the cache file and retry"));
        }
        if let Some(link) = entry.link_name().map_err(|e| {
            JxError::new(
                Kind::Toolchain,
                "cannot read a link target of the JDK tarball",
            )
            .caused_by(e)
        })? {
            let link = link.to_string_lossy().into_owned();
            if !entry_path_is_safe(&link) {
                return Err(JxError::new(
                    Kind::Toolchain,
                    "the JDK archive contains a symlink leaving its top directory",
                )
                .at(name.clone())
                .hint("jx refuses archives whose links escape the extracted tree"));
            }
        }
        entry.unpack_in(staging).map_err(|e| {
            JxError::new(
                Kind::Toolchain,
                "cannot extract an entry of the JDK tarball",
            )
            .at(name.clone())
            .caused_by(e)
        })?;
    }
    Ok(())
}

fn extract_zip(archive: &Path, staging: &Path) -> Result<(), JxError> {
    let file = std::fs::File::open(archive).map_err(|e| {
        JxError::new(Kind::Io, "cannot open the downloaded JDK archive")
            .at(archive.display().to_string())
            .caused_by(e)
    })?;
    let mut reader = zip::ZipArchive::new(std::io::BufReader::new(file)).map_err(|e| {
        JxError::new(Kind::Toolchain, "the JDK zip is corrupt and cannot be read")
            .at(archive.display().to_string())
            .caused_by(e)
    })?;
    let mut expanded: u64 = 0;
    for i in 0..reader.len() {
        let mut entry = reader.by_index(i).map_err(|e| {
            JxError::new(Kind::Toolchain, "cannot read an entry of the JDK zip")
                .at(archive.display().to_string())
                .caused_by(e)
        })?;
        let name = entry.name().to_string();
        if entry.encrypted() {
            return Err(JxError::new(
                Kind::Toolchain,
                "the JDK archive contains an encrypted entry",
            )
            .at(name));
        }
        expanded = expanded
            .checked_add(entry.size())
            .filter(|total| *total <= MAX_EXTRACTED_BYTES)
            .ok_or_else(|| {
                JxError::new(
                    Kind::Toolchain,
                    "the JDK archive expands past the 4 GiB budget",
                )
                .at(archive.display().to_string())
                .hint("the archive is corrupt or not a JDK; remove the cached file and retry")
            })?;
        if !entry_path_is_safe(&name) {
            return Err(JxError::new(
                Kind::Toolchain,
                "the JDK archive contains an unsafe entry path",
            )
            .at(name.clone())
            .hint("jx refuses archives with absolute or traversing paths"));
        }
        let target = staging.join(&name);
        if entry.is_dir() {
            std::fs::create_dir_all(&target).map_err(|e| {
                JxError::new(Kind::Toolchain, "cannot create a directory of the JDK")
                    .at(target.display().to_string())
                    .caused_by(e)
            })?;
            continue;
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                JxError::new(Kind::Toolchain, "cannot create a directory of the JDK")
                    .at(parent.display().to_string())
                    .caused_by(e)
            })?;
        }
        let mut out = std::fs::File::create(&target).map_err(|e| {
            JxError::new(Kind::Toolchain, "cannot write an entry of the JDK")
                .at(target.display().to_string())
                .caused_by(e)
        })?;
        std::io::copy(&mut entry, &mut out).map_err(|e| {
            JxError::new(Kind::Toolchain, "cannot extract an entry of the JDK")
                .at(target.display().to_string())
                .caused_by(e)
        })?;
    }
    Ok(())
}

/// If extraction produced exactly one top directory, splice its contents up a
/// level so `<staging>/bin/java` is the layout; otherwise leave as-is.
fn strip_single_top_dir(staging: &Path) -> Result<(), JxError> {
    let entries: Vec<std::fs::DirEntry> = std::fs::read_dir(staging)
        .map_err(|e| {
            JxError::new(Kind::Io, "cannot list the extracted JDK")
                .at(staging.display().to_string())
                .caused_by(e)
        })?
        .filter_map(|it| it.ok())
        .collect();
    let mut dirs: Vec<PathBuf> = Vec::new();
    for e in &entries {
        match e.metadata().map_err(|err| {
            JxError::new(Kind::Io, "cannot stat the extracted JDK")
                .at(e.path().display().to_string())
                .caused_by(err)
        })? {
            m if m.is_dir() => dirs.push(e.path()),
            _ => return Ok(()), // a stray file at top: keep layout, verification decides
        }
    }
    if dirs.len() != 1 {
        return Ok(());
    }
    let dir = dirs.into_iter().next().expect("len 1 checked");
    move_children(&dir, staging)?;
    std::fs::remove_dir(&dir).ok();
    Ok(())
}

fn move_children(from: &Path, to: &Path) -> Result<(), JxError> {
    for entry in std::fs::read_dir(from).map_err(|e| {
        JxError::new(Kind::Io, "cannot list extracted JDK levels")
            .at(from.display().to_string())
            .caused_by(e)
    })? {
        let entry = entry
            .map_err(|e| JxError::new(Kind::Io, "cannot list extracted JDK levels").caused_by(e))?;
        std::fs::rename(entry.path(), to.join(entry.file_name())).map_err(|e| {
            JxError::new(Kind::Io, "cannot consolidate the extracted JDK")
                .at(entry.path().display().to_string())
                .caused_by(e)
        })?;
    }
    Ok(())
}

/// Confirm `bin/java[.exe]` exists after extraction; else the layout is not a
/// JDK jx understands and the install is refused.
fn verify_java_binary(root: &Path) -> Result<(), JxError> {
    let bin = root.join("bin");
    let java = if std::env::consts::OS == "windows" {
        bin.join("java.exe")
    } else {
        bin.join("java")
    };
    if !java.is_file() {
        return Err(JxError::new(
            Kind::Toolchain,
            "the extracted archive does not look like a JDK (no bin/java)",
        )
        .at(root.display().to_string())
        .hint("the archive may be corrupt or an unexpected image type; retry the download"));
    }
    Ok(())
}

/// Convenience for tests and doctor-style introspection: where archives land
/// before extraction (never in the user's system directories).
pub fn staging_dir_for(dest_root: &Path, final_dir_name: &str) -> PathBuf {
    dest_root.join(format!(".staging-{final_dir_name}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_paths() {
        assert!(entry_path_is_safe("jdk-25.0.5+8/bin/java"));
        assert!(entry_path_is_safe("jdk-25.0.5+8/release"));
        assert!(!entry_path_is_safe("../evil"));
        assert!(!entry_path_is_safe("/absolute"));
        for evil in ["a\\b", "C:/x", "a..b/..", "//share", "nul\x00", "a/../b"] {
            assert!(!entry_path_is_safe(evil), "should reject: {evil:?}");
        }
    }

    #[test]
    fn staging_name_is_predictable() {
        assert_eq!(
            staging_dir_for(Path::new("/x"), "temurin-25.0.5+8"),
            PathBuf::from("/x/.staging-temurin-25.0.5+8")
        );
    }
}
