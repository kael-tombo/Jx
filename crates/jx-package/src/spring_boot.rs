//! Spring Boot's executable-jar layout, written without the Boot plugins.
//!
//! The Boot build plugins repackage a plain jar: project classes move to
//! `BOOT-INF/classes`, runtime jars to `BOOT-INF/lib`, and the loader classes
//! sit at the archive root so `java -jar` can start `JarLauncher`, which
//! builds a class loader over the nested jars. jx writes the same layout; it
//! cannot invent the loader, so the project must declare
//! `org.springframework.boot:spring-boot-loader` at its Boot version.

use std::collections::HashSet;
use std::io::Write as _;
use std::path::Path;

use jx_common::error::{JxError, Kind};

use crate::{
    PackageOutput, ancestor_directories, collect_files, io_error, push_header, reset_dist,
    safe_entry_name, skip_embedded, validate_file_name, validate_manifest_value, zip_error,
    zip_read_error,
};

/// Launcher entry point in a Spring Boot 3.2+ loader jar.
pub const LAUNCHER: &str = "org.springframework.boot.loader.launch.JarLauncher";
const LAUNCHER_CLASS: &str = "org/springframework/boot/loader/launch/JarLauncher.class";

/// Everything a Spring Boot executable jar needs.
pub struct SpringBootRequest<'a> {
    pub dist_dir: &'a Path,
    pub classes_dir: &'a Path,
    pub jar_name: &'a str,
    pub main_class: &'a str,
    /// `spring-boot-loader` jar; its classes go to the archive root.
    pub loader_jar: &'a Path,
    /// The loader version, written as `Spring-Boot-Version`.
    pub loader_version: &'a str,
    /// Runtime jars copied (not merged) below `BOOT-INF/lib`, each with
    /// the file name it takes inside the archive.
    pub dependencies: &'a [NestedJar<'a>],
}

/// One runtime jar nested below `BOOT-INF/lib`: where its bytes live, and
/// the `.jar` name it takes inside the executable archive (the Boot
/// launcher only scans `BOOT-INF/lib` for nested archives, so the name
/// must keep its extension).
#[derive(Debug, Clone)]
pub struct NestedJar<'a> {
    pub path: &'a Path,
    pub name: String,
}

/// Write one executable jar in the Spring Boot 3.2+ layout.
pub fn package(request: &SpringBootRequest<'_>) -> Result<PackageOutput, JxError> {
    validate_file_name(request.jar_name)?;
    validate_manifest_value("Start-Class", request.main_class)?;
    if request.main_class.chars().any(char::is_whitespace) {
        return Err(JxError::new(
            Kind::Build,
            "Start-Class must be a class name without whitespace",
        )
        .at(request.main_class.to_string())
        .hint("fix `[run] main` in project.toml"));
    }

    // Validate the layout and every nested name before touching dist, so a
    // refused package never destroys the previous one.
    let mut loader = open_loader(request)?;
    loader
        .by_name(LAUNCHER_CLASS)
        .map_err(|_| old_loader(request.loader_jar))?;
    let mut nested = Vec::with_capacity(request.dependencies.len());
    let mut seen_names = HashSet::new();
    for jar in request.dependencies {
        validate_file_name(&jar.name)?;
        if !jar.name.ends_with(".jar") {
            return Err(JxError::new(
                Kind::Build,
                format!("nested jar {:?} needs its `.jar` extension", jar.name),
            )
            .hint("the Boot launcher scans `BOOT-INF/lib` for nested archives by extension"));
        }
        if !seen_names.insert(jar.name.clone()) {
            return Err(JxError::new(
                Kind::Build,
                format!("two dependency jars are named {:?}", jar.name),
            )
            .hint(
                "Spring Boot nests jars flat under BOOT-INF/lib; exclude one of the colliding artifacts",
            ));
        }
        nested.push((jar.path, format!("BOOT-INF/lib/{}", jar.name)));
    }

    reset_dist(request.dist_dir)?;
    std::fs::create_dir_all(request.dist_dir).map_err(|e| {
        JxError::new(Kind::Io, "cannot create the dist directory")
            .at(request.dist_dir.display().to_string())
            .caused_by(e)
    })?;
    let jar_path = request.dist_dir.join(request.jar_name);
    let file = std::fs::File::create(&jar_path).map_err(|e| {
        JxError::new(Kind::Io, "cannot create the package jar")
            .at(jar_path.display().to_string())
            .caused_by(e)
    })?;
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(0o644);
    // Nested jars stay uncompressed (STORED): the Boot loader memory-maps
    // them through the outer central directory, which needs raw offsets.
    // This is also why the repackaged layout nests instead of merging.
    let stored = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Stored)
        .unix_permissions(0o644);

    zip.start_file("META-INF/MANIFEST.MF", options)
        .map_err(|e| zip_error(&jar_path, e))?;
    zip.write_all(manifest_text(request).as_bytes())
        .map_err(|e| io_error("cannot write the Spring Boot manifest", &jar_path, e))?;

    // Collect every file entry first: the Boot launcher resolves
    // `BOOT-INF/classes/` and nested archives from the central directory,
    // so explicit directory entries are written before the files.
    let mut loader_names = Vec::new();
    let mut seen = HashSet::new();
    seen.insert("META-INF/MANIFEST.MF".to_string());
    let mut unsafe_entries = 0;
    // Loader classes at the root, exactly where the Boot plugins put them.
    for index in 0..loader.len() {
        let entry = loader
            .by_index(index)
            .map_err(|e| zip_read_error(request.loader_jar, e))?;
        if entry.is_dir() {
            continue;
        }
        let name = entry.name().to_string();
        if skip_embedded(&name) {
            continue;
        }
        if !safe_entry_name(&name) {
            unsafe_entries += 1;
            continue;
        }
        if seen.insert(name.clone()) {
            loader_names.push(name);
        }
    }

    let mut class_files = Vec::new();
    for source in collect_files(request.classes_dir)? {
        let relative = source
            .strip_prefix(request.classes_dir)
            .expect("walked files stay under classes_dir");
        let name = relative
            .components()
            .map(|component| component.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        let entry = format!("BOOT-INF/classes/{name}");
        if !safe_entry_name(&entry) {
            unsafe_entries += 1;
            continue;
        }
        class_files.push((source, entry));
    }

    let mut directories: Vec<String> = Vec::new();
    let ancestors_of = |entry: &str, directories: &mut Vec<String>| {
        for dir in ancestor_directories(entry) {
            if !directories.contains(&dir) {
                directories.push(dir);
            }
        }
    };
    for name in &loader_names {
        ancestors_of(name, &mut directories);
    }
    for (_, entry) in &class_files {
        ancestors_of(entry, &mut directories);
    }
    for (_, entry) in &nested {
        ancestors_of(entry, &mut directories);
    }
    directories.sort();
    for dir in &directories {
        zip.add_directory(dir, options)
            .map_err(|e| zip_error(&jar_path, e))?;
    }

    for name in &loader_names {
        let mut entry = loader
            .by_name(name)
            .map_err(|e| zip_read_error(request.loader_jar, e))?;
        zip.start_file(name, options)
            .map_err(|e| zip_error(&jar_path, e))?;
        std::io::copy(&mut entry, &mut zip).map_err(|e| {
            io_error(
                "cannot copy the Spring Boot loader classes",
                request.loader_jar,
                e,
            )
        })?;
    }

    for (source, entry) in &class_files {
        zip.start_file(entry, options)
            .map_err(|e| zip_error(&jar_path, e))?;
        let mut input = std::fs::File::open(source)
            .map_err(|e| io_error("cannot read a class for packaging", source, e))?;
        std::io::copy(&mut input, &mut zip)
            .map_err(|e| io_error("cannot write a BOOT-INF class", &jar_path, e))?;
    }

    for (source, entry) in &nested {
        zip.start_file(entry, stored)
            .map_err(|e| zip_error(&jar_path, e))?;
        let mut input = std::fs::File::open(source)
            .map_err(|e| io_error("cannot open a dependency jar", source, e))?;
        std::io::copy(&mut input, &mut zip)
            .map_err(|e| io_error("cannot nest a dependency jar", &jar_path, e))?;
    }
    zip.finish().map_err(|e| zip_error(&jar_path, e))?;

    Ok(PackageOutput {
        jar: jar_path,
        lib_dir: None,
        dependency_count: request.dependencies.len(),
        duplicate_entries: 0,
        versioned_entries: 0,
        unsafe_entries,
    })
}

fn open_loader(
    request: &SpringBootRequest<'_>,
) -> Result<zip::ZipArchive<std::io::BufReader<std::fs::File>>, JxError> {
    let file = std::fs::File::open(request.loader_jar).map_err(|e| {
        io_error(
            "cannot open the Spring Boot loader jar",
            request.loader_jar,
            e,
        )
    })?;
    zip::ZipArchive::new(std::io::BufReader::new(file))
        .map_err(|e| zip_read_error(request.loader_jar, e))
}

fn old_loader(loader: &Path) -> JxError {
    JxError::new(
        Kind::Build,
        "the Spring Boot loader jar has no 3.2+ launcher",
    )
    .at(loader.display().to_string())
    .hint(
        "jx writes the `org.springframework.boot.loader.launch` layout; use spring-boot-loader 3.2 or newer",
    )
}

fn manifest_text(request: &SpringBootRequest<'_>) -> String {
    let mut out = String::new();
    push_header(&mut out, "Manifest-Version", "1.0");
    push_header(
        &mut out,
        "Created-By",
        concat!("jx ", env!("CARGO_PKG_VERSION")),
    );
    push_header(&mut out, "Main-Class", LAUNCHER);
    push_header(&mut out, "Start-Class", request.main_class);
    push_header(&mut out, "Spring-Boot-Version", request.loader_version);
    push_header(&mut out, "Spring-Boot-Classes", "BOOT-INF/classes/");
    push_header(&mut out, "Spring-Boot-Lib", "BOOT-INF/lib/");
    out.push_str("\r\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read as _;
    use std::path::PathBuf;
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

    fn loader_jar(dir: &Path, version: &str) -> PathBuf {
        let path = dir.join(format!("spring-boot-loader-{version}.jar"));
        make_jar(
            &path,
            &[
                (
                    "org/springframework/boot/loader/launch/JarLauncher.class",
                    b"launcher",
                ),
                (
                    "org/springframework/boot/loader/archive/Archive.class",
                    b"archive",
                ),
                ("META-INF/MANIFEST.MF", b"loader-manifest"),
            ],
        );
        path
    }

    #[test]
    fn writes_the_spring_boot_executable_layout() {
        // Boot 3.2+ and Boot 4 share the loader layout and launcher class.
        for version in ["3.4.0", "4.0.0"] {
            let dir = tempdir().unwrap();
            let classes = dir.path().join("classes");
            write(&classes.join("app/App.class"), b"app-class");
            let loader = loader_jar(dir.path(), version);
            let dep_one = dir.path().join("jars/dep-one-1.0.jar");
            let dep_two = dir.path().join("jars/dep-two-1.0.jar");
            write(&dep_one, b"one-bytes");
            write(&dep_two, b"two-bytes");

            let output = package(&SpringBootRequest {
                dist_dir: &dir.path().join("dist"),
                classes_dir: &classes,
                jar_name: "app-0.1.0.jar",
                main_class: "app.App",
                loader_jar: &loader,
                loader_version: version,
                dependencies: &[
                    NestedJar {
                        path: &dep_one,
                        name: "dep-one-1.0.jar".to_string(),
                    },
                    NestedJar {
                        path: &dep_two,
                        name: "dep-two-1.0.jar".to_string(),
                    },
                ],
            })
            .unwrap();

            assert_eq!(output.dependency_count, 2);
            assert!(output.lib_dir.is_none());
            let (manifest, names) = read_jar(&output.jar);
            // The launcher resolves the classes root and nested archives
            // from directory entries; a jar of bare files does not start.
            for dir_entry in ["BOOT-INF/", "BOOT-INF/classes/", "BOOT-INF/lib/"] {
                assert!(
                    names.contains(&dir_entry.to_string()),
                    "{dir_entry} is an explicit entry: {names:?}"
                );
            }
            assert!(
                manifest
                    .contains("Main-Class: org.springframework.boot.loader.launch.JarLauncher\r\n"),
                "{manifest}"
            );
            assert!(manifest.contains("Start-Class: app.App\r\n"), "{manifest}");
            assert!(
                manifest.contains(&format!("Spring-Boot-Version: {version}\r\n")),
                "{manifest}"
            );
            assert!(
                manifest.contains("Spring-Boot-Lib: BOOT-INF/lib/\r\n"),
                "{manifest}"
            );
            assert!(!manifest.contains("Class-Path"), "{manifest}");
            assert!(
                names.contains(
                    &"org/springframework/boot/loader/launch/JarLauncher.class".to_string()
                ),
                "{names:?}"
            );
            assert!(
                names.contains(&"BOOT-INF/classes/app/App.class".to_string()),
                "{names:?}"
            );
            assert_eq!(
                read_entry(&output.jar, "BOOT-INF/lib/dep-one-1.0.jar"),
                b"one-bytes"
            );
            assert_eq!(
                read_entry(&output.jar, "BOOT-INF/lib/dep-two-1.0.jar"),
                b"two-bytes"
            );
            assert_eq!(
                read_entry(&output.jar, "META-INF/MANIFEST.MF"),
                manifest.as_bytes()
            );
        }
    }

    #[test]
    fn an_old_loader_is_refused_before_dist_is_touched() {
        let dir = tempdir().unwrap();
        let loader = dir.path().join("spring-boot-loader-3.0.0.jar");
        make_jar(
            &loader,
            &[("org/springframework/boot/loader/JarLauncher.class", b"old")],
        );
        let classes = dir.path().join("classes");
        write(&classes.join("app/App.class"), b"app");
        let dist = dir.path().join("dist");
        std::fs::create_dir_all(&dist).unwrap();
        std::fs::write(dist.join("previous.jar"), b"previous").unwrap();

        let error = package(&SpringBootRequest {
            dist_dir: &dist,
            classes_dir: &classes,
            jar_name: "app-0.1.0.jar",
            main_class: "app.App",
            loader_jar: &loader,
            loader_version: "3.0.0",
            dependencies: &[],
        })
        .err()
        .expect("the old loader is refused");

        assert!(error.message.contains("3.2"), "{error}");
        assert!(
            dist.join("previous.jar").exists(),
            "a refused loader must not clear dist"
        );
    }

    #[test]
    fn duplicate_nested_jar_names_are_refused_before_dist_is_touched() {
        let dir = tempdir().unwrap();
        let classes = dir.path().join("classes");
        write(&classes.join("app/App.class"), b"app");
        let loader = loader_jar(dir.path(), "3.4.0");
        let first = dir.path().join("a/same-1.0.jar");
        let second = dir.path().join("b/same-1.0.jar");
        write(&first, b"first");
        write(&second, b"second");

        let error = package(&SpringBootRequest {
            dist_dir: &dir.path().join("dist"),
            classes_dir: &classes,
            jar_name: "app-0.1.0.jar",
            main_class: "app.App",
            loader_jar: &loader,
            loader_version: "3.4.0",
            dependencies: &[
                NestedJar {
                    path: &first,
                    name: "same-1.0.jar".to_string(),
                },
                NestedJar {
                    path: &second,
                    name: "same-1.0.jar".to_string(),
                },
            ],
        })
        .err()
        .expect("duplicate names are refused");

        assert!(error.message.contains("same-1.0.jar"), "{error}");
        assert!(!dir.path().join("dist").exists());
    }
}
