//! `jx import`: convert a Maven POM or a Gradle build into project.toml.
//!
//! Maven is read through jx's own effective-POM loader (parents, properties,
//! dependencyManagement and imported BOMs included). Gradle is read
//! statically and conservatively: only declarative dependencies, toolchain,
//! main class and version declarations are translated, and anything that
//! needs evaluation stops the import. Nothing is written until the generated
//! manifest has been parsed by the same validator every other command uses,
//! and an existing manifest is never overwritten without `--force`.

use std::path::{Path, PathBuf};

use jx_common::config;
use jx_common::error::{JxError, Kind};
use jx_common::paths;
use jx_manifest as manifest;
use jx_maven::{Client, PomLoader};
use jx_toolchain as toolchain;

use crate::Cli;

#[derive(Debug)]
enum Input {
    Maven(PathBuf),
    Gradle(PathBuf),
    Jot(PathBuf),
}

impl Input {
    fn path(&self) -> &Path {
        match self {
            Input::Maven(path) | Input::Gradle(path) | Input::Jot(path) => path,
        }
    }
}

/// Run `jx import`. `path` is a POM, a Gradle build script, or a directory
/// containing one.
pub fn run(
    cli: &Cli,
    path: Option<PathBuf>,
    output: Option<PathBuf>,
    force: bool,
    dry_run: bool,
) -> Result<(), JxError> {
    let cwd = std::env::current_dir()
        .map_err(|e| JxError::new(Kind::Io, "cannot read the current directory").caused_by(e))?;
    let (input, note) = resolve_input(path, &cwd)?;
    let source_path = input.path().to_path_buf();
    let output_path = output
        .or_else(|| cli.manifest_path.clone())
        .unwrap_or_else(|| cwd.join("project.toml"));

    if output_path.exists() && !force && !dry_run {
        return Err(JxError::new(
            Kind::Usage,
            "a jx manifest already exists at the import destination",
        )
        .at(output_path.display().to_string())
        .hint("pass --force to overwrite it, --output to choose another path, or --dry-run to inspect the conversion"));
    }
    if output_path == source_path {
        return Err(JxError::new(
            Kind::Usage,
            "the import output would overwrite the build file",
        )
        .at(output_path.display().to_string())
        .hint("choose a different --output path"));
    }

    if let Some(note) = &note {
        eprintln!("note: {note}");
    }

    let imported = match &input {
        Input::Maven(pom_path) => {
            let home = paths::jx_home()?;
            let settings = config::load(&home)?;
            let client = Client::new(&home, settings, cli.offline)?;
            let loader = PomLoader::new(&client);
            let pom = loader.load_file(pom_path)?;
            jx_interop::manifest_from_pom(
                &pom,
                &pom_path.display().to_string(),
                toolchain::DEFAULT_FEATURE,
            )?
        }
        Input::Jot(manifest) => {
            let text = read_text(manifest, "the jot manifest")?;
            let catalog = find_jot_catalog(manifest)
                .map(|path| {
                    read_text(&path, "libs.versions.toml")
                        .map(|text| (path.display().to_string(), text))
                })
                .transpose()?;
            jx_interop::jot::manifest_from_jot(&jx_interop::jot::JotSource {
                path: manifest.display().to_string(),
                text,
                catalog,
                default_version: "0.1.0".to_string(),
                default_feature: toolchain::DEFAULT_FEATURE,
            })?
        }
        Input::Gradle(script) => {
            let text = read_text(script, "the Gradle build script")?;
            let dir = script
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .unwrap_or(Path::new("."));
            let settings = ["settings.gradle.kts", "settings.gradle"]
                .iter()
                .map(|name| dir.join(name))
                .find(|candidate| candidate.is_file())
                .map(|candidate| {
                    read_text(&candidate, "the Gradle settings file")
                        .map(|text| (candidate.display().to_string(), text))
                })
                .transpose()?;
            let properties = {
                let candidate = dir.join("gradle.properties");
                candidate
                    .is_file()
                    .then(|| {
                        read_text(&candidate, "gradle.properties")
                            .map(|text| (candidate.display().to_string(), text))
                    })
                    .transpose()?
            };
            let dir_name = dir
                .canonicalize()
                .ok()
                .or_else(|| Some(dir.to_path_buf()))
                .and_then(|path| {
                    path.file_name()
                        .map(|name| name.to_string_lossy().to_string())
                })
                .or_else(|| {
                    std::env::current_dir().ok().and_then(|cwd| {
                        cwd.file_name()
                            .map(|name| name.to_string_lossy().to_string())
                    })
                })
                .filter(|name| !name.is_empty())
                .ok_or_else(|| {
                    JxError::new(
                        Kind::Io,
                        "cannot derive a project name from the Gradle directory",
                    )
                    .hint("pass a build script inside a named directory")
                })?;
            jx_interop::gradle::manifest_from_gradle(&jx_interop::gradle::GradleSource {
                script: script.display().to_string(),
                text,
                settings,
                properties,
                dir_name,
                default_version: "0.1.0".to_string(),
                default_feature: toolchain::DEFAULT_FEATURE,
            })?
        }
    };

    // The converter is supposed to produce a valid manifest; validate it with
    // the same parser `jx build` will use so a bug can never write a broken
    // project.toml.
    if let Err(error) = manifest::parse_text(&imported.text, &output_path) {
        return Err(JxError::new(
            Kind::Internal,
            "the generated project.toml failed jx's own manifest validation",
        )
        .at(output_path.display().to_string())
        .hint(format!(
            "this is a jx bug; nothing was written ({})",
            error.message
        )));
    }

    if dry_run {
        for note in &imported.notes {
            eprintln!("note: {note}");
        }
        print!("{}", imported.text);
        return Ok(());
    }

    if let Some(parent) = output_path.parent()
        && !parent.as_os_str().is_empty()
        && !parent.is_dir()
    {
        return Err(JxError::new(
            Kind::Io,
            "the manifest destination directory does not exist",
        )
        .at(parent.display().to_string())
        .hint("create the directory first, or choose another --output path"));
    }
    std::fs::write(&output_path, &imported.text).map_err(|e| {
        JxError::new(Kind::Io, "cannot write the imported manifest")
            .at(output_path.display().to_string())
            .caused_by(e)
    })?;

    if !cli.quiet {
        println!(
            "imported {} -> {}: {} dependencies, {} dev dependencies, {} processors",
            source_path.display(),
            output_path.display(),
            imported.dependencies,
            imported.dev_dependencies,
            imported.processors
        );
    }
    for note in &imported.notes {
        println!("  note: {note}");
    }
    if !cli.quiet {
        println!(
            "next: `jx lock`, then `jx build`; review {} against the original build",
            output_path.display()
        );
    }
    Ok(())
}

/// jot keeps `libs.versions.toml` beside `jot.toml` for a standalone
/// project and at the workspace root for a member. Prefer the local file;
/// otherwise accept the nearest ancestor that also has a `jot.toml` (the
/// documented workspace root), never an unrelated catalog further up.
fn find_jot_catalog(manifest: &Path) -> Option<PathBuf> {
    let dir = manifest
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let local = dir.join("libs.versions.toml");
    if local.is_file() {
        return Some(local);
    }
    for ancestor in dir.ancestors().skip(1) {
        let root_manifest = ancestor.join("jot.toml");
        let catalog = ancestor.join("libs.versions.toml");
        if root_manifest.is_file() && catalog.is_file() {
            return Some(catalog);
        }
    }
    None
}

fn read_text(path: &Path, what: &str) -> Result<String, JxError> {
    std::fs::read_to_string(path).map_err(|e| {
        JxError::new(Kind::Io, format!("cannot read {what}"))
            .at(path.display().to_string())
            .caused_by(e)
    })
}

fn resolve_input(path: Option<PathBuf>, cwd: &Path) -> Result<(Input, Option<String>), JxError> {
    match path {
        None => detect_in(cwd),
        Some(path) if path.is_dir() => detect_in(&path),
        Some(path) if path.is_file() => Ok((classify_file(&path)?, None)),
        Some(path) => Err(JxError::new(Kind::Io, "the import source does not exist")
            .at(path.display().to_string())
            .hint(
                "pass a pom.xml, a build.gradle, build.gradle.kts or jot.toml, or a directory containing one",
            )),
    }
}

fn classify_file(path: &Path) -> Result<Input, JxError> {
    match path.file_name().and_then(|name| name.to_str()) {
        Some("pom.xml") => Ok(Input::Maven(path.to_path_buf())),
        Some("build.gradle" | "build.gradle.kts") => Ok(Input::Gradle(path.to_path_buf())),
        Some("jot.toml") => Ok(Input::Jot(path.to_path_buf())),
        _ => Err(JxError::new(
            Kind::Usage,
            "the import source is not a Maven POM, a Gradle build script or a jot manifest",
        )
        .at(path.display().to_string())
        .hint("use a pom.xml, build.gradle, build.gradle.kts or jot.toml; settings files are read automatically")),
    }
}

/// Prefer Maven, then Gradle, then jot when a directory has more than one
/// build, but say so: the user can pass the other file explicitly.
fn detect_in(dir: &Path) -> Result<(Input, Option<String>), JxError> {
    let pom = dir.join("pom.xml");
    let kotlin = dir.join("build.gradle.kts");
    let groovy = dir.join("build.gradle");
    let jot = dir.join("jot.toml");
    let gradle = if kotlin.is_file() {
        Some(kotlin)
    } else if groovy.is_file() {
        Some(groovy)
    } else {
        None
    };
    let has_jot = jot.is_file();
    let multiple = |chosen: &str, others: &[&Path]| {
        if others.is_empty() {
            None
        } else {
            let names = others
                .iter()
                .filter_map(|path| path.file_name())
                .map(|name| name.to_string_lossy())
                .collect::<Vec<_>>()
                .join(" and ");
            Some(format!(
                "this directory has {chosen} and {names}; imported {chosen} — pass the other file explicitly to convert it instead"
            ))
        }
    };

    if pom.is_file() {
        let others = gradle
            .iter()
            .map(PathBuf::as_path)
            .chain(has_jot.then_some(jot.as_path()));
        return Ok((
            Input::Maven(pom),
            multiple("pom.xml", &others.collect::<Vec<_>>()),
        ));
    }
    if let Some(gradle) = gradle {
        let chosen = gradle
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "build.gradle".to_string());
        let others = has_jot.then_some(jot.as_path());
        return Ok((
            Input::Gradle(gradle),
            multiple(&chosen, &others.into_iter().collect::<Vec<_>>()),
        ));
    }
    if has_jot {
        return Ok((Input::Jot(jot), None));
    }
    Err(JxError::new(
        Kind::Io,
        "no Maven POM, Gradle build script or jot manifest found",
    )
    .at(dir.display().to_string())
    .hint(
        "pass a pom.xml, build.gradle, build.gradle.kts or jot.toml; `jx import` defaults to the current directory",
    ))
}
