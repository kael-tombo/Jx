//! Settings loaded from `<jx-home>/config.toml`.
//!
//! Mirrors and custom repositories live here (declared repositories may also
//! come from project.toml in a later milestone). Parsing stays permissive for
//! unknown keys for today; strict validation arrives with M2/M3 and will bring
//! the same did-you-mean diagnostics the manifest uses.

use std::path::Path;

use serde::Deserialize;

use crate::error::{JxError, Kind};

#[derive(Debug, Default, Clone)]
pub struct Settings {
    pub repositories: Vec<Repository>,
}

/// A Maven repository or mirror. `auth` is a reference (e.g. `env:TOKEN`),
/// never an inline secret; V1 never stores credentials in config.toml.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Repository {
    pub url: String,
    #[serde(default)]
    pub auth: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct ConfigFile {
    #[serde(default)]
    repositories: std::collections::BTreeMap<String, Repository>,
}

/// Load settings from `<jx-home>/config.toml`. A missing file is an empty
/// Settings; an unreadable or unparsable file is a diagnosed error.
pub fn load(home: &Path) -> Result<Settings, JxError> {
    let path = home.join("config.toml");
    if !path.exists() {
        return Ok(Settings::default());
    }
    let raw = std::fs::read_to_string(&path).map_err(|e| {
        JxError::new(Kind::Io, "cannot read config.toml")
            .at(path.display().to_string())
            .caused_by(e)
            .hint("check the file is readable, or remove it and let jx recreate defaults")
    })?;
    let file: ConfigFile = toml::from_str(&raw).map_err(|e| {
        JxError::new(Kind::Io, "cannot parse config.toml")
            .at(path.display().to_string())
            .caused_by(e)
            .hint("fix the TOML syntax; see the settings docs for the allowed keys")
    })?;
    let repositories = file
        .repositories
        .into_values()
        .map(|repo| Repository {
            url: repo.url,
            auth: repo.auth,
        })
        .collect();
    Ok(Settings { repositories })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn missing_config_is_default() {
        let dir = tempdir().unwrap();
        let s = load(dir.path()).unwrap();
        assert!(s.repositories.is_empty());
    }

    #[test]
    fn parses_repositories() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.toml"),
            "[repositories]\ninternal = { url = \"https://repo.example.com/maven\", auth = \"env:JX_INTERNAL_TOKEN\" }\n",
        )
        .unwrap();
        let s = load(dir.path()).unwrap();
        assert_eq!(s.repositories.len(), 1);
        assert_eq!(
            s.repositories[0].auth.as_deref(),
            Some("env:JX_INTERNAL_TOKEN")
        );
    }
}
