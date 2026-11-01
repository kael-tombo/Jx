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
}

impl Input {
    fn path(&self) -> &Path {
        match self {
            Input::Maven(path) | Input::Gradle(path) => path,
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
            "imported {} -> {}: {} dependencies, {} dev dependencies",
            source_path.display(),
            output_path.display(),
            imported.dependencies,
            imported.dev_dependencies
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
                "pass a pom.xml, a build.gradle or build.gradle.kts, or a directory containing one",
            )),
    }
}

fn classify_file(path: &Path) -> Result<Input, JxError> {
    match path.file_name().and_then(|name| name.to_str()) {
        Some("pom.xml") => Ok(Input::Maven(path.to_path_buf())),
        Some("build.gradle" | "build.gradle.kts") => Ok(Input::Gradle(path.to_path_buf())),
        _ => Err(JxError::new(
            Kind::Usage,
            "the import source is not a Maven POM or a Gradle build script",
        )
        .at(path.display().to_string())
        .hint("use a pom.xml, build.gradle or build.gradle.kts; settings files are read automatically")),
    }
}

/// Prefer Maven when a directory has both builds, but say so: the user can
/// pass the Gradle script explicitly to convert that build instead.
fn detect_in(dir: &Path) -> Result<(Input, Option<String>), JxError> {
    let pom = dir.join("pom.xml");
    let kotlin = dir.join("build.gradle.kts");
    let groovy = dir.join("build.gradle");
    let gradle = if kotlin.is_file() {
        Some(kotlin)
    } else if groovy.is_file() {
        Some(groovy)
    } else {
        None
    };
    match (pom.is_file(), gradle) {
        (true, Some(gradle)) => Ok((
            Input::Maven(pom),
            Some(format!(
                "this directory has both pom.xml and {}; imported pom.xml — pass {} explicitly to convert Gradle instead",
                gradle
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy(),
                gradle
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
            )),
        )),
        (true, None) => Ok((Input::Maven(pom), None)),
        (false, Some(gradle)) => Ok((Input::Gradle(gradle), None)),
        (false, None) => Err(JxError::new(
            Kind::Io,
            "no Maven POM or Gradle build script found",
        )
        .at(dir.display().to_string())
        .hint(
            "pass a pom.xml, build.gradle or build.gradle.kts; `jx import` defaults to the current directory",
        )),
    }
}
