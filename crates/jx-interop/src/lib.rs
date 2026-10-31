//! Maven and Gradle interoperability.
//!
//! The first M7 slice is Maven import: turn an effective POM into a jx
//! `project.toml`. Import is deliberately conservative — every approximation
//! is collected as a note and shown before the manifest is written, and
//! unsupported packaging, version ranges, unresolved versions or SNAPSHOTs
//! stop the import instead of producing a manifest that builds differently.
//!
//! Gradle import and delegated Maven/Gradle builds are still ahead.

use std::collections::HashSet;

use jx_common::error::{JxError, Kind};
use jx_maven::EffectivePom;
use jx_maven::xml::Dependency;

/// A generated manifest plus the honest account of what changed.
#[derive(Debug, Clone)]
pub struct ImportedManifest {
    pub text: String,
    /// Approximations and skipped POM features, in stable order.
    pub notes: Vec<String>,
    pub dependencies: usize,
    pub dev_dependencies: usize,
    /// Whether versions came from the POM's own compiler settings.
    pub jdk_source: Option<String>,
}

/// One direct dependency on its way into the manifest.
struct ImportedDependency {
    group_id: String,
    artifact_id: String,
    version: String,
    classifier: Option<String>,
}

/// Convert an effective POM into a new jx manifest.
///
/// `source` is only used in comments and diagnostics (usually the POM path).
/// `default_feature` is used when the POM carries no compiler release, which
/// is normal for POMs that rely on Maven's default; that choice is noted.
pub fn manifest_from_pom(
    pom: &EffectivePom,
    source: &str,
    default_feature: u32,
) -> Result<ImportedManifest, JxError> {
    let packaging = pom.packaging.to_ascii_lowercase();
    if packaging != "jar" {
        return Err(JxError::new(
            Kind::Resolve,
            format!(
                "{}:{} declares <packaging>{}</packaging>; jx builds jar applications",
                pom.coords.group_id, pom.coords.artifact_id, pom.packaging
            ),
        )
        .at(source.to_string())
        .hint(if packaging == "pom" {
            "an aggregator POM has no application sources; import the module's pom.xml instead"
        } else {
            "convert the packaging to jar first, or keep using Maven for this project"
        }));
    }
    if pom.coords.artifact_id.is_empty() || pom.coords.version.is_empty() {
        return Err(JxError::new(
            Kind::Resolve,
            "the POM does not resolve to a project name and version",
        )
        .at(source.to_string())
        .hint("give the POM an <artifactId> and a <version> (directly or through its parent)"));
    }
    check_value("project name", &pom.coords.artifact_id, source)?;
    check_value("project version", &pom.coords.version, source)?;

    let mut notes = Vec::new();
    let (detected_jdk, detected_source) = detect_jdk(pom);
    let (jdk, jdk_source) = match detected_jdk {
        Some(feature) => {
            notes.push(format!(
                "jdk = \"{feature}\" from the POM's {detected_source}"
            ));
            (feature, Some(detected_source))
        }
        None => {
            notes.push(format!(
                "no compiler release in the POM; using jdk = \"{default_feature}\" (the jx default)"
            ));
            (default_feature, None)
        }
    };
    if !(8..=99).contains(&jdk) {
        return Err(JxError::new(
            Kind::Resolve,
            format!("the POM asks for Java {jdk}, which jx cannot pin"),
        )
        .at(source.to_string())
        .hint("V1 pins JDK feature releases between 8 and 99"));
    }

    let mut main: Vec<ImportedDependency> = Vec::new();
    let mut dev: Vec<ImportedDependency> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();
    let mut optional = 0usize;
    let mut runtime = 0usize;
    let mut provided = 0usize;
    let mut exclusions = 0usize;
    for dep in &pom.dependencies {
        let scope = dep.scope.as_deref().unwrap_or("compile");
        if matches!(scope, "import") {
            continue;
        }
        let target = match scope {
            "compile" => &mut main,
            "test" => &mut dev,
            "runtime" => {
                runtime += 1;
                &mut main
            }
            "provided" => {
                provided += 1;
                &mut main
            }
            "system" => {
                skipped.push(format!(
                    "{}:{} (system scope has a local systemPath)",
                    dep.group_id, dep.artifact_id
                ));
                continue;
            }
            other => {
                skipped.push(format!(
                    "{}:{} (unsupported {} scope)",
                    dep.group_id, dep.artifact_id, other
                ));
                continue;
            }
        };
        if let Some(package_type) = dep.package_type.as_deref()
            && package_type != "jar"
        {
            skipped.push(format!(
                "{}:{} (type {package_type})",
                dep.group_id, dep.artifact_id
            ));
            continue;
        }
        let version = dependency_version(dep, source)?;
        if dep.optional {
            optional += 1;
        }
        if !dep.exclusions.is_empty() {
            exclusions += dep.exclusions.len();
        }
        check_ident("dependency groupId", &dep.group_id, source)?;
        check_ident("dependency artifactId", &dep.artifact_id, source)?;
        if let Some(classifier) = &dep.classifier {
            check_ident("dependency classifier", classifier, source)?;
        }
        if target.iter().any(|existing| {
            existing.group_id == dep.group_id && existing.artifact_id == dep.artifact_id
        }) {
            return Err(JxError::new(
                Kind::Resolve,
                format!(
                    "{}:{} is declared more than once, possibly with classifiers",
                    dep.group_id, dep.artifact_id
                ),
            )
            .at(source.to_string())
            .hint(
                "project.toml cannot declare two classifiers of the same artifact yet; remove one or keep Maven for this module",
            ));
        }
        target.push(ImportedDependency {
            group_id: dep.group_id.clone(),
            artifact_id: dep.artifact_id.clone(),
            version,
            classifier: dep.classifier.clone(),
        });
    }

    if !skipped.is_empty() {
        notes.push(format!(
            "skipped {} direct dependency/dependencies jx cannot represent: {}",
            skipped.len(),
            skipped.join(", ")
        ));
    }
    if runtime > 0 {
        notes.push(format!(
            "{runtime} runtime-scope dependenc{} moved to [dependencies]; jx has no separate runtime scope yet",
            if runtime == 1 { "y was" } else { "ies were" }
        ));
    }
    if provided > 0 {
        notes.push(format!(
            "{provided} provided-scope dependenc{} imported as normal because jx has no provided scope; {} also packaged",
            if provided == 1 { "y was" } else { "ies were" },
            if provided == 1 { "it will be" } else { "they will be" }
        ));
    }
    if optional > 0 {
        notes.push(format!(
            "{optional} optional dependenc{} imported as normal; jx does not model optionality",
            if optional == 1 { "y was" } else { "ies were" }
        ));
    }
    if exclusions > 0 {
        notes.push(format!(
            "{exclusions} Maven exclusion(s) are not represented in project.toml; jx's resolver may include artifacts Maven excluded"
        ));
    }
    if !dev.is_empty() {
        notes.push(
            "test dependencies were imported, but `jx test` runs JUnit 5 through the JUnit Platform console standalone; if the POM does not declare it, add it with `jx add --dev org.junit.platform:junit-platform-console-standalone`"
                .to_string(),
        );
    }

    let bom_imports: Vec<String> = pom
        .dependency_management
        .iter()
        .filter(|dep| dep.scope.as_deref() == Some("import"))
        .map(|dep| format!("{}:{}", dep.group_id, dep.artifact_id))
        .collect();
    if !bom_imports.is_empty() {
        let shown = bom_imports.iter().take(4).cloned().collect::<Vec<_>>();
        let more = bom_imports.len().saturating_sub(shown.len());
        let suffix = if more == 0 {
            String::new()
        } else {
            format!(", and {more} more")
        };
        notes.push(format!(
            "{} BOM import(s) were expanded at import time ({}{}); direct versions are pinned exactly, so no platform entries were written",
            bom_imports.len(),
            shown.join(", "),
            suffix
        ));
    }
    if !pom.repositories.is_empty() {
        let ids: Vec<&str> = pom
            .repositories
            .iter()
            .map(|repo| repo.id.as_str())
            .collect();
        notes.push(format!(
            "POM repositories ({}) are not migrated; jx only trusts repositories configured in config.toml",
            ids.join(", ")
        ));
    }
    notes.push(
        "direct dependencies are exact-pinned with `=`; transitive versions still follow jx's nearest-wins resolver"
            .to_string(),
    );

    let text = render(
        &pom.coords.artifact_id,
        &pom.coords.version,
        jdk,
        &main,
        &dev,
        source,
    );
    Ok(ImportedManifest {
        text,
        notes,
        dependencies: main.len(),
        dev_dependencies: dev.len(),
        jdk_source,
    })
}

/// The version a dependency will be pinned to. Maven leaves versions absent
/// when dependencyManagement was supposed to supply one; import must not
/// silently choose something else.
fn dependency_version(dep: &Dependency, source: &str) -> Result<String, JxError> {
    let raw = dep
        .version
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty());
    let Some(version) = raw else {
        return Err(JxError::new(
            Kind::Resolve,
            format!(
                "{}:{} has no version after Maven's dependencyManagement was applied",
                dep.group_id, dep.artifact_id
            ),
        )
        .at(source.to_string())
        .hint(
            "add a <version> to the dependency, or move the BOM that manages it into <dependencyManagement>; jx cannot express a versionless dependency without a platform",
        ));
    };
    if version.contains("${") {
        return Err(JxError::new(
            Kind::Resolve,
            format!(
                "{}:{} still has an unresolved property in its version ({version})",
                dep.group_id, dep.artifact_id
            ),
        )
        .at(source.to_string())
        .hint("define the property in the POM (or its parent) and import again"));
    }
    if version.starts_with('[') || version.starts_with('(') || version.contains(',') {
        return Err(JxError::new(
            Kind::Resolve,
            format!(
                "{}:{} uses the version range {version}",
                dep.group_id, dep.artifact_id
            ),
        )
        .at(source.to_string())
        .hint("jx resolves exact release versions only; replace the range with the version Maven actually selects"));
    }
    if version.to_ascii_lowercase().ends_with("-snapshot") {
        return Err(JxError::new(
            Kind::Resolve,
            format!(
                "{}:{} depends on the SNAPSHOT version {version}",
                dep.group_id, dep.artifact_id
            ),
        )
        .at(source.to_string())
        .hint("jx's lockfile pins release bytes; use a released dependency version"));
    }
    let allowed = version
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '+'));
    if !allowed {
        return Err(JxError::new(
            Kind::Resolve,
            format!(
                "{}:{} has a version jx cannot write to project.toml: {version:?}",
                dep.group_id, dep.artifact_id
            ),
        )
        .at(source.to_string())
        .hint("versions may contain letters, digits and `.`, `_`, `-`, `+`"));
    }
    Ok(version.to_string())
}

/// The compiler release to pin, and the property it came from. Release beats
/// target; `java.version` is a common Spring Boot property; source is the
/// last-resort compiler setting.
fn detect_jdk(pom: &EffectivePom) -> (Option<u32>, String) {
    for key in [
        "maven.compiler.release",
        "maven.compiler.target",
        "java.version",
        "maven.compiler.source",
    ] {
        let Some(raw) = pom
            .properties
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.trim())
        else {
            continue;
        };
        if let Some(feature) = parse_java_version(raw) {
            return (Some(feature), format!("{key} = {raw}"));
        }
    }
    (None, String::new())
}

/// `8`, `1.8`, `17`, `21.0.2` and `1.8.0_202` all name a feature release.
fn parse_java_version(raw: &str) -> Option<u32> {
    let raw = raw.trim();
    if let Some(rest) = raw.strip_prefix("1.") {
        return rest
            .split(['.', '_', '-'])
            .next()
            .and_then(|part| part.parse::<u32>().ok());
    }
    raw.split(['.', '_', '-'])
        .next()
        .and_then(|part| part.parse::<u32>().ok())
}

fn check_value(what: &str, value: &str, source: &str) -> Result<(), JxError> {
    if value.chars().any(char::is_control) {
        return Err(JxError::new(
            Kind::Resolve,
            format!("the POM's {what} contains a control character"),
        )
        .at(source.to_string())
        .hint("remove control characters from the POM before importing"));
    }
    Ok(())
}

/// Maven identifiers and jx's manifest use the same safe alphabet. Checking
/// here keeps an invalid POM a clear import error rather than a generated
/// manifest that fails its own validation later.
fn check_ident(what: &str, value: &str, source: &str) -> Result<(), JxError> {
    if value.is_empty()
        || !value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        return Err(JxError::new(
            Kind::Resolve,
            format!("the POM's {what} {value:?} is not a valid Maven identifier"),
        )
        .at(source.to_string())
        .hint("use letters, digits and `.`, `_`, `-`"));
    }
    Ok(())
}

fn render(
    name: &str,
    version: &str,
    jdk: u32,
    main: &[ImportedDependency],
    dev: &[ImportedDependency],
    source: &str,
) -> String {
    let source = source.replace(['\r', '\n'], " ");
    let mut out = String::new();
    out.push_str("# Imported from ");
    out.push_str(&source);
    out.push_str(" by `jx import`.\n");
    out.push_str("# Direct dependencies are exact-pinned. Review the import notes,\n");
    out.push_str("# then run `jx lock` and `jx build` to verify the conversion.\n\n");
    out.push_str("[project]\n");
    out.push_str(&format!("name = {}\n", toml_string(name)));
    out.push_str(&format!("version = {}\n", toml_string(version)));
    out.push_str(&format!("jdk = {}\n", toml_string(&jdk.to_string())));

    let mut used: HashSet<String> = HashSet::new();
    if !main.is_empty() {
        out.push_str("\n[dependencies]\n");
        for dep in main {
            push_dependency(&mut out, dep, &mut used);
        }
    }
    if !dev.is_empty() {
        out.push_str("\n[dev-dependencies]\n");
        for dep in dev {
            push_dependency(&mut out, dep, &mut used);
        }
    }
    out
}

fn push_dependency(out: &mut String, dep: &ImportedDependency, used: &mut HashSet<String>) {
    let mut alias = dep.artifact_id.clone();
    let mut suffix = 2;
    while used.contains(&alias) {
        alias = format!("{}-{suffix}", dep.artifact_id);
        suffix += 1;
    }
    used.insert(alias.clone());
    let mut coordinate = format!("{}:{}", dep.group_id, dep.artifact_id);
    coordinate.push_str(&format!(":={}", dep.version));
    if let Some(classifier) = &dep.classifier {
        coordinate.push(':');
        coordinate.push_str(classifier);
    }
    out.push_str(&format!(
        "{} = {}\n",
        toml_key(&alias),
        toml_string(&coordinate)
    ));
}

/// Always write a quoted TOML key: artifact ids may contain dots, and a
/// quoted key keeps every legal Maven id valid here without rewriting names.
fn toml_key(value: &str) -> String {
    toml_string(value)
}

fn toml_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{:04X}", c as u32)),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use jx_maven::effective::Coords;
    use jx_maven::xml::Dependency;

    fn dep(group: &str, artifact: &str, version: Option<&str>) -> Dependency {
        Dependency {
            group_id: group.to_string(),
            artifact_id: artifact.to_string(),
            version: version.map(str::to_string),
            package_type: None,
            classifier: None,
            scope: None,
            optional: false,
            exclusions: Vec::new(),
        }
    }

    fn pom(dependencies: Vec<Dependency>) -> EffectivePom {
        EffectivePom {
            coords: Coords {
                group_id: "com.example".into(),
                artifact_id: "app".into(),
                version: "1.2.3".into(),
            },
            packaging: "jar".into(),
            properties: vec![("maven.compiler.release".into(), "17".into())],
            dependencies,
            dependency_management: Vec::new(),
            repositories: Vec::new(),
        }
    }

    #[test]
    fn converts_coordinates_scopes_and_classifiers() {
        let mut guava = dep("com.google.guava", "guava", Some("33.4.8-jre"));
        guava.classifier = Some("jre".into());
        let mut junit = dep("org.junit.jupiter", "junit-jupiter", Some("5.11.4"));
        junit.scope = Some("test".into());
        let mut provided = dep("jakarta.servlet", "jakarta.servlet-api", Some("6.0.0"));
        provided.scope = Some("provided".into());
        let imported =
            manifest_from_pom(&pom(vec![guava, junit, provided]), "pom.xml", 25).unwrap();

        assert!(imported.text.contains("name = \"app\""));
        assert!(imported.text.contains("version = \"1.2.3\""));
        assert!(imported.text.contains("jdk = \"17\""));
        assert!(
            imported
                .text
                .contains("\"guava\" = \"com.google.guava:guava:=33.4.8-jre:jre\"")
        );
        assert!(imported.text.contains(
            "[dev-dependencies]\n\"junit-jupiter\" = \"org.junit.jupiter:junit-jupiter:=5.11.4\""
        ));
        assert!(
            imported.text.contains(
                "\"jakarta.servlet-api\" = \"jakarta.servlet:jakarta.servlet-api:=6.0.0\""
            )
        );
        assert_eq!(imported.dependencies, 2);
        assert_eq!(imported.dev_dependencies, 1);
        assert!(
            imported
                .notes
                .iter()
                .any(|n| n.contains("provided scope") && n.contains("also packaged"))
        );
        assert!(
            imported
                .notes
                .iter()
                .any(|n| n.contains("junit-platform-console-standalone")),
            "test imports must explain jx's JUnit launcher requirement"
        );
        assert_eq!(
            imported.jdk_source.as_deref(),
            Some("maven.compiler.release = 17")
        );
    }

    #[test]
    fn missing_version_is_an_error_not_a_guess() {
        let error = manifest_from_pom(&pom(vec![dep("g", "a", None)]), "pom.xml", 25)
            .expect_err("no version");
        assert!(error.message.contains("has no version"), "{error}");
    }

    #[test]
    fn ranges_snapshots_and_unresolved_properties_are_rejected() {
        for version in ["[1.0,2.0)", "1.0-SNAPSHOT", "${x.version}"] {
            let error = manifest_from_pom(&pom(vec![dep("g", "a", Some(version))]), "pom.xml", 25)
                .expect_err("unsupported version");
            assert!(!error.message.is_empty(), "{version}");
        }
    }

    #[test]
    fn non_jar_packaging_is_rejected() {
        let mut project = pom(Vec::new());
        project.packaging = "pom".into();
        let error = manifest_from_pom(&project, "pom.xml", 25).expect_err("aggregator");
        assert!(error.message.contains("packaging"), "{error}");
    }

    #[test]
    fn duplicate_artifact_with_two_classifiers_is_refused() {
        let mut native = dep("org.example", "native", Some("1.0"));
        native.classifier = Some("linux-x86_64".into());
        let mut windows = dep("org.example", "native", Some("1.0"));
        windows.classifier = Some("windows-x86_64".into());
        let error = manifest_from_pom(&pom(vec![native, windows]), "pom.xml", 25)
            .expect_err("two classifiers cannot be expressed");
        assert!(error.message.contains("more than once"), "{error}");
    }

    #[test]
    fn invalid_maven_identifier_is_diagnosed_before_rendering() {
        let error = manifest_from_pom(
            &pom(vec![dep("bad group", "a", Some("1.0"))]),
            "pom.xml",
            25,
        )
        .expect_err("invalid group id");
        assert!(error.message.contains("groupId"), "{error}");
    }

    #[test]
    fn duplicate_aliases_get_distinct_names() {
        let project = pom(vec![
            dep("one.example", "common", Some("1.0")),
            dep("two.example", "common", Some("2.0")),
        ]);
        let imported = manifest_from_pom(&project, "pom.xml", 25).unwrap();
        assert!(imported.text.contains("\"common\" = "));
        assert!(imported.text.contains("\"common-2\" = "));
    }

    #[test]
    fn java_version_spellings() {
        assert_eq!(parse_java_version("8"), Some(8));
        assert_eq!(parse_java_version("1.8"), Some(8));
        assert_eq!(parse_java_version("1.8.0_202"), Some(8));
        assert_eq!(parse_java_version("21.0.2"), Some(21));
        assert_eq!(parse_java_version("not-a-version"), None);
    }
}
