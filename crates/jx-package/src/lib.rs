//! Packaging: a portable thin `dist/` directory and an opt-in fat jar.
//!
//! Thin: the project jar contains only project classes and resources;
//! dependencies are copied below `dist/lib/<group path>/` and referenced from
//! the manifest's `Class-Path`, so the whole `dist/` tree is portable.
//! Fat: dependency classes/resources are merged into one jar with explicit
//! duplicate, service-file, signature and multi-release rules.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};

use jx_common::error::{JxError, Kind};

/// Thin jar + `dist/lib`, or one merged fat jar.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PackageMode {
    Thin,
    Fat,
}

/// One dependency to copy into `dist/lib` (thin) or merge (fat).
pub struct DependencyJar<'a> {
    /// Forward-slash relative path below the dist directory, e.g.
    /// `lib/com/google/guava/guava-33.4.8-jre.jar`. Ignored for fat jars.
    pub relative_path: &'a str,
    pub source: &'a Path,
}

/// Everything one `jx package` needs.
pub struct PackageRequest<'a> {
    pub mode: PackageMode,
    /// Destination directory; replaced on every package.
    pub dist_dir: &'a Path,
    pub classes_dir: &'a Path,
    pub jar_name: &'a str,
    pub main_class: Option<&'a str>,
    pub dependencies: &'a [DependencyJar<'a>],
}

/// Where the package landed and what the merge had to handle.
pub struct PackageOutput {
    pub jar: PathBuf,
    /// `Some` for a thin package; `None` for a fat jar.
    pub lib_dir: Option<PathBuf>,
    pub dependency_count: usize,
    /// Fat jar only: entries dropped because a project/earlier artifact won.
    pub duplicate_entries: usize,
    /// Fat jar only: `META-INF/versions/` (multi-release) entries skipped.
    pub versioned_entries: usize,
    /// Fat jar only: absolute/traversing entry names refused.
    pub unsafe_entries: usize,
}

/// Build `dist_dir` from scratch, using the requested packaging mode.
pub fn package(request: &PackageRequest<'_>) -> Result<PackageOutput, JxError> {
    validate_file_name(request.jar_name)?;
    if let Some(main) = request.main_class {
        validate_manifest_value("Main-Class", main)?;
    }
    if request.mode == PackageMode::Thin {
        for dependency in request.dependencies {
            validate_relative_path(dependency.relative_path)?;
        }
    }
    reset_dist(request.dist_dir)?;
    std::fs::create_dir_all(request.dist_dir).map_err(|e| {
        JxError::new(Kind::Io, "cannot create the dist directory")
            .at(request.dist_dir.display().to_string())
            .caused_by(e)
    })?;
    match request.mode {
        PackageMode::Thin => package_thin(request),
        PackageMode::Fat => package_fat(request),
    }
}

fn reset_dist(dist: &Path) -> Result<(), JxError> {
    if dist.exists() {
        std::fs::remove_dir_all(dist).map_err(|e| {
            JxError::new(Kind::Io, "cannot clear the previous dist directory")
                .at(dist.display().to_string())
                .caused_by(e)
                .hint("close any running program that holds files under target/dist")
        })?;
    }
    Ok(())
}

fn package_thin(request: &PackageRequest<'_>) -> Result<PackageOutput, JxError> {
    let mut class_path = Vec::with_capacity(request.dependencies.len());
    for dependency in request.dependencies {
        validate_relative_path(dependency.relative_path)?;
        class_path.push(dependency.relative_path.to_string());
    }
    let lib_dir = request.dist_dir.join("lib");
    std::fs::create_dir_all(&lib_dir).map_err(|e| {
        JxError::new(Kind::Io, "cannot create the dist library directory")
            .at(lib_dir.display().to_string())
            .caused_by(e)
    })?;
    for dependency in request.dependencies {
        let target = request.dist_dir.join(dependency.relative_path);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                JxError::new(Kind::Io, "cannot create a dist dependency directory")
                    .at(parent.display().to_string())
                    .caused_by(e)
            })?;
        }
        std::fs::copy(dependency.source, &target).map_err(|e| {
            JxError::new(Kind::Io, "cannot copy a dependency into dist/lib")
                .at(format!(
                    "{} -> {}",
                    dependency.source.display(),
                    target.display()
                ))
                .caused_by(e)
        })?;
    }
    let jar_path = request.dist_dir.join(request.jar_name);
    write_jar(
        &jar_path,
        request.classes_dir,
        request.main_class,
        &class_path,
    )?;
    Ok(PackageOutput {
        jar: jar_path,
        lib_dir: Some(lib_dir),
        dependency_count: request.dependencies.len(),
        duplicate_entries: 0,
        versioned_entries: 0,
        unsafe_entries: 0,
    })
}

fn package_fat(request: &PackageRequest<'_>) -> Result<PackageOutput, JxError> {
    let jar_path = request.dist_dir.join(request.jar_name);
    let mut stats = FatStats::default();
    write_fat_jar(
        &jar_path,
        request.classes_dir,
        request.main_class,
        request.dependencies,
        &mut stats,
    )?;
    Ok(PackageOutput {
        jar: jar_path,
        lib_dir: None,
        dependency_count: request.dependencies.len(),
        duplicate_entries: stats.duplicates,
        versioned_entries: stats.versioned,
        unsafe_entries: stats.unsafe_entries,
    })
}

/// The conventional project jar name, validated as one path component.
pub fn jar_file_name(name: &str, version: &str) -> Result<String, JxError> {
    validate_file_name(name)?;
    validate_file_name(version)?;
    let name = format!("{name}-{version}.jar");
    validate_file_name(&name)?;
    Ok(name)
}

/// `lib/<group path>/<file name>` for a locked artifact. The group is turned
/// into directories so two groups with the same artifact name cannot collide.
pub fn dependency_relative_path(group_id: &str, file_name: &str) -> Result<String, JxError> {
    validate_file_name(file_name)?;
    let mut path = String::from("lib");
    for segment in group_id.split('.') {
        if segment.is_empty() {
            return Err(JxError::new(
                Kind::Build,
                format!("dependency group {group_id:?} has an empty segment"),
            ));
        }
        validate_file_name(segment)?;
        path.push('/');
        path.push_str(segment);
    }
    path.push('/');
    path.push_str(file_name);
    Ok(path)
}

/// The Maven artifact file name: `artifact-version[-classifier].jar`.
pub fn artifact_file_name(
    artifact_id: &str,
    version: &str,
    classifier: Option<&str>,
) -> Result<String, JxError> {
    validate_file_name(artifact_id)?;
    validate_file_name(version)?;
    let name = match classifier {
        Some(classifier) => {
            validate_file_name(classifier)?;
            format!("{artifact_id}-{version}-{classifier}.jar")
        }
        None => format!("{artifact_id}-{version}.jar"),
    };
    validate_file_name(&name)?;
    Ok(name)
}

/// Write a jar from `classes_dir` with a deterministic entry order. A
/// `META-INF/MANIFEST.MF` in the classes directory is ignored; jx writes its
/// own. Resources are included alongside classes.
fn write_jar(
    output: &Path,
    classes_dir: &Path,
    main_class: Option<&str>,
    class_path: &[String],
) -> Result<(), JxError> {
    let manifest = manifest_text(main_class, class_path)?;
    let file = std::fs::File::create(output).map_err(|e| {
        JxError::new(Kind::Io, "cannot create the package jar")
            .at(output.display().to_string())
            .caused_by(e)
    })?;
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(0o644);
    zip.start_file("META-INF/MANIFEST.MF", options)
        .map_err(|e| zip_error(output, e))?;
    zip.write_all(manifest.as_bytes())
        .map_err(|e| io_error("cannot write the jar manifest", output, e))?;

    for source in collect_files(classes_dir)? {
        let relative = source
            .strip_prefix(classes_dir)
            .expect("walked files stay under classes_dir");
        let name = relative
            .components()
            .map(|component| component.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        if name.eq_ignore_ascii_case("META-INF/MANIFEST.MF") {
            continue;
        }
        zip.start_file(&name, options)
            .map_err(|e| zip_error(output, e))?;
        let mut input = std::fs::File::open(&source)
            .map_err(|e| io_error("cannot read a class or resource for packaging", &source, e))?;
        std::io::copy(&mut input, &mut zip)
            .map_err(|e| io_error("cannot write a jar entry", output, e))?;
    }
    zip.finish().map_err(|e| zip_error(output, e))?;
    Ok(())
}

#[derive(Default)]
struct FatStats {
    duplicates: usize,
    versioned: usize,
    unsafe_entries: usize,
}

/// Merge project entries and dependency jars into one jar. Project entries
/// win collisions; dependency entries follow lock order and the first one
/// wins. Service files are merged by provider name. Signature files,
/// `module-info.class`, `META-INF/INDEX.LIST` and multi-release entries are
/// deliberately omitted because a merged jar invalidates their assumptions.
fn write_fat_jar(
    output: &Path,
    classes_dir: &Path,
    main_class: Option<&str>,
    dependencies: &[DependencyJar<'_>],
    stats: &mut FatStats,
) -> Result<(), JxError> {
    let manifest = manifest_text(main_class, &[])?;
    let file = std::fs::File::create(output).map_err(|e| {
        JxError::new(Kind::Io, "cannot create the fat jar")
            .at(output.display().to_string())
            .caused_by(e)
    })?;
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(0o644);
    zip.start_file("META-INF/MANIFEST.MF", options)
        .map_err(|e| zip_error(output, e))?;
    zip.write_all(manifest.as_bytes())
        .map_err(|e| io_error("cannot write the fat jar manifest", output, e))?;

    let mut seen: HashSet<String> = HashSet::new();
    seen.insert("META-INF/MANIFEST.MF".to_string());
    let mut services: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();

    for source in collect_files(classes_dir)? {
        let relative = source
            .strip_prefix(classes_dir)
            .expect("walked files stay under classes_dir");
        let name = relative
            .components()
            .map(|component| component.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        if name.starts_with("META-INF/versions/") {
            stats.versioned += 1;
            continue;
        }
        if skip_embedded(&name) {
            continue;
        }
        if let Some(service) = service_name(&name) {
            let text = std::fs::read_to_string(&source)
                .map_err(|e| io_error("cannot read a service descriptor", &source, e))?;
            add_providers(service, &text, &mut services);
            continue;
        }
        if !safe_entry_name(&name) {
            stats.unsafe_entries += 1;
            continue;
        }
        if !seen.insert(name.clone()) {
            stats.duplicates += 1;
            continue;
        }
        zip.start_file(&name, options)
            .map_err(|e| zip_error(output, e))?;
        let mut input = std::fs::File::open(&source)
            .map_err(|e| io_error("cannot read a class for the fat jar", &source, e))?;
        std::io::copy(&mut input, &mut zip)
            .map_err(|e| io_error("cannot write a fat jar entry", output, e))?;
    }

    for dependency in dependencies {
        let file = std::fs::File::open(dependency.source).map_err(|e| {
            io_error(
                "cannot open a dependency jar for the fat jar",
                dependency.source,
                e,
            )
        })?;
        let mut archive = zip::ZipArchive::new(std::io::BufReader::new(file))
            .map_err(|e| zip_read_error(dependency.source, e))?;
        for index in 0..archive.len() {
            let mut entry = archive
                .by_index(index)
                .map_err(|e| zip_read_error(dependency.source, e))?;
            if entry.is_dir() {
                continue;
            }
            let name = entry.name().to_string();
            if name.starts_with("META-INF/versions/") {
                stats.versioned += 1;
                continue;
            }
            if skip_embedded(&name) {
                continue;
            }
            if let Some(service) = service_name(&name) {
                let mut text = String::new();
                entry.read_to_string(&mut text).map_err(|e| {
                    io_error(
                        "cannot read a service descriptor from a dependency",
                        dependency.source,
                        e,
                    )
                })?;
                add_providers(service, &text, &mut services);
                continue;
            }
            if !safe_entry_name(&name) {
                stats.unsafe_entries += 1;
                continue;
            }
            if !seen.insert(name.clone()) {
                stats.duplicates += 1;
                continue;
            }
            zip.start_file(&name, options)
                .map_err(|e| zip_error(output, e))?;
            std::io::copy(&mut entry, &mut zip)
                .map_err(|e| io_error("cannot write a fat jar entry", output, e))?;
        }
    }

    for (service, providers) in services {
        let name = format!("META-INF/services/{service}");
        zip.start_file(&name, options)
            .map_err(|e| zip_error(output, e))?;
        for provider in providers {
            zip.write_all(provider.as_bytes())
                .map_err(|e| io_error("cannot write a merged service descriptor", output, e))?;
            zip.write_all(b"\n")
                .map_err(|e| io_error("cannot write a merged service descriptor", output, e))?;
        }
    }
    zip.finish().map_err(|e| zip_error(output, e))?;
    Ok(())
}

fn skip_embedded(name: &str) -> bool {
    name.eq_ignore_ascii_case("META-INF/MANIFEST.MF")
        || name.eq_ignore_ascii_case("module-info.class")
        || name.eq_ignore_ascii_case("META-INF/INDEX.LIST")
        || is_signature(name)
}

fn is_signature(name: &str) -> bool {
    if !name.to_ascii_uppercase().starts_with("META-INF/") {
        return false;
    }
    let upper = name.to_ascii_uppercase();
    [".SF", ".DSA", ".RSA", ".EC"]
        .iter()
        .any(|extension| upper.ends_with(extension))
}

fn service_name(name: &str) -> Option<&str> {
    name.strip_prefix("META-INF/services/")
        .filter(|service| !service.is_empty() && !service.contains('/'))
}

fn add_providers(service: &str, text: &str, services: &mut BTreeMap<String, BTreeSet<String>>) {
    let providers = services.entry(service.to_string()).or_default();
    for line in text.lines() {
        let provider = line.split('#').next().unwrap_or("").trim();
        if !provider.is_empty() {
            providers.insert(provider.to_string());
        }
    }
}

fn safe_entry_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('/')
        && !name.contains('\\')
        && !name.contains('\0')
        && name
            .split('/')
            .all(|segment| !segment.is_empty() && segment != "." && segment != "..")
}

/// The manifest text, with header lines wrapped exactly as the jar format
/// expects (no line over 72 bytes; continuations start with one space).
fn manifest_text(main_class: Option<&str>, class_path: &[String]) -> Result<String, JxError> {
    let mut out = String::new();
    push_header(&mut out, "Manifest-Version", "1.0");
    push_header(
        &mut out,
        "Created-By",
        concat!("jx ", env!("CARGO_PKG_VERSION")),
    );
    if let Some(main) = main_class {
        validate_manifest_value("Main-Class", main)?;
        if main.chars().any(char::is_whitespace) {
            return Err(JxError::new(
                Kind::Build,
                "Main-Class must be a class name without whitespace",
            )
            .at(main.to_string())
            .hint("fix `[run] main` in project.toml"));
        }
        push_header(&mut out, "Main-Class", main);
    }
    if !class_path.is_empty() {
        for entry in class_path {
            validate_manifest_value("Class-Path", entry)?;
            if entry.chars().any(char::is_whitespace) {
                return Err(JxError::new(
                    Kind::Build,
                    format!("dependency path {entry:?} contains whitespace"),
                )
                .hint("dependency file names must be safe path components; rename the artifact"));
            }
        }
        push_header(&mut out, "Class-Path", &class_path.join(" "));
    }
    out.push_str("\r\n");
    Ok(out)
}

fn push_header(out: &mut String, key: &str, value: &str) {
    let line = format!("{key}: {value}");
    let mut start = 0;
    let mut first = true;
    while start < line.len() {
        let limit = if first { 72 } else { 71 };
        let mut end = (start + limit).min(line.len());
        while !line.is_char_boundary(end) {
            end -= 1;
        }
        if !first {
            out.push(' ');
        }
        out.push_str(&line[start..end]);
        out.push_str("\r\n");
        start = end;
        first = false;
    }
}

fn validate_manifest_value(header: &str, value: &str) -> Result<(), JxError> {
    if value.is_empty()
        || value
            .chars()
            .any(|c| c.is_control() || matches!(c, '\r' | '\n'))
    {
        return Err(JxError::new(
            Kind::Build,
            format!("{header} cannot be written to the jar manifest"),
        )
        .at(value.to_string())
        .hint("remove line breaks and control characters from the manifest value"));
    }
    Ok(())
}

fn validate_file_name(value: &str) -> Result<(), JxError> {
    let safe = !value.is_empty()
        && value != "."
        && value != ".."
        && !value.ends_with(' ')
        && !value.ends_with('.')
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '+'));
    if !safe {
        return Err(JxError::new(
            Kind::Build,
            format!("{value:?} cannot be used as a file name under dist/"),
        )
        .hint(
            "dist file names allow ASCII letters, digits, '.', '_', '-' and '+'; \
             change the project or artifact version",
        ));
    }
    Ok(())
}

fn validate_relative_path(value: &str) -> Result<(), JxError> {
    if !value.starts_with("lib/") || value.starts_with('/') || value.contains('\\') {
        return Err(JxError::new(
            Kind::Build,
            format!("dependency path {value:?} must be a relative path below dist/lib"),
        ));
    }
    for segment in value.split('/') {
        if segment.is_empty() || segment == "." || segment == ".." {
            return Err(JxError::new(
                Kind::Build,
                format!("dependency path {value:?} has an unsafe segment"),
            ));
        }
        validate_file_name(segment)?;
    }
    Ok(())
}

fn collect_files(root: &Path) -> Result<Vec<PathBuf>, JxError> {
    if !root.is_dir() {
        return Err(JxError::new(
            Kind::Build,
            format!("no compiled classes under {}", root.display()),
        )
        .hint("run `jx build` first"));
    }
    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let mut entries: Vec<_> = std::fs::read_dir(&dir)
            .map_err(|e| {
                JxError::new(Kind::Io, "cannot list compiled classes for packaging")
                    .at(dir.display().to_string())
                    .caused_by(e)
            })?
            .filter_map(|entry| entry.ok())
            .collect();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            let file_type = match entry.file_type() {
                Ok(file_type) => file_type,
                Err(_) => continue,
            };
            if file_type.is_dir() {
                stack.push(path);
            } else if file_type.is_file() {
                files.push(path);
            }
        }
    }
    files.sort();
    Ok(files)
}

fn zip_error(output: &Path, error: zip::result::ZipError) -> JxError {
    JxError::new(Kind::Build, "cannot write the package jar")
        .at(output.display().to_string())
        .caused_by(error)
}

fn zip_read_error(source: &Path, error: zip::result::ZipError) -> JxError {
    JxError::new(Kind::Build, "cannot read a dependency jar for the fat jar")
        .at(source.display().to_string())
        .caused_by(error)
        .hint("run `jx fetch` again; the cached jar may be corrupt")
}

fn io_error(context: &str, path: &Path, error: std::io::Error) -> JxError {
    JxError::new(Kind::Io, context)
        .at(path.display().to_string())
        .caused_by(error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn write(path: &Path, bytes: &[u8]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    fn make_jar(path: &Path, entries: &[(&str, &[u8])]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let file = std::fs::File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for (name, bytes) in entries {
            zip.start_file(*name, options).unwrap();
            zip.write_all(bytes).unwrap();
        }
        zip.finish().unwrap();
    }

    fn read_entry(path: &Path, name: &str) -> Vec<u8> {
        let file = std::fs::File::open(path).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();
        let mut bytes = Vec::new();
        archive
            .by_name(name)
            .unwrap()
            .read_to_end(&mut bytes)
            .unwrap();
        bytes
    }

    fn read_jar(path: &Path) -> (String, Vec<String>) {
        let file = std::fs::File::open(path).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();
        let mut manifest = String::new();
        archive
            .by_name("META-INF/MANIFEST.MF")
            .unwrap()
            .read_to_string(&mut manifest)
            .unwrap();
        let mut names: Vec<String> = (0..archive.len())
            .map(|index| archive.by_index(index).unwrap().name().to_string())
            .collect();
        names.sort();
        (manifest, names)
    }

    #[test]
    fn builds_dist_with_manifest_and_dependencies() {
        let dir = tempdir().unwrap();
        let classes = dir.path().join("classes");
        write(&classes.join("app/App.class"), b"class-bytes");
        write(&classes.join("config/app.properties"), b"x=1");
        write(&classes.join("META-INF/MANIFEST.MF"), b"stale");
        let dependency = dir.path().join("dep.jar");
        std::fs::write(&dependency, b"dep-bytes").unwrap();
        let relative = dependency_relative_path("com.example", "lib-1.0.jar").unwrap();
        assert_eq!(relative, "lib/com/example/lib-1.0.jar");
        let dist = dir.path().join("dist");
        std::fs::create_dir_all(dist.join("stale")).unwrap();
        std::fs::write(dist.join("stale/old.txt"), b"old").unwrap();

        let output = package(&PackageRequest {
            mode: PackageMode::Thin,
            dist_dir: &dist,
            classes_dir: &classes,
            jar_name: "app-0.1.0.jar",
            main_class: Some("app.App"),
            dependencies: &[DependencyJar {
                relative_path: &relative,
                source: &dependency,
            }],
        })
        .unwrap();
        assert_eq!(output.dependency_count, 1);
        assert_eq!(output.jar, dist.join("app-0.1.0.jar"));
        assert_eq!(std::fs::read(dist.join(&relative)).unwrap(), b"dep-bytes");
        assert!(!dist.join("stale").exists(), "dist starts clean");

        let (manifest, names) = read_jar(&output.jar);
        assert!(manifest.contains("Main-Class: app.App\r\n"), "{manifest}");
        assert!(
            manifest.contains("Class-Path: lib/com/example/lib-1.0.jar\r\n"),
            "{manifest}"
        );
        assert_eq!(
            names,
            vec![
                "META-INF/MANIFEST.MF",
                "app/App.class",
                "config/app.properties"
            ]
        );
        for line in manifest.lines() {
            assert!(line.len() <= 72, "manifest line too long: {line:?}");
        }
    }

    #[test]
    fn fat_jar_merges_services_and_reports_skips() {
        let dir = tempdir().unwrap();
        let classes = dir.path().join("classes");
        write(&classes.join("app/App.class"), b"project-app");
        write(
            &classes.join("META-INF/services/com.example.Spi"),
            b"app.ProjectProvider\n",
        );
        let dep1 = dir.path().join("dep1.jar");
        make_jar(
            &dep1,
            &[
                ("dep/One.class", b"one"),
                ("app/App.class", b"dep-app-loses"),
                ("META-INF/MANIFEST.MF", b"dep-manifest"),
                ("META-INF/DEP.SF", b"signature"),
                ("META-INF/versions/9/dep/One.class", b"versioned"),
                (
                    "META-INF/services/com.example.Spi",
                    b"# providers\ndep.One\n",
                ),
            ],
        );
        let dep2 = dir.path().join("dep2.jar");
        make_jar(
            &dep2,
            &[
                ("dep/Two.class", b"two"),
                ("META-INF/services/com.example.Spi", b"dep.Two\ndep.One\n"),
            ],
        );
        let dist = dir.path().join("dist");
        let output = package(&PackageRequest {
            mode: PackageMode::Fat,
            dist_dir: &dist,
            classes_dir: &classes,
            jar_name: "app-0.1.0-all.jar",
            main_class: Some("app.App"),
            dependencies: &[
                DependencyJar {
                    relative_path: "ignored/dep1.jar",
                    source: &dep1,
                },
                DependencyJar {
                    relative_path: "ignored/dep2.jar",
                    source: &dep2,
                },
            ],
        })
        .unwrap();
        assert_eq!(output.dependency_count, 2);
        assert!(output.lib_dir.is_none());
        assert_eq!(output.duplicate_entries, 1);
        assert_eq!(output.versioned_entries, 1);
        assert!(!dist.join("lib").exists());

        let (manifest, names) = read_jar(&output.jar);
        assert!(manifest.contains("Main-Class: app.App\r\n"), "{manifest}");
        assert!(!manifest.contains("Class-Path"), "{manifest}");
        assert!(!names.iter().any(|name| name.ends_with(".SF")));
        assert!(
            !names
                .iter()
                .any(|name| name.starts_with("META-INF/versions/")),
            "{names:?}"
        );
        assert_eq!(read_entry(&output.jar, "app/App.class"), b"project-app");
        assert_eq!(read_entry(&output.jar, "dep/One.class"), b"one");
        assert_eq!(read_entry(&output.jar, "dep/Two.class"), b"two");
        let services =
            String::from_utf8(read_entry(&output.jar, "META-INF/services/com.example.Spi"))
                .unwrap();
        assert_eq!(services, "app.ProjectProvider\ndep.One\ndep.Two\n");
    }

    #[test]
    fn libraries_have_no_main_class_header() {
        let dir = tempdir().unwrap();
        let classes = dir.path().join("classes");
        write(&classes.join("lib/Lib.class"), b"lib");
        let dist = dir.path().join("dist");
        let output = package(&PackageRequest {
            mode: PackageMode::Thin,
            dist_dir: &dist,
            classes_dir: &classes,
            jar_name: "lib-0.1.0.jar",
            main_class: None,
            dependencies: &[],
        })
        .unwrap();
        let (manifest, _) = read_jar(&output.jar);
        assert!(!manifest.contains("Main-Class"), "{manifest}");
        assert!(!manifest.contains("Class-Path"), "{manifest}");
    }

    #[test]
    fn long_classpath_wraps_and_round_trips() {
        let entry = format!(
            "lib/com/example/{}/artifact-1.0.jar",
            "verylongsegment".repeat(5)
        );
        let manifest = manifest_text(Some("app.App"), std::slice::from_ref(&entry)).unwrap();
        for line in manifest.split("\r\n") {
            assert!(line.len() <= 72, "line too long ({}): {line:?}", line.len());
        }
        let mut value = String::new();
        let mut in_class_path = false;
        for line in manifest.split("\r\n") {
            if let Some(rest) = line.strip_prefix("Class-Path: ") {
                value.push_str(rest);
                in_class_path = true;
            } else if in_class_path {
                match line.strip_prefix(' ') {
                    Some(rest) => value.push_str(rest),
                    None => break,
                }
            }
        }
        assert_eq!(value, entry);
    }

    #[test]
    fn unsafe_names_are_rejected() {
        assert!(jar_file_name("app", "1/2").is_err());
        assert!(jar_file_name("app", "1 2").is_err());
        assert!(jar_file_name("app", "1#2").is_err());
        assert!(jar_file_name("app", "1+2").is_ok());
        assert!(artifact_file_name("lib", "1.0", Some("..")).is_err());
        assert!(dependency_relative_path("com..example", "lib.jar").is_err());
        assert!(validate_relative_path("lib/../escape.jar").is_err());
        assert!(validate_relative_path("lib\\escape.jar").is_err());
        assert!(manifest_text(Some("app\nInjected"), &[]).is_err());
        assert!(manifest_text(Some("app App"), &[]).is_err());
        assert!(manifest_text(None, &["lib/with space.jar".to_string()]).is_err());
    }
}
