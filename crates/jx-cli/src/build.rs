//! CLI orchestration of build and run for M1: manifest → JDK → javac →
//! (optionally) main-class launch. Dependencies join the classpath in M3;
//! entrypoint discovery lives in `jx-run`, compilation in `jx-build`.

use jx_common::error::{JxError, Kind};
use jx_manifest as manifest;
use jx_toolchain as toolchain;

use crate::{Cli, locate_manifest};

struct Loaded {
    manifest_dir: std::path::PathBuf,
    manifest: manifest::Manifest,
}

fn load(cli: &Cli) -> Result<Loaded, JxError> {
    let path = locate_manifest(cli)?;
    let manifest_dir = path
        .parent()
        .expect("manifest path always has a parent")
        .to_path_buf();
    let manifest = manifest::parse(&path)?;
    Ok(Loaded {
        manifest_dir,
        manifest,
    })
}

/// The JDK for the pinned feature: installed one wins; else download —
/// unless --offline, which hard-fails with the polished message M1 owes.
fn ensure_jdk(cli: &Cli, feature: u32) -> Result<toolchain::InstalledJdk, JxError> {
    let home = toolchain::default_home()?;
    if let Some(found) = toolchain::installed_for_feature(&home, feature)? {
        return Ok(found);
    }
    if cli.offline {
        return Err(JxError::new(
            Kind::Toolchain,
            format!("--offline: no jx JDK {feature} is installed and downloads are disabled"),
        )
        .hint(format!(
            "run `jx jdk install {feature}` once with a network; everything after works offline"
        )));
    }
    toolchain::ensure_jdk(&home, feature)
}

/// `compile` + resource copy for the project the CLI resolved. Returns the
/// locked dependency jars so `run` can hand the same classpath to the JVM.
fn compile_project(cli: &Cli, loaded: &Loaded) -> Result<Vec<std::path::PathBuf>, JxError> {
    let feature = loaded.manifest.jdk_feature()?;
    let dependency_jars = crate::deps::classpath(cli)?;
    let jdk = ensure_jdk(cli, feature)?;
    let classes = loaded.manifest_dir.join("target").join("classes");
    jx_build::compile(
        &jdk.javac(),
        feature,
        &loaded.manifest_dir.join("src/main/java"),
        &classes,
        &dependency_jars,
    )?;
    let n = jx_build::copy_resources(&loaded.manifest_dir.join("src/main/resources"), &classes)?;
    if n > 0 {
        println!("  copied {n} resources");
    }
    Ok(dependency_jars)
}

/// `jx build`
pub fn build(cli: &Cli) -> Result<(), JxError> {
    let loaded = load(cli)?;
    compile_project(cli, &loaded)?;
    Ok(())
}

/// `jx run [-- args]`, with optional `--debug [--port N]`.
pub fn run(
    cli: &Cli,
    debug_port: Option<u16>,
    argv: Vec<String>,
) -> Result<std::process::ExitCode, JxError> {
    let loaded = load(cli)?;
    let dependency_jars = compile_project(cli, &loaded)?;
    let feature = loaded.manifest.jdk_feature()?;
    let classes = loaded.manifest_dir.join("target").join("classes");
    let main_class = jx_run::decide_main(&classes, loaded.manifest.main_class())?;
    let jdk = ensure_jdk(cli, feature)?;
    if let Some(port) = debug_port {
        println!("jx run: waiting for a debugger on port {port}…");
    }
    let runner = jx_run::PlainMainRunner;
    let mut classpath = vec![classes.display().to_string()];
    classpath.extend(
        dependency_jars
            .iter()
            .map(|path| path.display().to_string()),
    );
    let spec = jx_run::RunSpec {
        java: jdk.java(),
        classpath,
        main_class,
        argv,
        debug_port,
    };
    let code = jx_run::Runner::run(&runner, &spec)?;
    Ok(std::process::ExitCode::from(code as u8))
}
