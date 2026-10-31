//! The content-addressed artifact cache.
//!
//! Every artifact jx downloads is stored under `<home>/cache/artifacts/` keyed
//! by its sha256, and verified again on every read (both checksum requirements
//! come from the brief: computed from the downloaded bytes and re-checked at
//! read time). A changed checksum for an already-stored version is surfaced as
//! a hard error by higher layers; this layer just guarantees verified reads.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::error::{JxError, Kind};

/// Where the sha256 of a id lives: `content/<aa>/<sha256hex>` with both-hex
/// shard directory so no directory ever holds thousands of entries.
fn content_path(root: &Path, sha256_hex: &str) -> PathBuf {
    let aa = &sha256_hex[..2];
    root.join("artifacts")
        .join("content")
        .join(aa)
        .join(sha256_hex)
}

pub struct Cache {
    root: PathBuf,
}

impl Cache {
    /// Construct the cache for the home directory path, without creating it.
    pub fn new(home: &Path) -> Cache {
        Cache {
            root: home.join("cache"),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Create the directory layout. Safe to call repeatedly.
    pub fn ensure_layout(&self) -> Result<(), JxError> {
        std::fs::create_dir_all(self.root.join("artifacts").join("content")).map_err(|e| {
            JxError::new(Kind::Io, "cannot create the cache directory")
                .at(self.root.display().to_string())
                .caused_by(e)
                .hint("set JX_HOME to a writable directory and run `jx doctor`")
        })
    }

    fn sha256_hex(content: &[u8]) -> String {
        hex::encode(Sha256::digest(content))
    }

    /// Store bytes if not present, returning their hex sha256.
    /// Content addressing makes this idempotent.
    pub fn put(&self, content: &[u8]) -> Result<String, JxError> {
        self.ensure_layout()?;
        let hex = Self::sha256_hex(content);
        let path = content_path(&self.root, &hex);
        if !path.exists() {
            // Write to a temp name in the same directory, then rename for
            // atomicity; concurrent invokers never observe a partial file.
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir).map_err(|e| {
                    JxError::new(Kind::Io, "cannot create a cache shard directory")
                        .at(dir.display().to_string())
                        .caused_by(e)
                })?;
            }
            let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
            {
                let mut f = std::fs::File::create(&tmp).map_err(|e| {
                    JxError::new(Kind::Io, "cannot write to the cache")
                        .at(tmp.display().to_string())
                        .caused_by(e)
                })?;
                f.write_all(content).map_err(|e| {
                    JxError::new(Kind::Io, "cannot write to the cache")
                        .at(tmp.display().to_string())
                        .caused_by(e)
                })?;
                f.sync_all().ok();
            }
            if !path.exists() {
                std::fs::rename(&tmp, &path).map_err(|e| {
                    JxError::new(Kind::Io, "cannot finalise a cache entry")
                        .at(path.display().to_string())
                        .caused_by(e)
                })?;
            } else {
                std::fs::remove_file(&tmp).ok();
            }
        }
        Ok(hex)
    }

    /// Read bytes for a id, re-verifying the checksum on read.
    pub fn get(&self, sha256_hex: &str) -> Result<Vec<u8>, JxError> {
        if sha256_hex.len() != 64 || !sha256_hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(
                JxError::new(Kind::Lock, "malformed sha256 key in cache lookup")
                    .hint("retry `jx fetch`; if this persists, report it"),
            );
        }
        let path = content_path(&self.root, sha256_hex);
        let bytes =
            std::fs::read(&path).map_err(|e| JxError::new(Kind::Io, "cache miss").caused_by(e))?;
        let actual = Self::sha256_hex(&bytes);
        if actual != sha256_hex {
            return Err(
                JxError::new(Kind::Lock, "checksum mismatch reading the cache")
                    .at(path.display().to_string())
                    .hint("delete the corrupted cache file named above and run `jx fetch` again"),
            );
        }
        Ok(bytes)
    }

    /// Whether a id is present (used by the lock fast path).
    pub fn has(&self, sha256_hex: &str) -> bool {
        sha256_hex.len() == 64
            && sha256_hex.bytes().all(|b| b.is_ascii_hexdigit())
            && content_path(&self.root, sha256_hex).exists()
    }

    /// The content path for a key, or a cache-miss error. Callers that put
    /// this on a classpath must verify the entry first (`jx fetch`/the build
    /// fast path do); the JVM cannot be handed a checksum.
    pub fn path_of(&self, sha256_hex: &str) -> Result<PathBuf, JxError> {
        if sha256_hex.len() != 64 || !sha256_hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(
                JxError::new(Kind::Lock, "malformed sha256 key in cache lookup")
                    .hint("run `jx fetch`; if this persists, report it"),
            );
        }
        let path = content_path(&self.root, sha256_hex);
        if !path.is_file() {
            return Err(JxError::new(Kind::Io, "cache miss")
                .at(path.display().to_string())
                .hint("run `jx fetch`"));
        }
        Ok(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn put_get_roundtrip_and_verification() {
        let dir = tempdir().unwrap();
        let cache = Cache::new(dir.path());
        let hex = cache.put(b"hello").unwrap();
        assert_eq!(hex, hex::encode(Sha256::digest(b"hello")));
        assert_eq!(cache.get(&hex).unwrap(), b"hello");
        assert!(cache.has(&hex));
        // Second put is idempotent and returns the same key.
        assert_eq!(cache.put(b"hello").unwrap(), hex);
    }

    #[test]
    fn corrupted_entry_is_a_hard_error() {
        let dir = tempdir().unwrap();
        let cache = Cache::new(dir.path());
        let hex = cache.put(b"hello").unwrap();
        let path = content_path(&dir.path().join("cache"), &hex);
        std::fs::write(&path, b"tampered").unwrap();
        let err = cache.get(&hex).unwrap_err();
        assert!(err.message.contains("checksum mismatch"));
    }

    #[test]
    fn malformed_key_rejected() {
        let dir = tempdir().unwrap();
        let cache = Cache::new(dir.path());
        assert!(cache.put(b"hello").is_ok());
        assert!(cache.get("not-a-key").is_err());
    }

    #[test]
    fn cache_miss_is_reported() {
        let dir = tempdir().unwrap();
        let cache = Cache::new(dir.path());
        let hex = hex::encode(Sha256::digest(b"absent"));
        assert!(cache.get(&hex).is_err());
        assert!(!cache.has(&hex));
        assert!(cache.path_of(&hex).is_err());
    }

    #[test]
    fn path_of_returns_an_existing_content_file() {
        let dir = tempdir().unwrap();
        let cache = Cache::new(dir.path());
        let hex = cache.put(b"hello").unwrap();
        let path = cache.path_of(&hex).unwrap();
        assert!(path.is_file());
        assert_eq!(std::fs::read(path).unwrap(), b"hello");
        assert!(cache.path_of("nope").is_err());
    }
}
