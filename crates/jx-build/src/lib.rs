//! Compilation with the pinned JDK's `javac`. V1 is whole-project, not
//! incremental: the argument file is written to dodge Windows command-line
//! limits, all sources are recompiled when anything changes (fingerprint
//! skipping arrives in M4), and diagnostics pass through untouched in place.

use std::path::{Path, PathBuf};
use std::process::Command;

use jx_common::error::{JxError, Kind};

/// One folder of sources (or resources) that lands in a destination dir.
#[derive(Debug, Clone)]
pub struct SourceSet {
    pub root: PathBuf,
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
