//! `jx script`: hand a single-file Java (or JBang-supported) program to JBang.
//!
//! This is an adapter, not a script runner. JBang owns script dependencies,
//! caching and execution. When the script is inside a jx project, jx installs
//! and exports that project's pinned JDK so both tools use the same Java; when
//! there is no manifest, JBang keeps its own Java policy (ADR 0015).

use std::path::PathBuf;
use std::process::ExitCode;

use jx_common::error::{JxError, Kind};
use jx_interop::jbang;
use jx_manifest as manifest;

use crate::{Cli, locate_manifest};

pub fn run(cli: &Cli, file: PathBuf, args: Vec<String>) -> Result<ExitCode, JxError> {
    let cwd = std::env::current_dir()
        .map_err(|e| JxError::new(Kind::Io, "cannot read the current directory").caused_by(e))?;
    let script = if file.is_absolute() {
        file
    } else {
        cwd.join(file)
    };

    // Resolve JBang before touching the JDK store: a missing tool or script
    // must fail without downloading anything.
    let plan = jbang::plan(&jbang::Request {
        script: &script,
        cwd: &cwd,
        args: &args,
        offline: cli.offline,
    })?;

    // A script belongs to the project above it, not to the shell's working
    // directory. An explicit --manifest-path still wins.
    let manifest_start = script.parent().unwrap_or(&cwd);
    let manifest_path = match cli.manifest_path.clone() {
        Some(_) => Some(locate_manifest(cli)?),
        None => manifest::find_manifest(manifest_start).ok(),
    };
    let java_home = match manifest_path {
        Some(path) => {
            let project = manifest::parse(&path)?;
            // JBang is a delegated adapter: it follows the manifest's JDK
            // feature and never reads jx.lock.
            let jdk = crate::build::ensure_jdk(cli, project.jdk_feature()?, None)?;
            Some(jdk.dir)
        }
        None => None,
    };
    if !cli.quiet {
        match &java_home {
            Some(home) => eprintln!(
                "delegating {} to JBang with the pinned JDK at {}",
                script.display(),
                home.display()
            ),
            None => eprintln!("delegating {} to JBang", script.display()),
        }
    }
    let code = jbang::execute(&plan, java_home.as_deref())?;
    Ok(ExitCode::from(u8::try_from(code).unwrap_or(1)))
}
