//! `jx export maven`: a POM that mirrors the jx manifest and lockfile.
//!
//! The export is intentionally conservative: direct dependencies carry the
//! concrete versions recorded in `jx.lock`, dev dependencies become `test`
//! scope, classifiers are preserved, and `[overrides]` become managed
//! dependencies. Platform BOMs are not exported (their effect is already in
//! the locked versions), and transitive exclusions are not recreated. The
//! generated file is a starting point for a Maven build, not a promise that
//! plugin configuration is identical.

use std::fmt::Write as _;

use jx_common::error::{JxError, Kind};

/// One dependency in the exported POM.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MavenDependency {
    pub group_id: String,
    pub artifact_id: String,
    pub version: String,
    pub classifier: Option<String>,
    /// `None` means Maven's default `compile` scope.
    pub scope: Option<&'static str>,
}

impl MavenDependency {
    pub fn new(
        group_id: impl Into<String>,
        artifact_id: impl Into<String>,
        version: impl Into<String>,
    ) -> Self {
        MavenDependency {
            group_id: group_id.into(),
            artifact_id: artifact_id.into(),
            version: version.into(),
            classifier: None,
            scope: None,
        }
    }

    pub fn classifier(mut self, classifier: impl Into<String>) -> Self {
        self.classifier = Some(classifier.into());
        self
    }

    pub fn test_scope(mut self) -> Self {
        self.scope = Some("test");
        self
    }
}

/// Everything needed to render a POM.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MavenProject {
    /// Maven has no equivalent of jx's project name alone, so the caller
    /// supplies a groupId (the CLI defaults to `local.jx`).
    pub group_id: String,
    pub artifact_id: String,
    pub version: String,
    pub java_release: u32,
    pub main_class: Option<String>,
    pub dependencies: Vec<MavenDependency>,
    /// Locked versions for `[overrides]`, emitted as managed dependencies.
    pub overrides: Vec<MavenDependency>,
}

/// Render a deterministic `pom.xml`.
pub fn pom(project: &MavenProject) -> Result<String, JxError> {
    validate_ident("groupId", &project.group_id)?;
    validate_ident("artifactId", &project.artifact_id)?;
    validate_value("version", &project.version)?;
    if let Some(main) = &project.main_class {
        validate_value("mainClass", main)?;
    }
    for dependency in project.dependencies.iter().chain(&project.overrides) {
        validate_ident("dependency groupId", &dependency.group_id)?;
        validate_ident("dependency artifactId", &dependency.artifact_id)?;
        validate_value("dependency version", &dependency.version)?;
        if let Some(classifier) = &dependency.classifier {
            validate_value("dependency classifier", classifier)?;
        }
        if let Some(scope) = dependency.scope
            && !matches!(
                scope,
                "compile" | "provided" | "runtime" | "test" | "system"
            )
        {
            return Err(JxError::new(
                Kind::Internal,
                format!("{scope:?} is not a Maven scope"),
            ));
        }
    }

    let mut out = String::new();
    out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    out.push_str(
        "<project xmlns=\"http://maven.apache.org/POM/4.0.0\"\n\
         \x20        xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\"\n\
         \x20        xsi:schemaLocation=\"http://maven.apache.org/POM/4.0.0 \
         https://maven.apache.org/xsd/maven-4.0.0.xsd\">\n",
    );
    out.push_str("  <modelVersion>4.0.0</modelVersion>\n");
    push_element(&mut out, 2, "groupId", &project.group_id);
    push_element(&mut out, 2, "artifactId", &project.artifact_id);
    push_element(&mut out, 2, "version", &project.version);
    out.push_str("  <properties>\n");
    push_element(&mut out, 4, "project.build.sourceEncoding", "UTF-8");
    push_element(
        &mut out,
        4,
        "maven.compiler.release",
        &project.java_release.to_string(),
    );
    out.push_str("  </properties>\n");

    if !project.dependencies.is_empty() {
        out.push_str("  <dependencies>\n");
        for dependency in &project.dependencies {
            push_dependency(&mut out, 4, dependency);
        }
        out.push_str("  </dependencies>\n");
    }

    if !project.overrides.is_empty() {
        out.push_str("  <dependencyManagement>\n");
        out.push_str("    <dependencies>\n");
        for override_dep in &project.overrides {
            push_dependency(&mut out, 6, override_dep);
        }
        out.push_str("    </dependencies>\n");
        out.push_str("  </dependencyManagement>\n");
    }

    out.push_str("  <build>\n");
    out.push_str("    <plugins>\n");
    push_plugin(&mut out, "maven-compiler-plugin", "3.13.0", None, false);
    if let Some(main) = &project.main_class {
        push_plugin(&mut out, "maven-jar-plugin", "3.4.2", Some(main), false);
        push_dependency_copy_plugin(&mut out);
    }
    if project
        .dependencies
        .iter()
        .any(|dependency| dependency.scope == Some("test"))
    {
        push_plugin(&mut out, "maven-surefire-plugin", "3.5.2", None, true);
    }
    out.push_str("    </plugins>\n");
    out.push_str("  </build>\n");
    out.push_str("</project>\n");
    Ok(out)
}

fn push_dependency(out: &mut String, indent: usize, dependency: &MavenDependency) {
    let pad = " ".repeat(indent);
    let _ = writeln!(out, "{pad}<dependency>");
    push_element(out, indent + 2, "groupId", &dependency.group_id);
    push_element(out, indent + 2, "artifactId", &dependency.artifact_id);
    push_element(out, indent + 2, "version", &dependency.version);
    if let Some(classifier) = &dependency.classifier {
        push_element(out, indent + 2, "classifier", classifier);
    }
    if let Some(scope) = dependency.scope {
        push_element(out, indent + 2, "scope", scope);
    }
    let _ = writeln!(out, "{pad}</dependency>");
}

/// The plugin block is deliberately small: compiler release, jar main class
/// and surefire for JUnit 5. No jx-specific plugin exists, and no lifecycle
/// behaviour beyond a conventional Maven build is implied.
fn push_plugin(
    out: &mut String,
    artifact_id: &str,
    version: &str,
    main_class: Option<&str>,
    junit5: bool,
) {
    out.push_str("      <plugin>\n");
    push_element(out, 8, "artifactId", artifact_id);
    push_element(out, 8, "version", version);
    if let Some(main) = main_class {
        out.push_str("        <configuration>\n");
        out.push_str("          <archive>\n");
        out.push_str("            <manifest>\n");
        push_element(out, 14, "mainClass", main);
        out.push_str("              <addClasspath>true</addClasspath>\n");
        out.push_str("              <classpathPrefix>lib/</classpathPrefix>\n");
        out.push_str("            </manifest>\n");
        out.push_str("          </archive>\n");
        out.push_str("        </configuration>\n");
    }
    if junit5 {
        out.push_str("        <configuration>\n");
        out.push_str("          <useModulePath>false</useModulePath>\n");
        out.push_str("        </configuration>\n");
    }
    out.push_str("      </plugin>\n");
}

/// Copy runtime dependencies below `target/lib` so the jar manifest's
/// `Class-Path` entries work with `java -jar`, matching jx's thin layout.
fn push_dependency_copy_plugin(out: &mut String) {
    out.push_str("      <plugin>\n");
    push_element(out, 8, "artifactId", "maven-dependency-plugin");
    push_element(out, 8, "version", "3.8.1");
    out.push_str("        <executions>\n");
    out.push_str("          <execution>\n");
    push_element(out, 12, "id", "copy-runtime-dependencies");
    push_element(out, 12, "phase", "package");
    out.push_str("            <goals>\n");
    push_element(out, 14, "goal", "copy-dependencies");
    out.push_str("            </goals>\n");
    out.push_str("            <configuration>\n");
    push_element(out, 14, "outputDirectory", "${project.build.directory}/lib");
    push_element(out, 14, "includeScope", "runtime");
    out.push_str("            </configuration>\n");
    out.push_str("          </execution>\n");
    out.push_str("        </executions>\n");
    out.push_str("      </plugin>\n");
}

fn push_element(out: &mut String, indent: usize, name: &str, value: &str) {
    let _ = writeln!(
        out,
        "{}<{name}>{}</{name}>",
        " ".repeat(indent),
        xml_escape(value)
    );
}

fn validate_ident(what: &str, value: &str) -> Result<(), JxError> {
    let safe = !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    if safe {
        Ok(())
    } else {
        Err(JxError::new(
            Kind::Manifest,
            format!("{what} {value:?} is not a valid Maven identifier"),
        )
        .hint("use letters, digits and `.`, `_`, `-`; pass --group-id to pick a groupId"))
    }
}

fn validate_value(what: &str, value: &str) -> Result<(), JxError> {
    if value.is_empty() || value.chars().any(|c| c.is_control()) {
        return Err(JxError::new(
            Kind::Manifest,
            format!("{what} {value:?} cannot be written to a POM"),
        )
        .hint("remove control characters from project.toml"));
    }
    Ok(())
}

fn xml_escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> MavenProject {
        MavenProject {
            group_id: "com.example".into(),
            artifact_id: "app".into(),
            version: "0.1.0".into(),
            java_release: 25,
            main_class: Some("app.App".into()),
            dependencies: vec![
                MavenDependency::new("com.google.guava", "guava", "33.4.8-jre"),
                MavenDependency::new("org.junit.jupiter", "junit-jupiter", "5.11.4").test_scope(),
                MavenDependency::new("org.example", "native", "1.0").classifier("windows-x86_64"),
            ],
            overrides: vec![MavenDependency::new("org.slf4j", "slf4j-api", "2.0.16")],
        }
    }

    #[test]
    fn application_pom_carries_coordinates_scopes_and_main_class() {
        let text = pom(&app()).unwrap();
        assert!(text.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>"));
        assert!(text.contains("<groupId>com.example</groupId>"));
        assert!(text.contains("<artifactId>app</artifactId>"));
        assert!(text.contains("<version>0.1.0</version>"));
        assert!(text.contains("<maven.compiler.release>25</maven.compiler.release>"));
        assert!(text.contains("<scope>test</scope>"));
        assert!(text.contains("<classifier>windows-x86_64</classifier>"));
        assert!(text.contains("<mainClass>app.App</mainClass>"));
        assert!(text.contains("maven-jar-plugin"));
        assert!(text.contains("<addClasspath>true</addClasspath>"));
        assert!(text.contains("<classpathPrefix>lib/</classpathPrefix>"));
        assert!(text.contains("maven-dependency-plugin"));
        assert!(text.contains("<includeScope>runtime</includeScope>"));
        assert!(text.contains("maven-surefire-plugin"));
        assert!(text.contains("<dependencyManagement>"));
        assert!(text.contains("<version>2.0.16</version>"));
        // The managed override is not also emitted as a normal dependency.
        assert_eq!(
            text.matches("<artifactId>slf4j-api</artifactId>").count(),
            1
        );
        assert_eq!(text.matches("<artifactId>guava</artifactId>").count(), 1);
    }

    #[test]
    fn library_pom_has_no_main_class_or_jar_plugin() {
        let mut project = app();
        project.main_class = None;
        project.dependencies.clear();
        project.overrides.clear();
        let text = pom(&project).unwrap();
        assert!(!text.contains("<mainClass>"));
        assert!(!text.contains("maven-jar-plugin"));
        assert!(!text.contains("maven-dependency-plugin"));
        assert!(!text.contains("<dependencies>"));
        assert!(!text.contains("maven-surefire-plugin"));
    }

    #[test]
    fn values_are_xml_escaped_and_controls_rejected() {
        let mut project = app();
        project.version = "1.0<&>".into();
        let text = pom(&project).unwrap();
        assert!(text.contains("<version>1.0&lt;&amp;&gt;</version>"));
        project.version = "1.0\n2.0".into();
        assert!(pom(&project).is_err());
    }

    #[test]
    fn invalid_identifiers_are_rejected() {
        let mut project = app();
        project.group_id = "com/example".into();
        assert!(pom(&project).is_err());
        project.group_id = "com.example".into();
        project.artifact_id = String::new();
        assert!(pom(&project).is_err());
    }
}
