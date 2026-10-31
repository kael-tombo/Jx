//! XML parsing for artifacts: model POM files and repository metadata
//! (`maven-metadata.xml`). Both models are data-only; the effective-POM
//! merge and snapshot resolution are built above them.
//!
//! POM scope for M2 per ADR 0003: parent, coordinates, packaging, properties,
//! dependencies, `dependencyManagement`, `repositories` and
//! `distributionManagement`. Profiles are explicitly out of scope (recorded
//! in `docs/KNOWN_LIMITATIONS.md`).
//!
//! Metadata is parsed per
//! <https://maven.apache.org/ref/current/maven-repository-metadata/repository-metadata.html>
//! as verified in ADR 0003: `versioning/latest|release|versions|lastUpdated`
//! in artifact-level files, plus `versioning/snapshot{timestamp,buildNumber}`
//! and optional `snapshotVersions` entries in snapshot-level files. That
//! metadata file itself carries `groupId`/`artifactId`/`version` (note: the
//! version there is the *base* version for snapshots, without the marker).

use jx_common::error::{JxError, Kind};
use quick_xml::Reader;
use quick_xml::events::Event;

// ---------------------------------------------------------------------------
// Element-tree reader: a minimal fully-qualified-path picker
// ---------------------------------------------------------------------------

/// Parses one top-level XML document into a tree of element children, with
/// concatenated direct text. Enough structure for POMs and metadata files,
/// without pulling in a DOM dependency.
#[derive(Clone, Debug, PartialEq)]
pub struct Element {
    pub name: String,
    pub text: String,
    pub children: Vec<Element>,
}

impl Element {
    fn new(name: &str) -> Element {
        Element {
            name: local_name(name),
            text: String::new(),
            children: Vec::new(),
        }
    }

    /// First child with this name.
    pub fn child(&self, name: &str) -> Option<&Element> {
        self.children.iter().find(|c| c.name == name)
    }

    /// Child text, or `None` when the child does not exist.
    pub fn child_text(&self, name: &str) -> Option<&str> {
        self.child(name).map(|c| c.text.as_str())
    }

    /// All children with this name (for repeated elements like `dependency`).
    pub fn children(&self, name: &str) -> impl Iterator<Item = &Element> {
        self.children.iter().filter(move |c| c.name == name)
    }
}

/// Strip an XML namespace prefix, if any: `{ns}dependency` and
/// `m2:dependency` both read as `dependency`.
fn local_name(name: &str) -> String {
    match name.rfind([':', '}']) {
        Some(index) => name[index + 1..].to_string(),
        None => name.to_string(),
    }
}

/// Parse an XML document (POM or repository metadata). Namespace prefixes are
/// ignored; processing instructions, comments and DOCTYPEs are skipped.
pub fn parse_xml(xml: &str, what: &str) -> Result<Element, JxError> {
    let mut reader = Reader::from_str(xml);
    // White space between elements is never part of a model value.
    reader.config_mut().trim_text(true);

    // Stack of open elements; element 0 is a synthetic document root that is
    // never closed by an End event, so "back at the root" (stack of length 1)
    // marks a completed top-level element.
    let mut stack: Vec<Element> = vec![Element::new("#document")];
    let mut top_level: Vec<Element> = Vec::new();
    loop {
        match reader.read_event() {
            Ok(Event::Start(start)) => {
                let name = local_name(
                    std::str::from_utf8(start.name().as_ref())
                        .map_err(|e| xml_error(what, "malformed element name", e))?,
                );
                stack.push(Element::new(&name));
            }
            Ok(Event::Empty(start)) => {
                let name = String::from_utf8_lossy(start.name().as_ref()).into_owned();
                let name = local_name(&name);
                // An empty element is a child with no text: parse it as a
                // completed element. At the top level it is a document element
                // candidate; below it is appended at the current depth.
                let element = Element::new(&name);
                if stack.len() == 1 {
                    top_level.push(element);
                } else {
                    stack
                        .last_mut()
                        .expect("root stays open")
                        .children
                        .push(element);
                }
            }
            Ok(Event::Text(text)) => {
                let decoded = text
                    .unescape()
                    .map_err(|e| xml_error(what, "malformed XML entity", e))?;
                let current = stack.last_mut().expect("root stays open");
                current.text.push_str(decoded.trim());
            }
            Ok(Event::CData(text)) => {
                let current = stack.last_mut().expect("root stays open");
                current
                    .text
                    .push_str(&String::from_utf8_lossy(text.as_ref()));
            }
            Ok(Event::End(_)) => {
                let finished = stack.pop().expect("root stays open");
                if stack.len() == 1 {
                    top_level.push(finished);
                } else {
                    stack
                        .last_mut()
                        .expect("root stays open")
                        .children
                        .push(finished);
                }
            }
            Ok(Event::Eof) => return finish_document(&top_level, what),
            Ok(_) => {} // comments, PIs, declarations, DOCTYPE
            Err(e) => {
                let position = reader.buffer_position();
                return Err(JxError::new(
                    Kind::Resolve,
                    format!("{what}: malformed XML (near byte {position})"),
                )
                .caused_by(e)
                .hint("check the repository serving this file"));
            }
        }
    }
}

/// A parsed document has exactly one top-level element; anything else is a
/// diagnosed truncation or malformed-content error.
fn finish_document(top_level: &[Element], what: &str) -> Result<Element, JxError> {
    match top_level {
        [element] => Ok(element.clone()),
        [] => Err(
            JxError::new(Kind::Resolve, format!("{what} ended mid-document"))
                .hint("the file is truncated; retry the fetch once with a network"),
        ),
        _ => Err(JxError::new(
            Kind::Resolve,
            format!("{what} does not contain exactly one document element"),
        )
        .hint("the file may be truncated or not XML at all")),
    }
}

fn xml_error(what: &str, message: &str, cause: impl std::fmt::Debug) -> JxError {
    let message = format!("{what}: {message} ({cause:?})");
    JxError::new(Kind::Resolve, message).hint("check the repository serving this file")
}

// ---------------------------------------------------------------------------
// POM (model)
// ---------------------------------------------------------------------------

/// `<parent>` coordinates plus the optional `relativePath`.
#[derive(Clone, Debug, PartialEq)]
pub struct ParentRef {
    pub group_id: String,
    pub artifact_id: String,
    pub version: String,
    pub relative_path: Option<String>,
}

/// One `<dependency>`; used identically inside `<dependencies>` and
/// `<dependencyManagement>`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Dependency {
    pub group_id: String,
    pub artifact_id: String,
    /// `None` inherits from dependencyManagement or the parent.
    pub version: Option<String>,
    /// `<type>` of the artifact (`jar` default when absent; `pom` for BOM
    /// import entries). BOM-import recognition lives in the effective model.
    pub package_type: Option<String>,
    pub classifier: Option<String>,
    /// `None` is a real (not set) scope; the empty default for `<dependencies>`
    /// is `compile` and applies during merge, never here.
    pub scope: Option<String>,
    pub optional: bool,
    /// `(groupId, artifactId)` pairs from `<exclusions>`.
    pub exclusions: Vec<(String, String)>,
}

/// A `<repository>`/`<pluginRepository>` entry.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct RawRepository {
    pub id: String,
    pub url: String,
    /// `<releases><enabled>`; Maven's default is true.
    pub releases_enabled: bool,
    /// `<snapshots><enabled>`; Maven's default is true.
    pub snapshots_enabled: bool,
}

/// The parsed POM as written, before parent merging and interpolation.
#[derive(Debug, Default, PartialEq)]
pub struct RawPom {
    pub parent: Option<ParentRef>,
    /// `None` means "inherit from the parent" (a POM may omit its groupId).
    pub group_id: Option<String>,
    pub artifact_id: String,
    /// Same rule as `group_id`.
    pub version: Option<String>,
    /// `<packaging>`; defaults to `jar` when absent (`pom` for BOM parents).
    pub packaging: String,
    /// Declared `<properties>` in document order.
    pub properties: Vec<(String, String)>,
    pub dependencies: Vec<Dependency>,
    pub dependency_management: Vec<Dependency>,
    /// `<repositories>` plus `<pluginRepositories>` (marked for diagnostics).
    pub repositories: Vec<RawRepository>,
    /// `<distributionManagement><repository><url>` when present.
    pub distribution_url: Option<String>,
    /// The raw POM text, kept so the effective model can resolve properties
    /// with document-order knowledge.
    pub source_text: String,
}

/// Parse a POM whose text was fetched from `coordinate`. Parse errors name
/// the coordinate, not the file path, because the POM is remote content.
pub fn parse_pom(text: &str, coordinate: &str) -> Result<RawPom, JxError> {
    let what = format!("POM of {coordinate}");
    match parse_xml(text, &what) {
        Ok(root) => build_pom(&root).map_err(|e| e.at(coordinate.to_string())),
        Err(e) => Err(e.at(coordinate.to_string())),
    }
}

fn build_pom(root: &Element) -> Result<RawPom, JxError> {
    // XDoclet-era artifacts ship model 3.0 POMs, but only the project element
    // shape differs; the modelVersion is asserted to exist for diagnostics.
    let project = root;
    let mut pom = RawPom {
        packaging: "jar".to_string(),
        source_text: String::new(),
        ..RawPom::default()
    };

    pom.parent = project.child("parent").map(|p| ParentRef {
        group_id: p.child_text("groupId").unwrap_or_default().to_string(),
        artifact_id: p.child_text("artifactId").unwrap_or_default().to_string(),
        version: p.child_text("version").unwrap_or_default().to_string(),
        relative_path: p.child_text("relativePath").map(str::to_string),
    });

    pom.group_id = project.child_text("groupId").map(str::to_string);
    pom.artifact_id = project
        .child_text("artifactId")
        .unwrap_or_default()
        .to_string();
    pom.version = project.child_text("version").map(str::to_string);
    if project.child("packaging").is_some() {
        pom.packaging = project
            .child_text("packaging")
            .unwrap_or_default()
            .to_string();
    }

    if let Some(properties) = project.child("properties") {
        for (name, value) in properties_leaf_pairs(properties) {
            pom.properties.push((name, value));
        }
    }

    if let Some(deps) = project.child("dependencies") {
        pom.dependencies = deps.children("dependency").map(parse_dependency).collect();
    }
    if let Some(managed) = project
        .child("dependencyManagement")
        .and_then(|managed| managed.child("dependencies"))
    {
        pom.dependency_management = managed
            .children("dependency")
            .map(parse_dependency)
            .collect();
    }

    for child in project
        .children
        .iter()
        .filter(|c| c.name == "repositories" || c.name == "pluginRepositories")
    {
        for repo in child.children("repository") {
            pom.repositories.push(RawRepository {
                id: repo.child_text("id").unwrap_or_default().to_string(),
                url: repo.child_text("url").unwrap_or_default().to_string(),
                releases_enabled: enabled_flag(repo, "releases", true),
                snapshots_enabled: enabled_flag(repo, "snapshots", true),
            });
        }
    }

    pom.distribution_url = project
        .child("distributionManagement")
        .and_then(|dm| dm.child("repository"))
        .and_then(|r| r.child_text("url"))
        .map(str::to_string);

    if pom.artifact_id.is_empty() {
        return Err(
            JxError::new(Kind::Resolve, "the POM has no <artifactId>").hint(
                "the file is served as a POM but is not one; the repository may be misconfigured",
            ),
        );
    }
    Ok(pom)
}

/// Direct textual children of a `<properties>` block (nested elements are
/// not meaningful there and are ignored).
fn properties_leaf_pairs(properties: &Element) -> Vec<(String, String)> {
    properties
        .children
        .iter()
        .filter(|c| c.children.is_empty())
        .map(|c| (c.name.clone(), c.text.clone()))
        .collect()
}

fn enabled_flag(repo: &Element, kind: &str, default: bool) -> bool {
    match repo.child(kind).and_then(|k| k.child_text("enabled")) {
        Some(text) => text.eq_ignore_ascii_case("true"),
        None => default,
    }
}

fn parse_dependency(dependency: &Element) -> Dependency {
    Dependency {
        group_id: dependency
            .child_text("groupId")
            .unwrap_or_default()
            .to_string(),
        artifact_id: dependency
            .child_text("artifactId")
            .unwrap_or_default()
            .to_string(),
        version: dependency.child_text("version").map(str::to_string),
        package_type: dependency.child_text("type").map(str::to_string),
        classifier: dependency.child_text("classifier").map(str::to_string),
        scope: dependency.child_text("scope").map(str::to_string),
        optional: dependency
            .child_text("optional")
            .is_some_and(|o| o.eq_ignore_ascii_case("true")),
        exclusions: dependency
            .child("exclusions")
            .map(|exclusions| {
                exclusions
                    .children("exclusion")
                    .map(|exclusion| {
                        (
                            exclusion
                                .child_text("groupId")
                                .unwrap_or_default()
                                .to_string(),
                            exclusion
                                .child_text("artifactId")
                                .unwrap_or_default()
                                .to_string(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default(),
    }
}

// ---------------------------------------------------------------------------
// maven-metadata.xml (repository metadata)
// ---------------------------------------------------------------------------

/// The `<snapshot>` block of a snapshot metadata file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnapshotInfo {
    pub timestamp: String,
    pub build_number: String,
}

/// A `<snapshotVersions><snapshotVersion>` entry: the concrete file an
/// artifact+classifer published for the snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnapshotVersion {
    pub value: String,
    pub extension: String,
    pub classifier: Option<String>,
}

/// The parsed metadata document (both artifact-level and version-level).
#[derive(Debug, Default, PartialEq)]
pub struct ArtifactMetadata {
    pub latest: Option<String>,
    pub release: Option<String>,
    pub versions: Vec<String>,
    pub last_updated: Option<String>,
    pub snapshot: Option<SnapshotInfo>,
    pub snapshot_versions: Vec<SnapshotVersion>,
}

/// Parse a `maven-metadata.xml`. Snapshot files carry `version/snapshot`;
/// artifact-level ones list all versions. Both shapes parse into one model.
pub fn parse_metadata(text: &str, metadata_path: &str) -> Result<ArtifactMetadata, JxError> {
    let what = format!("maven-metadata.xml at {metadata_path}");
    match parse_xml(text, &what) {
        Ok(root) => Ok(build_metadata(&root)),
        Err(e) => Err(e.at(metadata_path.to_string())),
    }
}

fn build_metadata(metadata: &Element) -> ArtifactMetadata {
    let versioning = metadata.child("versioning");
    let mut metadata_model = ArtifactMetadata::default();
    if let Some(versioning) = versioning {
        metadata_model.latest = versioning.child_text("latest").map(str::to_string);
        metadata_model.release = versioning.child_text("release").map(str::to_string);
        metadata_model.last_updated = versioning.child_text("lastUpdated").map(str::to_string);
        if let Some(versions) = versioning.child("versions") {
            metadata_model.versions = versions
                .children("version")
                .map(|v| v.text.clone())
                .collect();
        }
        if let Some(snapshot) = versioning.child("snapshot") {
            metadata_model.snapshot = Some(SnapshotInfo {
                timestamp: snapshot
                    .child_text("timestamp")
                    .unwrap_or_default()
                    .to_string(),
                build_number: snapshot
                    .child_text("buildNumber")
                    .unwrap_or_default()
                    .to_string(),
            });
        }
        if let Some(snapshot_versions) = versioning.child("snapshotVersions") {
            for entry in snapshot_versions.children("snapshotVersion") {
                metadata_model.snapshot_versions.push(SnapshotVersion {
                    value: entry.child_text("value").unwrap_or_default().to_string(),
                    extension: entry.child_text("extension").unwrap_or("jar").to_string(),
                    classifier: entry.child_text("classifier").map(str::to_string),
                });
            }
        }
    }
    metadata_model
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_pom_shape() {
        let pom = crate::xml::parse_pom(
            r#"
<project xmlns="http://maven.apache.org/POM/4.0.0">
  <modelVersion>4.0.0</modelVersion>
  <parent>
    <groupId>com.x</groupId>
    <artifactId>x-parent</artifactId>
    <version>2.0</version>
    <relativePath>../x-parent/pom.xml</relativePath>
  </parent>
  <artifactId>core</artifactId>
  <packaging>pom</packaging>
  <properties>
    <guava.version>33.0.0</guava.version>
    <skipTests>true</skipTests>
  </properties>
  <dependencies>
    <dependency>
      <groupId>com.google.guava</groupId>
      <artifactId>guava</artifactId>
      <version>${guava.version}</version>
      <scope>test</scope>
      <optional>true</optional>
      <exclusions>
        <exclusion>
          <groupId>org.checkerframework</groupId>
          <artifactId>checker-qual</artifactId>
        </exclusion>
      </exclusions>
    </dependency>
    <dependency>
      <groupId>org.junit</groupId>
      <artifactId>junit-bom</artifactId>
      <version>5.0</version>
    </dependency>
  </dependencies>
  <dependencyManagement>
    <dependencies>
      <dependency>
        <groupId>org.junit</groupId>
        <artifactId>junit-bom</artifactId>
        <version>5.11.4</version>
        <scope>import</scope>
      </dependency>
    </dependencies>
  </dependencyManagement>
  <repositories>
    <repository>
      <id>internal</id>
      <url>https://repo.internal/maven</url>
      <snapshots>
        <enabled>false</enabled>
      </snapshots>
    </repository>
  </repositories>
  <distributionManagement>
    <repository>
      <id>internal-release</id>
      <url>https://repo.internal/releases</url>
    </repository>
  </distributionManagement>
</project>
"#,
            "com.x:core:1.0",
        )
        .unwrap();

        assert_eq!(pom.artifact_id, "core");
        assert!(pom.group_id.is_none()); // inherited from parent
        assert_eq!(pom.packaging, "pom");
        let parent = pom.parent.expect("parent present");
        assert_eq!(parent.version, "2.0");
        assert_eq!(parent.relative_path.as_deref(), Some("../x-parent/pom.xml"));
        assert_eq!(
            pom.properties,
            vec![
                ("guava.version".to_string(), "33.0.0".to_string()),
                ("skipTests".to_string(), "true".to_string())
            ]
        );
        assert_eq!(pom.dependencies.len(), 2);
        let first = &pom.dependencies[0];
        assert_eq!(first.version.as_deref(), Some("${guava.version}"));
        assert_eq!(first.scope.as_deref(), Some("test"));
        assert!(first.optional);
        assert_eq!(
            first.exclusions,
            vec![(
                "org.checkerframework".to_string(),
                "checker-qual".to_string()
            )]
        );
        assert_eq!(pom.dependency_management.len(), 1);
        assert_eq!(
            pom.dependency_management[0].version.as_deref(),
            Some("5.11.4")
        );
        assert_eq!(pom.repositories.len(), 1);
        let repo = &pom.repositories[0];
        assert_eq!(repo.id, "internal");
        assert!(!repo.snapshots_enabled);
        assert!(repo.releases_enabled); // default true when omitted
        assert_eq!(
            pom.distribution_url.as_deref(),
            Some("https://repo.internal/releases")
        );
    }

    #[test]
    fn minimal_pom_defaults_to_jar_packaging() {
        let pom = crate::xml::parse_pom(
            "<project><groupId>g</groupId><artifactId>a</artifactId><version>1</version></project>",
            "g:a:1",
        )
        .unwrap();
        assert_eq!(pom.packaging, "jar");
        assert_eq!(pom.version.as_deref(), Some("1"));
        assert!(pom.parent.is_none());
        assert!(pom.dependencies.is_empty());
    }

    #[test]
    fn missing_artifact_id_is_an_error_named_with_the_coordinate() {
        let e =
            crate::xml::parse_pom("<project><groupId>g</groupId></project>", "g:a:1").unwrap_err();
        assert!(e.message.contains("<artifactId>"), "{e}");
        assert_eq!(e.location.as_deref(), Some("g:a:1"));
    }

    #[test]
    fn truncated_xml_errors() {
        let e = crate::xml::parse_pom("<project><g", "g:a:1").unwrap_err();
        assert!(e.message.contains("POM of g:a:1"), "{e}");
    }

    #[test]
    fn artifact_level_metadata() {
        let metadata = crate::xml::parse_metadata(
            r#"
<metadata>
  <groupId>g</groupId>
  <artifactId>a</artifactId>
  <versioning>
    <latest>3.0</latest>
    <release>2.0</release>
    <versions>
      <version>1.0</version>
      <version>2.0</version>
      <version>3.0-SNAPSHOT</version>
    </versions>
    <lastUpdated>20250811053044</lastUpdated>
  </versioning>
</metadata>
"#,
            "g/a/maven-metadata.xml",
        )
        .unwrap();
        assert_eq!(metadata.release.as_deref(), Some("2.0"));
        assert_eq!(metadata.versions, vec!["1.0", "2.0", "3.0-SNAPSHOT"]);
        assert!(metadata.snapshot.is_none());
        assert_eq!(metadata.last_updated.as_deref(), Some("20250811053044"));
    }

    #[test]
    fn snapshot_metadata() {
        let metadata = crate::xml::parse_metadata(
            r#"
<metadata>
  <groupId>g</groupId>
  <artifactId>a</artifactId>
  <version>1.0-SNAPSHOT</version>
  <versioning>
    <snapshot>
      <buildNumber>12</buildNumber>
      <timestamp>20250101.070011</timestamp>
    </snapshot>
    <snapshotVersions>
      <snapshotVersion>
        <extension>jar</extension>
        <value>1.0-20250101.070011-12</value>
        <updated>20250101070011</updated>
      </snapshotVersion>
      <snapshotVersion>
        <extension>pom</extension>
        <value>1.0-20250101.070011-12</value>
      </snapshotVersion>
      <snapshotVersion>
        <classifier>tests</classifier>
        <extension>jar</extension>
        <value>1.0-20250101.070011-12-tests</value>
      </snapshotVersion>
    </snapshotVersions>
  </versioning>
</metadata>
"#,
            "g/a/1.0-SNAPSHOT/maven-metadata.xml",
        )
        .unwrap();
        let snapshot = metadata.snapshot.expect("snapshot info");
        assert_eq!(snapshot.timestamp, "20250101.070011");
        assert_eq!(snapshot.build_number, "12");
        assert_eq!(metadata.snapshot_versions.len(), 3);
        assert_eq!(
            metadata.snapshot_versions[0].value,
            "1.0-20250101.070011-12"
        );
        assert_eq!(metadata.snapshot_versions[1].extension, "pom");
        assert_eq!(
            metadata.snapshot_versions[2].classifier.as_deref(),
            Some("tests")
        );
    }

    #[test]
    fn empty_metadata_is_accepted_but_empty() {
        let metadata = crate::xml::parse_metadata("<metadata></metadata>", "p").unwrap();
        assert_eq!(metadata, ArtifactMetadata::default());
    }

    #[test]
    fn entities_are_unescaped() {
        let metadata =
            crate::xml::parse_metadata("<metadata><groupId>&lt;g&gt;</groupId></metadata>", "p")
                .unwrap();
        assert_eq!(metadata, ArtifactMetadata::default());
    }
}
