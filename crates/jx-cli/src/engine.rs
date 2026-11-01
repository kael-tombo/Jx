//! Which engine owns the project's build: jx's own pipeline, or a delegated
//! Maven/Gradle build.
//!
//! `[build] engine` in project.toml chooses persistently; the global
//! `--engine` flag overrides it for one invocation. Delegated `build`, `test`
//! and `clean` go through [`jx_interop::delegate`]; commands that would read
//! or write jx's own dependency graph are refused, because the external build
//! would ignore them (ADR 0012).

use jx_common::error::{JxError, Kind};
use jx_manifest::{BuildEngine, Manifest};

use crate::Cli;

/// CLI override first, then `[build] engine`, then jx.
pub(crate) fn selected(cli: &Cli, manifest: &Manifest) -> BuildEngine {
    cli.engine
        .map(BuildEngine::from)
        .unwrap_or(manifest.build_engine)
}

/// Display name for progress and errors.
pub(crate) fn label(engine: BuildEngine) -> &'static str {
    match engine {
        BuildEngine::Jx => "jx",
        BuildEngine::Maven => "Maven",
        BuildEngine::Gradle => "Gradle",
    }
}

/// Refuse a command whose result the delegated build would ignore.
///
/// `remedy` completes the hint with the command-specific next step; the
/// `--engine jx` override and the manifest edit are always offered.
pub(crate) fn require_jx(
    cli: &Cli,
    manifest: &Manifest,
    command: &str,
    remedy: &str,
) -> Result<(), JxError> {
    let engine = selected(cli, manifest);
    if !engine.is_delegated() {
        return Ok(());
    }
    Err(JxError::new(
        Kind::Usage,
        format!(
            "`{command}` is unavailable while this project delegates its build to {}",
            label(engine)
        ),
    )
    .hint(format!(
        "{remedy}. Pass `--engine jx` to use jx's own manifest for this command, or remove `[build] engine` from project.toml."
    )))
}
