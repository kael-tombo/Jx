//! `jx import`: convert a Maven POM into project.toml.
//!
//! The effective POM (parents, properties, dependencyManagement and imported
//! BOMs included) is read with jx's own loader, then converted conservatively.
//! Nothing is written until the generated manifest has been parsed by the
//! same validator every other command uses, and an existing manifest is never
//! overwritten without `--force`.

use std::path::{Path, PathBuf};

use jx_common::config;
use jx_common::error::{JxError, Kind};
use jx_common::paths;
use jx_manifest as manifest;
use jx_maven::{Client, PomLoader};
use jx_toolchain as toolchain;

use crate::Cli;

/// Run `jx import`. `path` is the POM (or a directory containing pom.xml).
pub fn maven(
    cli: &Cli,
    path: Option<PathBuf>,
    output: Option<PathBuf>,
    force: bool,
    dry_run: bool,
) -> Result<(), JxError> {
    let cwd = std::env::current_dir()
        .map_err(|e| JxError::new(Kind::Io, "cannot read the current directory").caused_by(e))?;
    let pom_path = resolve_pom_path(path, &cwd)?;
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
    if output_path == pom_path {
        return Err(
            JxError::new(Kind::Usage, "the import output would overwrite the POM")
                .at(output_path.display().to_string())
                .hint("choose a different --output path"),
        );
    }

    let home = paths::jx_home()?;
    let settings = config::load(&home)?;
    let client = Client::new(&home, settings, cli.offline)?;
    let loader = PomLoader::new(&client);
    let pom = loader.load_file(&pom_path)?;
    let imported = jx_interop::manifest_from_pom(
        &pom,
        &pom_path.display().to_string(),
        toolchain::DEFAULT_FEATURE,
    )?;

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

    let notes = &imported.notes;
    if dry_run {
        for note in notes {
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
            pom_path.display(),
            output_path.display(),
            imported.dependencies,
            imported.dev_dependencies
        );
    }
    for note in notes {
        println!("  note: {note}");
    }
    if !cli.quiet {
        println!(
            "next: `jx lock`, then `jx build`; review {} against the original POM",
            output_path.display()
        );
    }
    Ok(())
}

fn resolve_pom_path(path: Option<PathBuf>, cwd: &Path) -> Result<PathBuf, JxError> {
    let candidate = match path {
        None => cwd.join("pom.xml"),
        Some(path) if path.is_dir() => path.join("pom.xml"),
        Some(path) => path,
    };
    if candidate.is_file() {
        Ok(candidate)
    } else {
        Err(JxError::new(
            Kind::Io,
            "no Maven POM found at the import source",
        )
        .at(candidate.display().to_string())
        .hint(
            "pass the path to a pom.xml or to a directory containing one; `jx import` defaults to ./pom.xml",
        ))
    }
}
