//! Compilation with the pinned JDK's `javac`. Builds are all-or-nothing per
//! source set, but a fingerprint of the exact command (javac, sources,
//! resources, output dir and classpath) lets an unchanged build skip javac. The
//! argument file is written to dodge Windows command-line limits, and
//! diagnostics pass through untouched in place.

use std::path::{Path, PathBuf};
use std::process::Command;

use jx_common::error::{JxError, Kind};
use sha2::Digest as _;

/// One folder of sources (or resources) that lands in a destination dir.
#[derive(Debug, Clone)]
pub struct SourceSet {
    pub root: PathBuf,
}

/// One compile request: where the sources and resources are, where the
/// classes go, and what the compiler must see. Grouped so call sites read as
/// a description instead of a positional argument list.
pub struct CompileRequest<'a> {
    pub javac: &'a Path,
    pub feature: u32,
    pub sources_root: &'a Path,
    pub resources_root: Option<&'a Path>,
    pub out: &'a Path,
    pub classpath: &'a [PathBuf],
    pub cache_dir: &'a Path,
    pub label: &'a str,
    /// Compile even when the fingerprint matches (used when main sources were
    /// recompiled and test classes must follow).
    pub force: bool,
}

/// Compile one source set, but only when its fingerprint changed since the
/// last successful run. Returns `true` when javac actually ran.
pub fn compile_if_stale(request: &CompileRequest<'_>) -> Result<bool, JxError> {
    let hash = command_fingerprint(
        request.javac,
        request.feature,
        request.sources_root,
        request.resources_root,
        request.out,
        request.classpath,
    )?;
    if !request.force
        && read_fingerprint(request.cache_dir).as_deref() == Some(hash.as_str())
        && request.out.is_dir()
    {
        println!("  {}: up to date", request.label);
        return Ok(false);
    }
    compile(
        request.javac,
        request.feature,
        request.sources_root,
        request.out,
        request.classpath,
    )?;
    write_fingerprint(request.cache_dir, &hash)?;
    Ok(true)
}

/// Hash everything that determines a build's output: the compiler path, the
/// release, the destination, every source and resource file's relative name
/// and bytes, and every classpath entry's path, size and mtime. Resources are
/// inputs because deleting one must also delete its copy from `out`. Cache
/// jars are content addressed, but local jars can change in place, hence the
/// metadata.
pub fn command_fingerprint(
    javac: &Path,
    feature: u32,
    sources_root: &Path,
    resources_root: Option<&Path>,
    out: &Path,
    classpath: &[PathBuf],
) -> Result<String, JxError> {
    let mut hasher = sha2::Sha256::new();
    feed(&mut hasher, b"jx-build-fingerprint-v1");
    feed(&mut hasher, javac.to_string_lossy().as_bytes());
    feed(&mut hasher, &feature.to_be_bytes());
    feed(&mut hasher, out.to_string_lossy().as_bytes());
    for source in collect_files(sources_root, Some("java")) {
        let relative = source
            .strip_prefix(sources_root)
            .expect("collected under sources_root");
        feed(&mut hasher, b"s");
        feed(&mut hasher, relative.to_string_lossy().as_bytes());
        feed(&mut hasher, &read_bytes(&source)?);
    }
    if let Some(resources_root) = resources_root {
        for resource in collect_files(resources_root, None) {
            let relative = resource
                .strip_prefix(resources_root)
                .expect("collected under resources_root");
            feed(&mut hasher, b"r");
            feed(&mut hasher, relative.to_string_lossy().as_bytes());
            feed(&mut hasher, &read_bytes(&resource)?);
        }
    }
    for entry in classpath {
        feed(&mut hasher, entry.to_string_lossy().as_bytes());
        match std::fs::metadata(entry) {
            Ok(meta) => {
                feed(&mut hasher, &meta.len().to_be_bytes());
                let mtime = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_nanos())
                    .unwrap_or(0);
                feed(&mut hasher, &mtime.to_be_bytes());
            }
            Err(_) => feed(&mut hasher, b"missing"),
        }
    }
    Ok(hex::encode(hasher.finalize()))
}

fn feed(hasher: &mut sha2::Sha256, bytes: &[u8]) {
    hasher.update((bytes.len() as u64).to_be_bytes());
    hasher.update(bytes);
}

fn read_bytes(path: &Path) -> Result<Vec<u8>, JxError> {
    std::fs::read(path).map_err(|e| {
        JxError::new(Kind::Io, "cannot read a source file for fingerprinting")
            .at(path.display().to_string())
            .caused_by(e)
    })
}

fn fingerprint_path(cache_dir: &Path) -> PathBuf {
    cache_dir.join("fingerprint")
}

fn read_fingerprint(cache_dir: &Path) -> Option<String> {
    std::fs::read_to_string(fingerprint_path(cache_dir))
        .ok()
        .map(|value| value.trim().to_string())
}

fn write_fingerprint(cache_dir: &Path, hash: &str) -> Result<(), JxError> {
    std::fs::create_dir_all(cache_dir).map_err(|e| {
        JxError::new(Kind::Io, "cannot create the build cache directory")
            .at(cache_dir.display().to_string())
            .caused_by(e)
    })?;
    let path = fingerprint_path(cache_dir);
    std::fs::write(&path, hash).map_err(|e| {
        JxError::new(Kind::Io, "cannot write the build fingerprint")
            .at(path.display().to_string())
            .caused_by(e)
    })
}

/// Compile `src/main/java` into `out` with `javac --release <feature>`.
/// `classpath` holds the locked dependency jars (empty for a plain project).
/// Streams javac's own output; on a non-zero exit the error is diagnosed
/// with the suggested next step.
pub fn compile(
    javac: &Path,
    feature: u32,
    sources_root: &Path,
    out: &Path,
    classpath: &[PathBuf],
) -> Result<(), JxError> {
    let sources = collect_files(sources_root, Some("java"));
    if sources.is_empty() {
        return Err(JxError::new(
            Kind::Build,
            format!("no Java sources under {}", sources_root.display()),
        )
        .hint("this project has no src/main/java tree; `jx new`/`jx init` create one"));
    }
    // A full recompile replaces the output directory: class files for
    // deleted sources must not survive into the next build.
    if out.exists() {
        std::fs::remove_dir_all(out).map_err(|e| {
            JxError::new(Kind::Io, "cannot clear the classes output directory")
                .at(out.display().to_string())
                .caused_by(e)
                .hint("close any running program that holds files under target/")
        })?;
    }
    std::fs::create_dir_all(out).map_err(|e| {
        JxError::new(Kind::Io, "cannot create the classes output directory")
            .at(out.display().to_string())
            .caused_by(e)
    })?;

    let argfile = out.join("jx-javac-args.txt");
    let args = build_argfile(feature, out, &sources, classpath);
    std::fs::write(&argfile, &args).map_err(|e| {
        JxError::new(Kind::Io, "cannot write the javac argument file")
            .at(argfile.display().to_string())
            .caused_by(e)
    })?;

    let status = Command::new(javac)
        .arg(format!("@{}", argfile.display()))
        .status()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                JxError::new(Kind::Toolchain, "javac is missing from the installed JDK")
                    .at(javac.display().to_string())
                    .caused_by(e)
                    .hint("the JDK install looks damaged; `jx jdk remove` then rebuild")
            } else {
                JxError::new(Kind::Build, "cannot launch javac from the pinned JDK")
                    .at(javac.display().to_string())
                    .caused_by(e)
                    .hint("run `jx doctor`; check the archive extracted correctly")
            }
        })?;
    if !status.success() {
        return Err(JxError::new(
            Kind::Build,
            format!("javac failed (exit {})", status.code().unwrap_or(-1)),
        )
        .hint("missing imports usually mean a dependency is absent; add it with `jx add`"));
    }
    Ok(())
}

/// Copy every file under `resources_root` into `out`, preserving relative
/// structure. Missing roots are not an error (the Maven layout makes it
/// optional).
pub fn copy_resources(resources_root: &Path, out: &Path) -> Result<usize, JxError> {
    if !resources_root.is_dir() {
        return Ok(0);
    }
    let files = collect_files(resources_root, None);
    let mut copied = 0;
    for file in &files {
        let relative = file
            .strip_prefix(resources_root)
            .expect("file was collected under root");
        let target = out.join(relative);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                JxError::new(Kind::Io, "cannot create a resource directory")
                    .at(parent.display().to_string())
                    .caused_by(e)
            })?;
        }
        std::fs::copy(file, &target).map_err(|e| {
            JxError::new(Kind::Io, "cannot copy a resource")
                .at(format!("{} -> {}", file.display(), target.display()))
                .caused_by(e)
        })?;
        copied += 1;
    }
    Ok(copied)
}

fn build_argfile(feature: u32, out: &Path, sources: &[PathBuf], classpath: &[PathBuf]) -> String {
    let mut args = String::new();
    args.push_str(&format!("--release {feature}\n"));
    args.push_str("-encoding UTF-8\n");
    args.push_str(&format!("-d {}\n", quote(out.to_string_lossy())));
    if !classpath.is_empty() {
        let separator = if cfg!(windows) { ";" } else { ":" };
        let joined = classpath
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(separator);
        args.push_str(&format!("-classpath {}\n", quote(joined)));
    }
    for s in sources {
        args.push_str(&quote(s.to_string_lossy()));
        args.push('\n');
    }
    args
}

/// javac argument files understand double quotes; trailing backslashes before
/// a quote are the one edge Windows paths hit, and they only occur for root
/// directories, which are never quoted here.
fn quote(path: impl std::fmt::Display) -> String {
    let raw = path.to_string();
    if raw.contains(' ') || raw.contains('\t') {
        format!("\"{}\"", raw.replace('\\', "\\\\"))
    } else {
        raw
    }
}

/// Recursive file walker: deterministic order; `ext` filters by extension
/// (`None` takes every file, e.g. for resources).
fn collect_files(root: &Path, ext: Option<&str>) -> Vec<PathBuf> {
    if !root.is_dir() {
        return Vec::new();
    }
    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let mut entries: Vec<_> = match std::fs::read_dir(&dir) {
            Ok(entries) => entries.filter_map(|e| e.ok()).collect(),
            Err(_) => continue,
        };
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            let path = entry.path();
            let ft = match entry.file_type() {
                Ok(ft) => ft,
                Err(_) => continue,
            };
            if ft.is_dir() {
                stack.push(path);
            } else if ft.is_file()
                && ext.is_none_or(|wanted| path.extension().is_some_and(|e| e == wanted))
            {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn walker_is_deterministic_and_slices_java_only() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("src/main/java");
        std::fs::create_dir_all(root.join("app")).unwrap();
        std::fs::write(root.join("app/B.java"), "class B {}").unwrap();
        std::fs::write(root.join("app/A.java"), "class A {}").unwrap();
        std::fs::write(root.join("app/readme.txt"), "hi").unwrap();
        let files = collect_files(&root, Some("java"));
        let names: Vec<String> = files
            .iter()
            .map(|f| f.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["A.java", "B.java"]); // sorted, no .txt
    }

    #[test]
    fn resources_copy_creates_structure() {
        let dir = tempdir().unwrap();
        let res = dir.path().join("resources");
        std::fs::create_dir_all(res.join("config")).unwrap();
        std::fs::write(res.join("config/app.properties"), "x=1").unwrap();
        let out = dir.path().join("classes");
        let n = copy_resources(&res, &out).unwrap();
        assert_eq!(n, 1);
        assert!(out.join("config/app.properties").is_file());
        // empty roots are fine
        let n2 = copy_resources(&dir.path().join("absent"), &out).unwrap();
        assert_eq!(n2, 0);
    }

    #[test]
    fn argfile_adds_a_classpath_only_when_present() {
        let out = Path::new("target/classes");
        let sources = [PathBuf::from("src/main/java/app/App.java")];
        let plain = build_argfile(25, out, &sources, &[]);
        assert!(!plain.contains("-classpath"));
        let jars = [
            PathBuf::from("cache/content/aa/one.jar"),
            PathBuf::from("cache/content/bb/two.jar"),
        ];
        let with = build_argfile(25, out, &sources, &jars);
        assert!(with.contains("-classpath cache/content/aa/one.jar"));
        assert!(with.contains(if cfg!(windows) {
            "cache/content/aa/one.jar;cache/content/bb/two.jar"
        } else {
            "cache/content/aa/one.jar:cache/content/bb/two.jar"
        }));
    }

    #[test]
    fn fingerprint_tracks_sources_and_classpath() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("src/main/java");
        std::fs::create_dir_all(&root).unwrap();
        let source = root.join("App.java");
        std::fs::write(&source, "class App {}").unwrap();
        let out = dir.path().join("classes");
        let jar = dir.path().join("dep.jar");
        std::fs::write(&jar, b"jar").unwrap();
        let javac = Path::new("javac");
        let first = command_fingerprint(javac, 25, &root, None, &out, &[]).unwrap();
        assert_eq!(
            first,
            command_fingerprint(javac, 25, &root, None, &out, &[]).unwrap()
        );
        let with_jar =
            command_fingerprint(javac, 25, &root, None, &out, std::slice::from_ref(&jar)).unwrap();
        assert_ne!(first, with_jar);
        std::fs::write(&source, "class App { int x; }").unwrap();
        assert_ne!(
            first,
            command_fingerprint(javac, 25, &root, None, &out, &[]).unwrap()
        );
        assert_ne!(
            with_jar,
            command_fingerprint(javac, 26, &root, None, &out, &[jar]).unwrap()
        );

        let resources = dir.path().join("src/main/resources");
        std::fs::create_dir_all(&resources).unwrap();
        std::fs::write(resources.join("app.properties"), "x=1").unwrap();
        let with_resource =
            command_fingerprint(javac, 25, &root, Some(&resources), &out, &[]).unwrap();
        std::fs::write(resources.join("app.properties"), "x=2").unwrap();
        assert_ne!(
            with_resource,
            command_fingerprint(javac, 25, &root, Some(&resources), &out, &[]).unwrap()
        );
    }

    #[test]
    fn compile_without_sources_is_a_clear_error() {
        let dir = tempdir().unwrap();
        let fake_javac = dir.path().join("javac");
        let e = compile(
            &fake_javac,
            25,
            &dir.path().join("src"),
            &dir.path().join("out"),
            &[],
        )
        .expect_err("no sources");
        assert_eq!(e.kind, Kind::Build);
        assert!(e.message.contains("no Java sources"));
    }
}
