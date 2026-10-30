//! Resolution of jx's on-disk state location.
//!
//! Layout under `JX_HOME` (default `~/.jx`):
//!
//! ```text
//! $JX_HOME/
//!   config.toml                 # mirrors, repositories and settings
//!   jdks/<vendor>-<version>/    # installed JDKs
//!   cache/
//!     artifacts/content/<aa>/<sha256hex>   # content-addressed artifact cache
//! ```
//!
//! `JX_HOME` is respected everywhere; nothing here touches the user's system
//! Java installation.

use std::path::PathBuf;

use crate::error::{JxError, Kind};

/// The effective jx home directory: `$JX_HOME`, else `~/.jx`.
pub fn jx_home() -> Result<PathBuf, JxError> {
    if let Some(home) = std::env::var_os("JX_HOME")
        && !home.is_empty()
    {
        return Ok(PathBuf::from(home));
    }
    let base = user_home()?;
    Ok(base.join(".jx"))
}

/// The user's OS home directory, without extra dependencies.
fn user_home() -> Result<PathBuf, JxError> {
    let home = std::env::var_os("HOME")
        .filter(|v| !v.is_empty())
        .or_else(|| std::env::var_os("USERPROFILE").filter(|v| !v.is_empty()))
        .ok_or_else(|| {
            JxError::new(
                Kind::Io,
                "cannot determine the home directory (HOME and USERPROFILE are unset)",
            )
            .hint("set JX_HOME to the directory jx should use for its state")
        })?;
    Ok(PathBuf::from(home))
}
