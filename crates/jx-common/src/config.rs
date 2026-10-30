//! Settings loaded from `<JX_HOME>/config.toml`.
//!
//! Shape recorded in `docs/decisions/0003-repository-client-and-pom-model.md`:
//!
//! ```toml
//! [repositories.internal]
//! url = "https://repo.example.com/maven"
//! auth = "env:JX_INTERNAL_TOKEN"          # never an inline secret
//!
//! [mirrors.internal-mirror]
//! of = "external:*"                       # Maven's `mirrorOf` grammar
//! url = "https://mirror.example.com/maven"
//!
//! [proxy]
//! http = "http://proxy.corp:3128"
//! https = "http://proxy.corp:3128"
//! no_proxy = "localhost, *.corp.example.com"
//! ```
//!
//! Checksums are deliberately **not** configurable: jx always uses the
//! Maven 4 default `fail` (ADR 0003). Unknown keys are rejected with the
//! same did-you-mean style project.toml uses.

use std::fmt::Write as _;
use std::path::Path;

use toml::Table;

use crate::diagnose::{closest, unknown_key_error};
use crate::error::{JxError, Kind};

const CONFIG_TABLES: [&str; 3] = ["mirrors", "proxy", "repositories"];
const CONFIG_KIND_TABLE: &str = "config";

/// The keys allowed inside a `[repositories.<id>]` entry.
const REPOSITORY_KEYS: [&str; 2] = ["auth", "url"];
/// The keys allowed inside a `[mirrors.<id>]` entry.
const MIRROR_KEYS: [&str; 3] = ["auth", "of", "url"];
/// The keys allowed inside the `[proxy]` table.
const PROXY_KEYS: [&str; 3] = ["http", "https", "no_proxy"];

#[derive(Debug, Default, Clone)]
pub struct Settings {
    /// Declared repositories, in the order config.toml lists them
    /// (`central`-replacement order matters for resolution).
    pub repositories: Vec<Repository>,
    /// Declared mirrors; matching order is declaration order (ADR 0003).
    pub mirrors: Vec<Mirror>,
    /// The explicit proxy, overriding `HTTPS_PROXY`-style environment when set.
    pub proxy: Option<Proxy>,
}

/// A declared Maven repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repository {
    /// The id other configuration refers to (`mirrorOf`, diagnostics).
    pub id: String,
    pub url: String,
    /// `env:` reference to the `user:password` variable, or None.
    pub auth: Option<String>,
}

/// A declared mirror of one or more repositories (settings.xml semantics).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mirror {
    pub id: String,
    /// The `mirrorOf` pattern, verbatim as written (`central`, `external:*`,
    /// `*`, `!id` exclusions). Matching happens in the repository client.
    pub of: String,
    pub url: String,
    /// Credential reference; the **mirror's** own id selects these.
    pub auth: Option<String>,
}

/// An explicit proxy. `no_proxy` keeps the standard env grammar
/// (comma-separated, `*` wildcard/suffix patterns) verbatim.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Proxy {
    pub http: Option<String>,
    pub https: Option<String>,
    pub no_proxy: Option<String>,
}

/// A validated credential reference. The actual secret is only ever read
/// from the named environment variable at fetch time, never stored here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthRef {
    /// `env:VAR`: the variable holds `user:password`, sent as Basic auth.
    Basic { env_var: String },
    /// `bearer:env:VAR`: the variable holds a raw token, sent as Bearer.
    Bearer { env_var: String },
}

impl AuthRef {
    /// Parse and validate a credential reference from config.
    pub fn parse(text: &str) -> Result<AuthRef, JxError> {
        let (bearer, rest) = match text.strip_prefix("bearer:") {
            Some(rest) => (true, rest),
            None => (false, text),
        };
        let Some(env_var) = rest.strip_prefix("env:") else {
            return Err(JxError::new(
                Kind::Io,
                "auth must be an `env:VAR` reference, not a credential",
            )
            .hint(
                "the variable value is `user:password` for Basic auth (or a raw token after \
                 `bearer:env:VAR`); inline secrets are never allowed",
            ));
        };
        if env_var.is_empty()
            || !env_var
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            return Err(JxError::new(
                Kind::Io,
                format!("auth variable name {env_var:?} is not a valid env-var name"),
            )
            .hint("use letters, digits and underscores, e.g. `env:JX_INTERNAL_TOKEN`"));
        }
        Ok(if bearer {
            AuthRef::Bearer {
                env_var: env_var.to_string(),
            }
        } else {
            AuthRef::Basic {
                env_var: env_var.to_string(),
            }
        })
    }

    /// The environment variable to read at fetch time.
    pub fn env_var(&self) -> &str {
        match self {
            AuthRef::Basic { env_var } | AuthRef::Bearer { env_var } => env_var,
        }
    }
}

/// Load settings from `<JX_HOME>/config.toml`. A missing file is empty
/// Settings; anything unreadable, unparsable or key-invalid is diagnosed.
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
    let value: Table = toml::from_str(&raw).map_err(|e| {
        JxError::new(Kind::Io, "cannot parse config.toml")
            .at(path.display().to_string())
            .caused_by(e)
            .hint("fix the TOML syntax; the error quote names the offending construct")
    })?;

    let mut repositories_raw = None;
    let mut mirrors_raw = None;
    let mut proxy_raw = None;
    for (key, value) in &value {
        match key.as_str() {
            "repositories" => repositories_raw = Some(require_table(key, value, "config")?),
            "mirrors" => mirrors_raw = Some(require_table(key, value, "config")?),
            "proxy" => proxy_raw = Some(value),
            other => {
                return Err(unknown_key_error(
                    Kind::Io,
                    CONFIG_KIND_TABLE,
                    other,
                    &CONFIG_TABLES,
                ));
            }
        }
    }

    let repositories = repositories_raw
        .map(parse_repositories)
        .transpose()?
        .unwrap_or_default();
    let mirrors = mirrors_raw
        .map(parse_mirrors)
        .transpose()?
        .unwrap_or_default();
    let proxy_raw = proxy_raw
        .map(|value| require_table("proxy", value, "config"))
        .transpose()?;
    let proxy = proxy_raw.map(parse_proxy).transpose()?.flatten();

    Ok(Settings {
        repositories,
        mirrors,
        proxy,
    })
}

fn require_table<'a>(key: &str, value: &'a toml::Value, what: &str) -> Result<&'a Table, JxError> {
    value.as_table().ok_or_else(|| {
        JxError::new(Kind::Io, format!("`{key}` in config.toml must be a table"))
            .hint(format!("`{what}` holds tables; e.g. `[{key}.id-name]`"))
    })
}

/// Unknown-key diagnosis inside a config entry, with its allowed keys.
fn entry_key_error(entry: &str, key: &str, candidates: &[&str]) -> JxError {
    let mut message = format!("unknown key `{key}` in `{entry}`");
    if let Some(c) = closest(key, candidates) {
        let _ = write!(message, "\n       did you mean `{c}`?");
    }
    JxError::new(Kind::Io, message)
        .hint(format!("allowed `{entry}` keys: {}", candidates.join(", ")))
}

fn parse_repositories(raw: &Table) -> Result<Vec<Repository>, JxError> {
    let mut repositories = Vec::new();
    for (id, value) in raw {
        let entry = format!("repositories.{id}");
        let table = value.as_table().ok_or_else(|| {
            JxError::new(
                Kind::Io,
                format!("`{entry}` must be a table with a `url`"),
            )
            .hint("e.g. `internal = {{ url = \"https://repo.example.com/maven\", auth = \"env:TOKEN\" }}`")
        })?;
        let mut url: Option<String> = None;
        let mut auth: Option<String> = None;
        for (key, value) in table {
            match key.as_str() {
                "url" => url = Some(string_field(&entry, key, value, true)?),
                "auth" => auth = Some(string_field(&entry, key, value, true)?),
                other => return Err(entry_key_error(&entry, other, &REPOSITORY_KEYS)),
            }
        }
        let url = url.ok_or_else(|| JxError::new(Kind::Io, format!("`{entry}` needs a `url`")))?;
        check_http_url(&entry, &url)?;
        if let Some(auth) = &auth {
            AuthRef::parse(auth).map_err(|e| e.at(format!("{entry}: auth")))?;
        }
        check_id(&entry, id)?;
        repositories.push(Repository {
            id: id.to_string(),
            url,
            auth,
        });
    }
    Ok(repositories)
}

fn parse_mirrors(raw: &Table) -> Result<Vec<Mirror>, JxError> {
    let mut mirrors = Vec::new();
    for (id, value) in raw {
        let entry = format!("mirrors.{id}");
        let table = value.as_table().ok_or_else(|| {
            JxError::new(Kind::Io, format!("`{entry}` must be a table"))
                .hint("e.g. `internal-mirror = {{ of = \"internal\", url = \"https://mirror.example.com/maven\" }}`")
        })?;
        let mut url: Option<String> = None;
        let mut auth: Option<String> = None;
        let mut of: Option<String> = None;
        for (key, value) in table {
            match key.as_str() {
                "url" => url = Some(string_field(&entry, key, value, true)?),
                "of" => of = Some(string_field(&entry, key, value, true)?),
                "auth" => auth = Some(string_field(&entry, key, value, true)?),
                other => return Err(entry_key_error(&entry, other, &MIRROR_KEYS)),
            }
        }
        let url = url.ok_or_else(|| JxError::new(Kind::Io, format!("`{entry}` needs a `url`")))?;
        let of = of.ok_or_else(|| {
            JxError::new(
                Kind::Io,
                format!("`{entry}` needs `of` (which repository ids it mirrors, Maven's `mirrorOf` grammar)"),
            )
            .hint("examples: `central`, `*`, `external:*`, `internal,third-party`")
        })?;
        check_http_url(&entry, &url)?;
        if !of.is_empty() {
            // Patterns are strings Maven never trims; only reject whitespace-only.
        } else {
            return Err(
                JxError::new(Kind::Io, format!("`{entry}` has an empty `of` pattern")).hint(
                    "`of` selects the mirrored repositories: `central`, `*`, `external:*`, ids",
                ),
            );
        }
        if let Some(auth) = &auth {
            AuthRef::parse(auth).map_err(|e| e.at(format!("{entry}: auth")))?;
        }
        check_id(&entry, id)?;
        mirrors.push(Mirror {
            id: id.to_string(),
            of,
            url,
            auth,
        });
    }
    Ok(mirrors)
}

fn parse_proxy(table: &Table) -> Result<Option<Proxy>, JxError> {
    let entry = "proxy";
    let mut proxy = Proxy::default();
    let mut any = false;
    for (key, value) in table {
        any = true;
        match key.as_str() {
            "http" => proxy.http = Some(string_field(entry, key, value, false)?),
            "https" => proxy.https = Some(string_field(entry, key, value, false)?),
            "no_proxy" => proxy.no_proxy = Some(string_field(entry, key, value, false)?),
            other => return Err(entry_key_error(entry, other, &PROXY_KEYS)),
        }
    }
    Ok(if any { Some(proxy) } else { None })
}

fn string_field(
    entry: &str,
    key: &str,
    value: &toml::Value,
    required_shape: bool,
) -> Result<String, JxError> {
    let text = value.as_str().ok_or_else(|| {
        JxError::new(Kind::Io, format!("`{key}` in `{entry}` must be a string"))
            .hint(format!("e.g. `{key} = \"...\"`"))
    })?;
    if required_shape && text.is_empty() {
        return Err(JxError::new(
            Kind::Io,
            format!("`{key}` in `{entry}` must not be empty"),
        ));
    }
    Ok(text.to_string())
}

fn check_http_url(entry: &str, url: &str) -> Result<(), JxError> {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(JxError::new(
            Kind::Io,
            format!("`{entry}` url must start with `http://` or `https://`: {url:?}"),
        ));
    }
    Ok(())
}

fn check_id(entry: &str, id: &str) -> Result<(), JxError> {
    if id.is_empty()
        || id.chars().any(|c| c.is_whitespace())
        || id.chars().any(|c| c == '!' || c == '*' || c == ',')
    {
        return Err(JxError::new(
            Kind::Io,
            format!("`{entry}` is not a usable repository/mirror id"),
        )
        .hint(
            "ids are simple names (`central`, `internal`); `!`, `*` and `,` belong to `of` patterns, not ids",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn write_config(home: &Path, text: &str) {
        std::fs::write(home.join("config.toml"), text).unwrap();
    }

    #[test]
    fn missing_config_is_default() {
        let dir = tempdir().unwrap();
        let s = load(dir.path()).unwrap();
        assert!(s.repositories.is_empty());
        assert!(s.mirrors.is_empty());
        assert!(s.proxy.is_none());
    }

    #[test]
    fn parses_repositories() {
        let dir = tempdir().unwrap();
        write_config(
            dir.path(),
            "[repositories]\ninternal = { url = \"https://repo.example.com/maven\", auth = \"env:JX_INTERNAL_TOKEN\" }\n",
        );
        let s = load(dir.path()).unwrap();
        assert_eq!(s.repositories.len(), 1);
        assert_eq!(s.repositories[0].id, "internal");
        assert_eq!(
            s.repositories[0].auth.as_deref(),
            Some("env:JX_INTERNAL_TOKEN")
        );
    }

    #[test]
    fn parses_mirrors_proxy_and_no_proxy() {
        let dir = tempdir().unwrap();
        write_config(
            dir.path(),
            r#"
[mirrors.corp]
of = "external:*"
url = "https://mirror.example.com/maven"
auth = "bearer:env:MIRROR_TOKEN"

[proxy]
http = "http://proxy.corp:3128"
https = "http://proxy.corp:3128"
no_proxy = "localhost, *.corp.example.com"
"#,
        );
        let s = load(dir.path()).unwrap();
        assert_eq!(s.mirrors.len(), 1);
        assert_eq!(s.mirrors[0].of, "external:*");
        assert_eq!(
            s.mirrors[0].auth.as_deref(),
            Some("bearer:env:MIRROR_TOKEN")
        );
        let proxy = s.proxy.expect("proxy set");
        assert_eq!(proxy.https.as_deref(), Some("http://proxy.corp:3128"));
        assert_eq!(
            proxy.no_proxy.as_deref(),
            Some("localhost, *.corp.example.com")
        );
    }

    #[test]
    fn unknown_table_gets_did_you_mean() {
        let dir = tempdir().unwrap();
        write_config(dir.path(), "[repository]\n");
        let e = load(dir.path()).unwrap_err();
        assert!(e.message.contains("unknown key `repository`"), "{e}");
        assert!(e.message.contains("did you mean `repositories`?"), "{e}");
    }

    #[test]
    fn unknown_repository_key_gets_did_you_mean() {
        let dir = tempdir().unwrap();
        write_config(
            dir.path(),
            "[repositories]\ninternal = { url = \"https://repo.example.com/maven\", token = \"x\" }\n",
        );
        let e = load(dir.path()).unwrap_err();
        assert!(e.message.contains("unknown key `token`"), "{e}");
        assert!(
            e.message.contains("did you mean `auth`?") // closest to `token`
                || e.message.contains("did you mean `url`?"),
            "{e}"
        );
    }

    #[test]
    fn repository_without_url_is_rejected() {
        let dir = tempdir().unwrap();
        write_config(
            dir.path(),
            "[repositories]\ninternal = { auth = \"env:T\" }\n",
        );
        let e = load(dir.path()).unwrap_err();
        assert!(e.message.contains("needs a `url`"), "{e}");
    }

    #[test]
    fn inline_secret_is_rejected() {
        let dir = tempdir().unwrap();
        write_config(
            dir.path(),
            "[repositories]\ninternal = { url = \"https://repo.example.com/maven\", auth = \"user:secret\" }\n",
        );
        let e = load(dir.path()).unwrap_err();
        assert!(e.message.contains("env:VAR"), "{e}");
    }

    #[test]
    fn mirror_needs_of_pattern() {
        let dir = tempdir().unwrap();
        write_config(
            dir.path(),
            "[mirrors]\ncorp = { url = \"https://mirror.example.com/maven\" }\n",
        );
        let e = load(dir.path()).unwrap_err();
        assert!(e.message.contains("needs `of`"), "{e}");
    }

    #[test]
    fn auth_refs_parse_both_forms_and_reject_bad_names() {
        assert_eq!(
            AuthRef::parse("env:JX_TOKEN").unwrap(),
            AuthRef::Basic {
                env_var: "JX_TOKEN".to_string()
            }
        );
        assert_eq!(
            AuthRef::parse("bearer:env:JX_TOKEN").unwrap(),
            AuthRef::Bearer {
                env_var: "JX_TOKEN".to_string()
            }
        );
        assert!(AuthRef::parse("env:has-dash").is_err());
        assert!(AuthRef::parse("env:").is_err());
    }
}
