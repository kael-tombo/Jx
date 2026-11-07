//! Import a `jot.toml` project into jx's `project.toml`.
//!
//! jot owns a similar single-binary, single-manifest workflow. This is a
//! migration path, not a runtime dependency: jx reads the declarations jot
//! wrote, refuses the parts it cannot represent (workspaces, path
//! dependencies, Kotlin toolchains) and prints every approximation. The
//! result is validated by the CLI with the same parser `jx build` uses.

use std::collections::{BTreeMap, HashSet};

use jx_common::error::{JxError, Kind};

use crate::{
    ImportedDependency, ImportedManifest, RenderRequest, detect_framework, parse_java_version,
    render,
};

/// One `jot.toml` and its optional version catalog.
pub struct JotSource {
    /// Display path for diagnostics, usually `jot.toml`.
    pub path: String,
    pub text: String,
    /// `libs.versions.toml`, when the project has one.
    pub catalog: Option<(String, String)>,
    /// Used when `[project].version` is absent.
    pub default_version: String,
    /// Used when `[toolchains].java` is absent.
    pub default_feature: u32,
}

/// Convert a jot manifest into a jx manifest.
pub fn manifest_from_jot(source: &JotSource) -> Result<ImportedManifest, JxError> {
    let root: toml::Table = toml::from_str(&source.text).map_err(|e| {
        JxError::new(Kind::Resolve, "cannot parse jot.toml")
            .at(source.path.clone())
            .caused_by(e)
            .hint("fix the TOML syntax, then run `jx import` again")
    })?;

    if root.contains_key("workspace") {
        return Err(JxError::new(
            Kind::Resolve,
            "jot workspaces contain more than one project; jx has no workspace model",
        )
        .at(source.path.clone())
        .hint(
            "run `jx import` in one member directory at a time, or keep using jot for the workspace",
        ));
    }

    let project = table(&root, "project", &source.path)?.ok_or_else(|| {
        JxError::new(Kind::Resolve, "jot.toml has no [project] table")
            .at(source.path.clone())
            .hint("add [project] name and version, or import a Maven/Gradle build instead")
    })?;

    let name = string(project.get("name"), "[project] name", &source.path)?;
    if name.trim().is_empty() {
        return Err(JxError::new(Kind::Resolve, "[project] name is empty")
            .at(source.path.clone())
            .hint("give the jot project a name before importing"));
    }

    let mut notes = Vec::new();
    let version = match project.get("version") {
        Some(toml::Value::String(version)) if !version.trim().is_empty() => version.to_string(),
        Some(toml::Value::String(_)) => {
            return Err(JxError::new(Kind::Resolve, "[project] version is empty")
                .at(source.path.clone())
                .hint(format!(
                    "set a version, or remove the empty value to use {}",
                    source.default_version
                )));
        }
        Some(_) => {
            return Err(
                JxError::new(Kind::Resolve, "[project] version must be a string")
                    .at(source.path.clone()),
            );
        }
        None => {
            notes.push(format!(
                "jot.toml has no [project] version; using \"{}\"",
                source.default_version
            ));
            source.default_version.clone()
        }
    };

    let main_class = match project.get("main-class") {
        Some(toml::Value::String(value)) => {
            let value = value.trim();
            (!value.is_empty()).then(|| value.to_string())
        }
        Some(_) => {
            return Err(
                JxError::new(Kind::Resolve, "[project] main-class must be a string")
                    .at(source.path.clone()),
            );
        }
        None => None,
    };
    if main_class.is_some() {
        notes.push("the main class was imported into [run] main".to_string());
    }

    // jot's default is `flat`, not `maven`; an omitted `layout` must get the
    // same source-tree warning as an explicit `flat`.
    let flat_note = "jot's flat layout (the default when `layout` is omitted) uses src/, test/ and res/; jx builds src/main/java, src/test/java and src/main/resources. Move the sources before `jx build`.";
    match project.get("layout") {
        Some(toml::Value::String(layout)) if layout == "maven" => {}
        Some(toml::Value::String(layout)) if layout == "flat" => {
            notes.push(flat_note.to_string());
        }
        Some(toml::Value::String(other)) => {
            return Err(JxError::new(
                Kind::Resolve,
                format!("jot layout {other:?} is not a layout jx can import"),
            )
            .at(source.path.clone())
            .hint("jot supports `flat` and `maven`; fix the value before importing"));
        }
        Some(_) => {
            return Err(
                JxError::new(Kind::Resolve, "[project] layout must be a string")
                    .at(source.path.clone()),
            );
        }
        None => notes.push(flat_note.to_string()),
    }
    if let Some(group) = project.get("group") {
        match group {
            toml::Value::String(group) => notes.push(format!(
                "[project] group = {group:?} was not imported; jx has no publication metadata. `jx export maven --group-id {group}` writes the coordinate when publishing."
            )),
            _ => {
                return Err(JxError::new(
                    Kind::Resolve,
                    "[project] group must be a string",
                )
                .at(source.path.clone()));
            }
        }
    }
    for key in project.keys() {
        if !matches!(
            key.as_str(),
            "name" | "version" | "main-class" | "layout" | "group"
        ) {
            notes.push(format!("[project] {key} was not imported"));
        }
    }

    let (jdk, jdk_source) = read_toolchain(&root, source, &mut notes)?;

    let catalog = source
        .catalog
        .as_ref()
        .map(|(path, text)| Catalog::load(path, text, &source.path))
        .transpose()?;
    let mut catalog_uses = 0usize;

    let mut main = Vec::new();
    if let Some(table) = table(&root, "dependencies", &source.path)? {
        main = read_dependencies(
            table,
            "dependencies",
            &source.path,
            catalog.as_ref(),
            &mut catalog_uses,
        )?;
    }
    let main_ga: HashSet<(String, String)> = main
        .iter()
        .map(|dep| (dep.group_id.clone(), dep.artifact_id.clone()))
        .collect();

    let mut dev = Vec::new();
    if let Some(table) = table(&root, "test-dependencies", &source.path)? {
        for dep in read_dependencies(
            table,
            "test-dependencies",
            &source.path,
            catalog.as_ref(),
            &mut catalog_uses,
        )? {
            if main_ga.contains(&(dep.group_id.clone(), dep.artifact_id.clone())) {
                notes.push(format!(
                    "{}:{} is both a main and a test dependency; imported once as a main dependency",
                    dep.group_id, dep.artifact_id
                ));
            } else {
                dev.push(dep);
            }
        }
    }

    let mut processors = Vec::new();
    if let Some(table) = table(&root, "processors", &source.path)? {
        processors = read_dependencies(
            table,
            "processors",
            &source.path,
            catalog.as_ref(),
            &mut catalog_uses,
        )?;
    }

    if let Some(publish) = table(&root, "publish", &source.path)?
        && !publish.is_empty()
    {
        notes.push(
            "jot's [publish] metadata was not imported; `jx export maven` writes a POM when publishing is needed"
                .to_string(),
        );
    }

    for key in root.keys() {
        if !matches!(
            key.as_str(),
            "project"
                | "toolchains"
                | "dependencies"
                | "test-dependencies"
                | "processors"
                | "publish"
                | "workspace"
        ) {
            return Err(JxError::new(
                Kind::Resolve,
                format!("jot.toml table [{key}] is not part of the supported import"),
            )
            .at(source.path.clone())
            .hint("remove the table, or keep using jot for this project"));
        }
    }

    if catalog_uses > 0 {
        notes.push(format!(
            "{catalog_uses} dependency version(s) came from libs.versions.toml"
        ));
    }
    if !dev.is_empty() {
        notes.push(
            "test dependencies were imported, but `jx test` runs JUnit 5 through the JUnit Platform console standalone; if the project does not declare it, add it with `jx add --dev org.junit.platform:junit-platform-console-standalone`"
                .to_string(),
        );
    }
    notes.push(
        "direct dependencies and processors are exact-pinned with `=`; transitive versions still follow jx's nearest-wins resolver"
            .to_string(),
    );

    let framework = detect_framework(
        main.iter()
            .chain(&processors)
            .map(|dep| dep.group_id.as_str()),
    );
    if let Some((value, group)) = framework {
        notes.push(format!(
            "detected framework = \"{value}\" from the {group} group; remove [package] framework if this module is a plain jar"
        ));
    }
    let text = render(&RenderRequest {
        name: &name,
        version: &version,
        jdk,
        framework: framework.map(|(value, _)| value),
        main_class: main_class.as_deref(),
        main: &main,
        dev: &dev,
        processors: &processors,
        source: &source.path,
    });
    Ok(ImportedManifest {
        text,
        notes,
        dependencies: main.len(),
        dev_dependencies: dev.len(),
        processors: processors.len(),
        jdk_source,
    })
}

fn read_toolchain(
    root: &toml::Table,
    source: &JotSource,
    notes: &mut Vec<String>,
) -> Result<(u32, Option<String>), JxError> {
    let Some(toolchains) = table(root, "toolchains", &source.path)? else {
        notes.push(format!(
            "jot.toml has no [toolchains] java pin; using jdk = \"{}\"",
            source.default_feature
        ));
        return Ok((source.default_feature, None));
    };
    if toolchains.contains_key("kotlin") {
        return Err(JxError::new(
            Kind::Resolve,
            "this jot project pins a Kotlin toolchain; jx compiles Java only",
        )
        .at(source.path.clone())
        .hint("import a Java-only project, or keep using jot for Kotlin"));
    }
    for key in toolchains.keys() {
        if key != "java" {
            return Err(JxError::new(
                Kind::Resolve,
                format!("[toolchains] {key} is not part of jx's import"),
            )
            .at(source.path.clone())
            .hint("jx imports the `java` toolchain only"));
        }
    }

    let Some(value) = toolchains.get("java") else {
        notes.push(format!(
            "jot.toml has no java toolchain pin; using jdk = \"{}\"",
            source.default_feature
        ));
        return Ok((source.default_feature, None));
    };
    let raw = match value {
        toml::Value::Integer(number) => number.to_string(),
        toml::Value::String(text) => text.clone(),
        _ => {
            return Err(JxError::new(
                Kind::Resolve,
                "[toolchains] java must be a version string or integer",
            )
            .at(source.path.clone()));
        }
    };
    let (feature, provider) = parse_java_pin(&raw)?;
    if provider
        .as_deref()
        .is_some_and(|provider| provider != "temurin")
    {
        return Err(JxError::new(
            Kind::Resolve,
            format!(
                "jot pins Java from {}, but jx installs Eclipse Temurin only",
                provider.unwrap_or_default()
            ),
        )
        .at(source.path.clone())
        .hint(
            "change the jot pin to a plain feature release (for example `21`) before importing",
        ));
    }
    if !(8..=99).contains(&feature) {
        return Err(JxError::new(
            Kind::Resolve,
            format!("jot asks for Java {feature}, which jx cannot pin"),
        )
        .at(source.path.clone())
        .hint("V1 pins JDK feature releases between 8 and 99"));
    }
    notes.push(format!(
        "jdk = \"{feature}\" from [toolchains] java = {raw:?}"
    ));
    Ok((feature, Some(format!("[toolchains] java = {raw:?}"))))
}

/// `21`, `21.0.2`, `temurin-21` and `temurin-21.0.2` all reduce to a feature
/// release. A different vendor is returned so the caller can refuse it.
fn parse_java_pin(raw: &str) -> Result<(u32, Option<String>), JxError> {
    let raw = raw.trim();
    if let Some((provider, version)) = raw.split_once('-')
        && provider.chars().all(|c| c.is_ascii_alphabetic())
        && parse_java_version(version).is_some()
    {
        let feature = parse_java_version(version).expect("checked");
        return Ok((feature, Some(provider.to_string())));
    }
    parse_java_version(raw)
        .map(|feature| (feature, None))
        .ok_or_else(|| {
            JxError::new(Kind::Resolve, format!("cannot read Java version {raw:?}"))
                .hint("use a feature release such as `21`, or `temurin-21`")
        })
}

fn table<'a>(
    root: &'a toml::Table,
    name: &str,
    source: &str,
) -> Result<Option<&'a toml::Table>, JxError> {
    match root.get(name) {
        None => Ok(None),
        Some(toml::Value::Table(table)) => Ok(Some(table)),
        Some(_) => Err(
            JxError::new(Kind::Resolve, format!("[{name}] must be a table")).at(source.to_string()),
        ),
    }
}

fn string(value: Option<&toml::Value>, what: &str, source: &str) -> Result<String, JxError> {
    match value {
        Some(toml::Value::String(value)) => Ok(value.clone()),
        Some(_) => Err(
            JxError::new(Kind::Resolve, format!("{what} must be a string")).at(source.to_string()),
        ),
        None => Err(JxError::new(Kind::Resolve, format!("{what} is required"))
            .at(source.to_string())
            .hint("add it to [project] in jot.toml")),
    }
}

fn read_dependencies(
    table: &toml::Table,
    table_name: &str,
    source: &str,
    catalog: Option<&Catalog>,
    catalog_uses: &mut usize,
) -> Result<Vec<ImportedDependency>, JxError> {
    let mut dependencies = Vec::new();
    let mut seen: HashSet<(String, String)> = HashSet::new();
    for (alias, value) in table {
        let coordinate = match value {
            toml::Value::String(coordinate) => coordinate.clone(),
            toml::Value::Table(entry) => {
                if entry.contains_key("path") {
                    return Err(JxError::new(
                        Kind::Resolve,
                        format!("jot dependency {alias:?} uses a workspace path"),
                    )
                    .at(source.to_string())
                    .hint(
                        "jx has no workspace path dependencies; import the member project, or keep using jot",
                    ));
                }
                match entry.get("catalog").and_then(toml::Value::as_str) {
                    Some(name) => {
                        let catalog = catalog.ok_or_else(|| {
                            JxError::new(
                                Kind::Resolve,
                                format!(
                                    "jot dependency {alias:?} references catalog entry {name:?}, but libs.versions.toml was not found"
                                ),
                            )
                            .at(source.to_string())
                            .hint("put libs.versions.toml beside jot.toml, then import again")
                        })?;
                        *catalog_uses += 1;
                        catalog.resolve(name, source)?
                    }
                    None => inline_coordinate(entry, alias, source)?,
                }
            }
            _ => {
                return Err(JxError::new(
                    Kind::Resolve,
                    format!("jot dependency {alias:?} is not a coordinate or a table"),
                )
                .at(source.to_string()));
            }
        };
        let dependency = parse_coordinate(&coordinate, alias, source)?;
        let ga = (dependency.group_id.clone(), dependency.artifact_id.clone());
        if !seen.insert(ga) {
            return Err(JxError::new(
                Kind::Resolve,
                format!(
                    "[{table_name}] declares {}:{} more than once",
                    dependency.group_id, dependency.artifact_id
                ),
            )
            .at(source.to_string())
            .hint("remove the duplicate entry, then import again"));
        }
        dependencies.push(dependency);
    }
    Ok(dependencies)
}

fn inline_coordinate(entry: &toml::Table, alias: &str, source: &str) -> Result<String, JxError> {
    let (group, artifact) = match entry.get("module").and_then(toml::Value::as_str) {
        Some(module) => {
            let (group, artifact) = module.split_once(':').ok_or_else(|| {
                JxError::new(
                    Kind::Resolve,
                    format!("jot dependency {alias:?} has a module without `group:artifact`"),
                )
                .at(source.to_string())
            })?;
            (group.to_string(), artifact.to_string())
        }
        None => {
            let group = entry
                .get("group")
                .and_then(toml::Value::as_str)
                .ok_or_else(|| {
                    JxError::new(
                        Kind::Resolve,
                        format!("jot dependency {alias:?} has no module or group"),
                    )
                    .at(source.to_string())
                })?;
            let artifact = entry
                .get("name")
                .and_then(toml::Value::as_str)
                .ok_or_else(|| {
                    JxError::new(
                        Kind::Resolve,
                        format!("jot dependency {alias:?} has no name"),
                    )
                    .at(source.to_string())
                })?;
            (group.to_string(), artifact.to_string())
        }
    };
    let version = catalog_version(entry, &BTreeMap::new()).ok_or_else(|| {
        JxError::new(
            Kind::Resolve,
            format!("jot dependency {alias:?} has no version"),
        )
        .at(source.to_string())
        .hint("add a version, or reference a libs.versions.toml catalog entry")
    })?;
    Ok(format!("{group}:{artifact}:{version}"))
}

fn parse_coordinate(
    coordinate: &str,
    alias: &str,
    source: &str,
) -> Result<ImportedDependency, JxError> {
    let parts: Vec<&str> = coordinate.trim().split(':').collect();
    if !(3..=4).contains(&parts.len()) {
        return Err(JxError::new(
            Kind::Resolve,
            format!("jot dependency {alias:?} is not `group:artifact:version[:classifier]`"),
        )
        .at(source.to_string())
        .hint(format!("got {coordinate:?}")));
    }
    let (group, artifact, version) = (parts[0], parts[1], parts[2]);
    if !identifier(group) || !identifier(artifact) {
        return Err(JxError::new(
            Kind::Resolve,
            format!("jot dependency {alias:?} has an invalid group or artifact id"),
        )
        .at(source.to_string())
        .hint("use letters, digits and `.`, `_`, `-`"));
    }
    if version.is_empty()
        || !version
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '+'))
    {
        return Err(JxError::new(
            Kind::Resolve,
            format!("jot dependency {alias:?} has a version jx cannot write: {version:?}"),
        )
        .at(source.to_string()));
    }
    if version.to_ascii_lowercase().ends_with("-snapshot") {
        return Err(JxError::new(
            Kind::Resolve,
            format!("jot dependency {alias:?} uses SNAPSHOT version {version}"),
        )
        .at(source.to_string())
        .hint("jx's lockfile pins release bytes; use a released version"));
    }
    if version.starts_with('[') || version.starts_with('(') || version.contains(',') {
        return Err(JxError::new(
            Kind::Resolve,
            format!("jot dependency {alias:?} uses the version range {version}"),
        )
        .at(source.to_string())
        .hint("replace the range with the version jot actually resolves"));
    }
    let classifier = parts.get(3).map(|value| value.to_string());
    if classifier
        .as_deref()
        .is_some_and(|value| !identifier(value))
    {
        return Err(JxError::new(
            Kind::Resolve,
            format!("jot dependency {alias:?} has an invalid classifier"),
        )
        .at(source.to_string()));
    }
    Ok(ImportedDependency {
        group_id: group.to_string(),
        artifact_id: artifact.to_string(),
        version: version.to_string(),
        classifier,
    })
}

fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// The `[versions]` and `[libraries]` tables of a Gradle/jot version catalog.
struct Catalog {
    path: String,
    versions: BTreeMap<String, String>,
    libraries: toml::Table,
}

impl Catalog {
    fn load(path: &str, text: &str, source: &str) -> Result<Catalog, JxError> {
        let root: toml::Table = toml::from_str(text).map_err(|e| {
            JxError::new(Kind::Resolve, "cannot parse libs.versions.toml")
                .at(path.to_string())
                .caused_by(e)
                .hint("fix the catalog syntax, then import again")
        })?;
        let versions = match root.get("versions") {
            None => BTreeMap::new(),
            Some(toml::Value::Table(table)) => {
                let mut versions = BTreeMap::new();
                for (name, value) in table {
                    if let Some(version) = value.as_str() {
                        versions.insert(name.clone(), version.to_string());
                    } else {
                        return Err(JxError::new(
                            Kind::Resolve,
                            format!("catalog version {name:?} must be a string"),
                        )
                        .at(path.to_string()));
                    }
                }
                versions
            }
            Some(_) => {
                return Err(
                    JxError::new(Kind::Resolve, "[versions] must be a table").at(path.to_string())
                );
            }
        };
        let libraries = match root.get("libraries") {
            Some(toml::Value::Table(table)) => table.clone(),
            Some(_) => {
                return Err(
                    JxError::new(Kind::Resolve, "[libraries] must be a table").at(path.to_string())
                );
            }
            None => {
                return Err(JxError::new(
                    Kind::Resolve,
                    "libs.versions.toml has no [libraries] table",
                )
                .at(path.to_string())
                .hint("add the catalog entry, or inline the dependency in jot.toml"));
            }
        };
        // The source path is used by the caller when a lookup fails.
        let _ = source;
        Ok(Catalog {
            path: path.to_string(),
            versions,
            libraries,
        })
    }

    fn resolve(&self, name: &str, source: &str) -> Result<String, JxError> {
        let entry = self.libraries.get(name).ok_or_else(|| {
            JxError::new(
                Kind::Resolve,
                format!("libs.versions.toml has no library named {name:?}"),
            )
            .at(self.path.clone())
            .hint(format!("referenced from {source}"))
        })?;
        match entry {
            toml::Value::String(coordinate) => Ok(coordinate.clone()),
            toml::Value::Table(table) => {
                let (group, artifact) = match table.get("module").and_then(toml::Value::as_str) {
                    Some(module) => {
                        let (group, artifact) = module.split_once(':').ok_or_else(|| {
                            JxError::new(
                                Kind::Resolve,
                                format!("catalog library {name:?} has a malformed module"),
                            )
                            .at(self.path.clone())
                        })?;
                        (group, artifact)
                    }
                    None => (
                        table
                            .get("group")
                            .and_then(toml::Value::as_str)
                            .ok_or_else(|| {
                                JxError::new(
                                    Kind::Resolve,
                                    format!("catalog library {name:?} has no module or group"),
                                )
                                .at(self.path.clone())
                            })?,
                        table
                            .get("name")
                            .and_then(toml::Value::as_str)
                            .ok_or_else(|| {
                                JxError::new(
                                    Kind::Resolve,
                                    format!("catalog library {name:?} has no name"),
                                )
                                .at(self.path.clone())
                            })?,
                    ),
                };
                let version = catalog_version(table, &self.versions).ok_or_else(|| {
                    JxError::new(
                        Kind::Resolve,
                        format!("catalog library {name:?} has no version or version.ref"),
                    )
                    .at(self.path.clone())
                })?;
                Ok(format!("{group}:{artifact}:{version}"))
            }
            _ => Err(JxError::new(
                Kind::Resolve,
                format!("catalog library {name:?} must be a string or a table"),
            )
            .at(self.path.clone())),
        }
    }
}

fn catalog_version(table: &toml::Table, versions: &BTreeMap<String, String>) -> Option<String> {
    match table.get("version")? {
        toml::Value::String(version) => Some(version.clone()),
        toml::Value::Table(version) => {
            if let Some(name) = version.get("ref").and_then(toml::Value::as_str) {
                versions.get(name).cloned()
            } else {
                version
                    .get("require")
                    .and_then(toml::Value::as_str)
                    .map(str::to_string)
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(text: &str) -> JotSource {
        JotSource {
            path: "jot.toml".into(),
            text: text.into(),
            catalog: None,
            default_version: "0.1.0".into(),
            default_feature: 25,
        }
    }

    #[test]
    fn imports_project_toolchain_dependencies_and_main() {
        let imported = manifest_from_jot(&source(
            r#"
[project]
name = "demo"
version = "1.2.3"
main-class = "com.example.Main"

[toolchains]
java = "21"

[dependencies]
guava = "com.google.guava:guava:33.4.8-jre"

[test-dependencies]
junit = "org.junit.jupiter:junit-jupiter:5.11.4"
"#,
        ))
        .unwrap();

        assert!(imported.text.contains("name = \"demo\""));
        assert!(imported.text.contains("jdk = \"21\""));
        assert!(imported.text.contains("[run]\nmain = \"com.example.Main\""));
        assert!(
            imported
                .text
                .contains("\"guava\" = \"com.google.guava:guava:=33.4.8-jre\"")
        );
        assert!(imported.text.contains("[dev-dependencies]"));
        assert_eq!(imported.dependencies, 1);
        assert_eq!(imported.dev_dependencies, 1);
        assert!(!imported.text.contains("[workspace]"));
    }

    #[test]
    fn resolves_catalog_references() {
        let mut source = source(
            r#"
[project]
name = "demo"
version = "1.0.0"

[toolchains]
java = "17"

[dependencies]
junit = { catalog = "junit" }
"#,
        );
        source.catalog = Some((
            "libs.versions.toml".into(),
            r#"
[versions]
junit = "5.11.4"

[libraries]
junit = { module = "org.junit.jupiter:junit-jupiter", version.ref = "junit" }
"#
            .into(),
        ));

        let imported = manifest_from_jot(&source).unwrap();
        assert!(
            imported
                .text
                .contains("\"junit-jupiter\" = \"org.junit.jupiter:junit-jupiter:=5.11.4\"")
        );
        assert!(
            imported
                .notes
                .iter()
                .any(|note| note.contains("libs.versions.toml"))
        );
    }

    #[test]
    fn imports_the_real_jot_java_web_sample_with_processors() {
        let imported = manifest_from_jot(&source(
            r#"
[project]
name = "webserver"
version = "0.1.0"
main-class = "com.jot.samples.webserver.ServerMain"
layout = "maven"
[toolchains]
java = "21"
[dependencies]
mapstruct = "org.mapstruct:mapstruct:1.6.3"
[processors]
mapstruct = "org.mapstruct:mapstruct-processor:1.6.3"
[test-dependencies]
junit = "org.junit.jupiter:junit-jupiter:5.11.0"
"#,
        ))
        .unwrap();

        assert!(imported.text.contains("[processors]"));
        assert!(
            imported
                .text
                .contains("\"mapstruct-processor\" = \"org.mapstruct:mapstruct-processor:=1.6.3\""),
            "{}",
            imported.text
        );
        assert_eq!(imported.dependencies, 1);
        assert_eq!(imported.dev_dependencies, 1);
        assert_eq!(imported.processors, 1);
    }

    #[test]
    fn flat_layout_is_reported_when_explicit() {
        let imported = manifest_from_jot(&source(
            r#"
[project]
name = "demo"
layout = "flat"

[toolchains]
java = "21"
"#,
        ))
        .unwrap();
        assert!(
            imported
                .notes
                .iter()
                .any(|note| note.contains("src/main/java"))
        );
    }

    #[test]
    fn omitted_layout_means_jots_flat_default() {
        let imported = manifest_from_jot(&source(
            r#"
[project]
name = "demo"

[toolchains]
java = "21"
"#,
        ))
        .unwrap();
        assert!(
            imported
                .notes
                .iter()
                .any(|note| note.contains("flat layout") && note.contains("src/main/java")),
            "{:?}",
            imported.notes
        );
    }

    #[test]
    fn wrong_typed_project_fields_are_refused() {
        for (field, text) in [
            ("version", "version = 1"),
            ("main-class", "main-class = 1"),
            ("layout", "layout = 1"),
            ("group", "group = 1"),
        ] {
            let text =
                format!("[project]\nname = \"demo\"\n{text}\n\n[toolchains]\njava = \"21\"\n");
            let error = manifest_from_jot(&source(&text)).expect_err(field);
            assert!(
                error.message.contains(field) && error.message.contains("must be a string"),
                "{field}: {error}"
            );
        }
    }

    #[test]
    fn publishing_group_is_reported_as_not_imported() {
        let imported = manifest_from_jot(&source(
            r#"
[project]
name = "demo"
group = "com.example"
layout = "maven"

[toolchains]
java = "21"
"#,
        ))
        .unwrap();
        assert!(
            imported.notes.iter().any(|note| {
                note.contains("group")
                    && note.contains("not imported")
                    && note.contains("export maven")
            }),
            "{:?}",
            imported.notes
        );
    }

    #[test]
    fn workspaces_are_refused() {
        let error = manifest_from_jot(&source(
            r#"
[workspace]
members = ["api", "cli"]
"#,
        ))
        .expect_err("workspace");
        assert!(error.message.contains("workspace"), "{error}");
        assert!(
            error
                .hint
                .as_deref()
                .is_some_and(|hint| hint.contains("member directory")),
            "{error}"
        );
    }

    #[test]
    fn path_dependencies_are_refused() {
        let error = manifest_from_jot(&source(
            r#"
[project]
name = "demo"

[toolchains]
java = "21"

[dependencies]
domain = { path = "../domain" }
"#,
        ))
        .expect_err("path dependency");
        assert!(error.message.contains("workspace path"), "{error}");
    }

    #[test]
    fn a_non_temurin_pin_is_refused() {
        let error = manifest_from_jot(&source(
            r#"
[project]
name = "demo"

[toolchains]
java = "corretto-21"
"#,
        ))
        .expect_err("corretto");
        assert!(error.message.contains("Temurin"), "{error}");
        assert!(error.message.contains("corretto"), "{error}");
    }

    #[test]
    fn a_kotlin_toolchain_is_refused() {
        let error = manifest_from_jot(&source(
            r#"
[project]
name = "demo"

[toolchains]
kotlin = "2.1.0"
"#,
        ))
        .expect_err("kotlin");
        assert!(error.message.contains("Kotlin"), "{error}");
    }
}
