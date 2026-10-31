//! Comment- and format-preserving edits of the dependency tables.
//!
//! Like [`crate::set_jdk`], these functions edit the file as text, never by
//! reserializing the TOML. An added entry is appended at the end of its
//! table's content and a removed entry is deleted line by line; every other
//! byte (comments, indentation, line endings) survives.

use std::path::Path;

use jx_common::error::{JxError, Kind};

use crate::deps::{DependencySpec, WantedVersion};

/// Which dependency table an edit targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DepTable {
    Dependencies,
    DevDependencies,
}

impl DepTable {
    pub fn name(self) -> &'static str {
        match self {
            DepTable::Dependencies => "dependencies",
            DepTable::DevDependencies => "dev-dependencies",
        }
    }
}

/// Append one validated dependency to its table, creating the table when
/// absent. Returns whether the file changed.
pub fn add_dependency(
    path: &Path,
    table: DepTable,
    spec: &DependencySpec,
) -> Result<bool, JxError> {
    let raw = read(path)?;
    if has_alias(path, table, &spec.alias)? {
        return Err(JxError::new(
            Kind::Manifest,
            format!("`{}` is already declared in [{}]", spec.alias, table.name()),
        )
        .at(path.display().to_string())
        .hint("pass `--alias NAME` to add the same artifact under a different local name"));
    }
    let entry = render_entry(spec)?;
    let eol = line_ending(&raw);
    let lines: Vec<&str> = raw.split_inclusive('\n').collect();
    let header = find_table(&lines, table.name());

    let mut out = String::with_capacity(raw.len() + entry.len() + table.name().len() + 8);
    match header {
        Some(header) => {
            let end = table_end(&lines, header);
            // Place the entry after the table's last non-blank line, so
            // trailing comments stay attached to whatever follows them.
            let mut insert_at = header + 1;
            for i in (header + 1..end).rev() {
                if !lines[i].trim().is_empty() {
                    insert_at = i + 1;
                    break;
                }
            }
            for (index, line) in lines.iter().enumerate() {
                if index == insert_at {
                    if !out.ends_with('\n') {
                        out.push_str(eol);
                    }
                    out.push_str(&entry);
                    out.push_str(eol);
                }
                out.push_str(line);
            }
            if insert_at == lines.len() {
                if !out.ends_with('\n') {
                    out.push_str(eol);
                }
                out.push_str(&entry);
                out.push_str(eol);
            }
        }
        None => {
            out.push_str(&raw);
            if !out.is_empty() && !out.ends_with('\n') {
                out.push_str(eol);
            }
            if !out.is_empty() {
                out.push_str(eol);
            }
            out.push('[');
            out.push_str(table.name());
            out.push(']');
            out.push_str(eol);
            out.push_str(&entry);
            out.push_str(eol);
        }
    }

    write_checked(path, &raw, &out)
}

/// Remove the entry with `alias` from its table. Returns whether the file
/// changed; an absent alias is an error naming what is there.
pub fn remove_dependency(path: &Path, table: DepTable, alias: &str) -> Result<bool, JxError> {
    let raw = read(path)?;
    let lines: Vec<&str> = raw.split_inclusive('\n').collect();
    let Some(header) = find_table(&lines, table.name()) else {
        return Err(no_table(path, table));
    };
    let end = table_end(&lines, header);
    let mut available = Vec::new();
    let mut remove_at = None;
    for (index, line) in lines.iter().enumerate().take(end).skip(header + 1) {
        let Some(key) = entry_key(line) else {
            continue;
        };
        available.push(key.clone());
        if key == alias {
            remove_at = Some(index);
            break;
        }
    }
    let Some(remove_at) = remove_at else {
        available.sort();
        return Err(JxError::new(
            Kind::Manifest,
            format!("no `{alias}` in [{}]", table.name()),
        )
        .at(path.display().to_string())
        .hint(if available.is_empty() {
            format!("[{}] is empty", table.name())
        } else {
            format!("entries there: {}", available.join(", "))
        }));
    };
    let mut out = String::with_capacity(raw.len());
    for (index, line) in lines.iter().enumerate() {
        if index != remove_at {
            out.push_str(line);
        }
    }
    write_checked(path, &raw, &out)
}

/// Whether `alias` already names an entry in the table.
pub fn has_alias(path: &Path, table: DepTable, alias: &str) -> Result<bool, JxError> {
    let raw = read(path)?;
    let lines: Vec<&str> = raw.split_inclusive('\n').collect();
    let Some(header) = find_table(&lines, table.name()) else {
        return Ok(false);
    };
    let end = table_end(&lines, header);
    Ok(lines[header + 1..end]
        .iter()
        .filter_map(|line| entry_key(line))
        .any(|key| key == alias))
}

fn read(path: &Path) -> Result<String, JxError> {
    std::fs::read_to_string(path).map_err(|e| {
        JxError::new(Kind::Manifest, "cannot read project.toml")
            .at(path.display().to_string())
            .caused_by(e)
    })
}

fn write_checked(path: &Path, original: &str, edited: &str) -> Result<bool, JxError> {
    if original == edited {
        return Ok(false);
    }
    std::fs::write(path, edited).map_err(|e| {
        JxError::new(Kind::Manifest, "cannot write project.toml")
            .at(path.display().to_string())
            .caused_by(e)
    })?;
    // A structural edit must leave a manifest jx can still parse; roll back
    // rather than leaving the user with a broken file.
    if let Err(error) = crate::parse(path) {
        std::fs::write(path, original).ok();
        return Err(error.hint("the dependency edit was rolled back; project.toml is unchanged"));
    }
    Ok(true)
}

fn render_entry(spec: &DependencySpec) -> Result<String, JxError> {
    let alias = render_alias(&spec.alias)?;
    // Platform entries always pin their BOM and never carry a classifier;
    // ordinary entries keep the exact-pin `=` marker they were written with.
    if spec.platform {
        let wanted = spec.wanted.as_ref().ok_or_else(|| {
            JxError::new(
                Kind::Usage,
                format!("the platform `{}` has no version", spec.alias),
            )
        })?;
        return Ok(format!(
            "{alias} = {{ platform = \"{}:{}:{}\" }}\n",
            spec.group_id,
            spec.artifact_id,
            wanted.as_str()
        ));
    }
    let version = match &spec.wanted {
        Some(WantedVersion::Prefix(v)) => format!(":{v}"),
        Some(WantedVersion::Exact(v)) => format!(":={v}"),
        None => String::new(),
    };
    Ok(format!(
        "{alias} = \"{}:{}{version}{}\"\n",
        spec.group_id,
        spec.artifact_id,
        classifier_suffix(spec)
    ))
}

fn classifier_suffix(spec: &DependencySpec) -> String {
    spec.classifier
        .as_deref()
        .map(|c| format!(":{c}"))
        .unwrap_or_default()
}

/// Bare TOML keys are the readable case; anything else is quoted and
/// escaped so a user-chosen alias can never break the document.
fn render_alias(alias: &str) -> Result<String, JxError> {
    if alias.is_empty() {
        return Err(JxError::new(
            Kind::Usage,
            "a dependency alias must not be empty",
        ));
    }
    if alias
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Ok(alias.to_string());
    }
    let mut out = String::from("\"");
    for c in alias.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' | '\r' => {
                return Err(JxError::new(
                    Kind::Usage,
                    "a dependency alias cannot contain a line break",
                ));
            }
            _ => out.push(c),
        }
    }
    out.push('"');
    Ok(out)
}

/// Index of the `[name]` header, if present.
fn find_table(lines: &[&str], name: &str) -> Option<usize> {
    lines
        .iter()
        .position(|line| header_name(line).is_some_and(|found| found == name))
}

/// The first line at or after `header` that starts a new table.
fn table_end(lines: &[&str], header: usize) -> usize {
    lines
        .iter()
        .enumerate()
        .skip(header + 1)
        .find_map(|(index, line)| header_name(line).map(|_| index))
        .unwrap_or(lines.len())
}

fn header_name(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    if !trimmed.starts_with('[') || trimmed.starts_with("[[") {
        return None;
    }
    let end = trimmed.find(']')?;
    let rest = trimmed[end + 1..].trim_start();
    if !rest.is_empty() && !rest.starts_with('#') {
        return None;
    }
    Some(trimmed[1..end].trim())
}

/// The key at the start of a `key = value` line, or `None` for comments,
/// blanks and other constructs.
fn entry_key(line: &str) -> Option<String> {
    let trimmed = line.trim_start();
    if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with('[') {
        return None;
    }
    let equals = trimmed.find('=')?;
    let key = trimmed[..equals].trim();
    if key.is_empty() {
        return None;
    }
    if let Some(inner) = key.strip_prefix('"').and_then(|k| k.strip_suffix('"')) {
        // Only the escapes the writer emits are meaningful here; a malformed
        // key is simply not a match rather than a crash.
        let mut unescaped = String::with_capacity(inner.len());
        let mut chars = inner.chars();
        while let Some(c) = chars.next() {
            if c != '\\' {
                unescaped.push(c);
                continue;
            }
            match chars.next() {
                Some('n') => unescaped.push('\n'),
                Some('r') => unescaped.push('\r'),
                Some('t') => unescaped.push('\t'),
                Some('"') => unescaped.push('"'),
                Some('\\') => unescaped.push('\\'),
                Some(other) => {
                    unescaped.push('\\');
                    unescaped.push(other);
                }
                None => break,
            }
        }
        return Some(unescaped);
    }
    Some(key.to_string())
}

fn line_ending(raw: &str) -> &'static str {
    if raw.contains("\r\n") { "\r\n" } else { "\n" }
}

fn no_table(path: &Path, table: DepTable) -> JxError {
    JxError::new(
        Kind::Manifest,
        format!("project.toml has no [{}] table", table.name()),
    )
    .at(path.display().to_string())
    .hint(format!(
        "`jx add --dev` or add a [{}] table first",
        table.name()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deps::parse_user_spec;
    use tempfile::tempdir;

    fn manifest_at(dir: &Path, content: &str) -> std::path::PathBuf {
        let path = dir.join("project.toml");
        std::fs::write(&path, content).unwrap();
        path
    }

    const BASE: &str = "[project]\nname = \"h\"\nversion = \"0.1.0\"\njdk = \"25\"\n";

    #[test]
    fn add_appends_to_an_existing_table_and_preserves_comments() {
        let dir = tempdir().unwrap();
        let p = manifest_at(
            dir.path(),
            &format!(
                "{BASE}\n[dependencies]\n# keep me\nguava = \"com.google.guava:guava:33\"\n\n[run]\nmain = \"a.App\"\n"
            ),
        );
        let spec = parse_user_spec("slf4j", "org.slf4j:slf4j-api:2.0.16", false).unwrap();
        assert!(add_dependency(&p, DepTable::Dependencies, &spec).unwrap());
        let after = std::fs::read_to_string(&p).unwrap();
        assert!(after.contains("# keep me"));
        assert!(after.contains("guava = \"com.google.guava:guava:33\""));
        assert!(after.contains("slf4j = \"org.slf4j:slf4j-api:2.0.16\""));
        assert!(after.contains("[run]\nmain = \"a.App\""));
        let again = add_dependency(&p, DepTable::Dependencies, &spec).unwrap_err();
        assert!(again.message.contains("already declared"), "{again}");
    }

    #[test]
    fn add_creates_a_missing_table() {
        let dir = tempdir().unwrap();
        let p = manifest_at(dir.path(), BASE);
        let spec = parse_user_spec("junit", "org.junit.jupiter:junit-jupiter:5", false).unwrap();
        add_dependency(&p, DepTable::DevDependencies, &spec).unwrap();
        let after = std::fs::read_to_string(&p).unwrap();
        assert!(after.contains("[dev-dependencies]"));
        assert!(after.contains("junit = \"org.junit.jupiter:junit-jupiter:5\""));
        crate::parse(&p).expect("still valid");
    }

    #[test]
    fn add_handles_platform_and_exact_entries() {
        let dir = tempdir().unwrap();
        let p = manifest_at(dir.path(), BASE);
        let bom = parse_user_spec(
            "jackson-bom",
            "com.fasterxml.jackson:jackson-bom:2.18.0",
            true,
        )
        .unwrap();
        let exact = parse_user_spec("slf4j", "org.slf4j:slf4j-api:=2.0.16", false).unwrap();
        add_dependency(&p, DepTable::Dependencies, &bom).unwrap();
        add_dependency(&p, DepTable::Dependencies, &exact).unwrap();
        let after = std::fs::read_to_string(&p).unwrap();
        assert!(
            after.contains(
                "jackson-bom = { platform = \"com.fasterxml.jackson:jackson-bom:2.18.0\" }"
            )
        );
        assert!(after.contains("slf4j = \"org.slf4j:slf4j-api:=2.0.16\""));
        crate::parse(&p).expect("still valid");
    }

    #[test]
    fn remove_deletes_only_the_named_line() {
        let dir = tempdir().unwrap();
        let p = manifest_at(
            dir.path(),
            &format!(
                "{BASE}\n[dependencies]\n# comment\nguava = \"com.google.guava:guava:33\"\nslf4j = \"org.slf4j:slf4j-api:2.0.16\"\n"
            ),
        );
        assert!(remove_dependency(&p, DepTable::Dependencies, "guava").unwrap());
        let after = std::fs::read_to_string(&p).unwrap();
        assert!(after.contains("# comment"));
        assert!(!after.contains("guava ="));
        assert!(after.contains("slf4j ="));
        crate::parse(&p).expect("still valid");
    }

    #[test]
    fn remove_names_the_available_aliases() {
        let dir = tempdir().unwrap();
        let p = manifest_at(
            dir.path(),
            &format!("{BASE}\n[dependencies]\nguava = \"com.google.guava:guava:33\"\n"),
        );
        let error = remove_dependency(&p, DepTable::Dependencies, "nope").unwrap_err();
        assert!(error.message.contains("no `nope`"), "{error}");
        assert!(error.hint.as_deref().unwrap().contains("guava"), "{error}");
    }

    #[test]
    fn remove_without_the_table_is_clear() {
        let dir = tempdir().unwrap();
        let p = manifest_at(dir.path(), BASE);
        let error = remove_dependency(&p, DepTable::DevDependencies, "junit").unwrap_err();
        assert!(error.message.contains("no [dev-dependencies]"), "{error}");
    }

    #[test]
    fn aliases_that_need_quoting_round_trip() {
        let dir = tempdir().unwrap();
        let p = manifest_at(dir.path(), BASE);
        let spec = parse_user_spec("my lib", "g:a:1", false).unwrap();
        add_dependency(&p, DepTable::Dependencies, &spec).unwrap();
        assert!(has_alias(&p, DepTable::Dependencies, "my lib").unwrap());
        assert!(remove_dependency(&p, DepTable::Dependencies, "my lib").unwrap());
        assert!(!std::fs::read_to_string(&p).unwrap().contains("my lib"));
    }

    #[test]
    fn crlf_files_stay_crlf() {
        let dir = tempdir().unwrap();
        let p = manifest_at(
            dir.path(),
            "[project]\r\nname = \"h\"\r\nversion = \"0.1.0\"\r\njdk = \"25\"\r\n",
        );
        let spec = parse_user_spec("guava", "com.google.guava:guava:33", false).unwrap();
        add_dependency(&p, DepTable::Dependencies, &spec).unwrap();
        let after = std::fs::read_to_string(&p).unwrap();
        assert!(after.contains("[dependencies]\r\nguava ="));
        assert!(!after.contains("\n[dependencies]\n"));
    }
}
