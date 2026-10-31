//! The jx command line. Command wiring lives here; behaviour lives in the
//! workspace crates behind the extension points listed in docs/README.md.

mod build;
mod deps;
mod doctor;
mod scaffold;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};
use jx_common::color::{self, When};
use jx_common::error::{JxError, Kind};
use jx_manifest as manifest;
use jx_toolchain as toolchain;

#[derive(ValueEnum, Clone, Copy, Debug)]
#[value(name = "color")]
enum ColorArg {
    Auto,
    Always,
    Never,
}

impl From<ColorArg> for When {
    fn from(v: ColorArg) -> Self {
        match v {
            ColorArg::Auto => When::Auto,
            ColorArg::Always => When::Always,
            ColorArg::Never => When::Never,
        }
    }
}

#[derive(Parser)]
#[command(
    name = "jx",
    version,
    about = "A Cargo/uv-style front door for Java",
    long_about = "jx manages JDKs, resolves dependencies from Maven Central into a \
reproducible lockfile, and builds, tests, runs and packages Java projects from one \
small manifest.\n\nStatus: M6 (runtime images). Live: jx jdk <command>, jx new/init, \
jx add/remove, jx lock/fetch, jx tree/why, jx build/test/run/package/image, \
jx export maven and jx doctor. Maven/Gradle import and Gradle export are \
still ahead."
)]
struct Cli {
    /// Auto, always or never colour stderr output. NO_COLOR is always honoured.
    #[arg(long, value_enum, default_value_t = ColorArg::Auto, global = true)]
    color: ColorArg,
    /// Work without network access (installs refuse; cached work continues).
    #[arg(long, global = true)]
    offline: bool,
    /// Fail if resolving would change jx.lock
    #[arg(long, global = true)]
    locked: bool,
    /// Path to project.toml.
    #[arg(long, value_name = "PATH", global = true)]
    manifest_path: Option<PathBuf>,
    /// Print full error cause chains.
    #[arg(short = 'v', long, global = true)]
    verbose: bool,
    /// Print less; reports only warnings and failures.
    #[arg(short = 'q', long, global = true)]
    quiet: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Scaffold a new jx project: app by default, `--lib` for a library
    New {
        name: String,
        #[arg(long)]
        lib: bool,
    },
    /// Initialise jx in an existing directory
    Init,
    /// Convert an existing Maven or Gradle build (planned M7)
    Import { path: Option<PathBuf> },
    /// Add a dependency, then update jx.lock
    Add {
        /// group:artifact[:version] (a missing version picks the latest stable release)
        dependency: String,
        /// Add to [dev-dependencies]
        #[arg(long)]
        dev: bool,
        /// Add a `{ platform = … }` BOM entry instead of a normal dependency
        #[arg(long)]
        platform: bool,
        /// Local alias (default: the artifact id)
        #[arg(long, value_name = "NAME")]
        alias: Option<String>,
    },
    /// Remove a dependency, then update jx.lock
    Remove {
        /// The local alias from project.toml
        alias: String,
        /// Remove from [dev-dependencies] instead of [dependencies]
        #[arg(long)]
        dev: bool,
    },
    /// Resolve the graph and write jx.lock
    Lock,
    /// Download and verify the artifacts pinned in jx.lock
    Fetch,
    /// Print the resolved dependency graph
    Tree,
    /// Explain why an artifact is in the graph
    Why { coord: String },
    /// Compile with the pinned JDK
    Build,
    /// Build and run the main class; `jx run -- args` passes them on
    Run {
        /// Suspend in a JDWP server until a debugger attaches
        #[arg(long)]
        debug: bool,
        /// JDWP port for --debug (default 5005)
        #[arg(long, value_name = "N")]
        port: Option<u16>,
        /// Arguments after a bare `--` go to the program
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Compile main and test sources, then run JUnit 5
    Test,
    /// Package classes into a jar and target/dist
    Package {
        /// Merge dependencies into one runnable jar instead of a thin jar + lib/
        #[arg(long)]
        fat: bool,
    },
    /// Build a runtime image with jdeps + jlink under target/dist/runtime
    Image {
        /// Include every JDK module instead of trimming to the app's needs
        #[arg(long)]
        full: bool,
    },
    /// Remove target/
    Clean,
    /// Manage JDK installations under ~/.jx/jdks
    #[command(subcommand)]
    Jdk(JdkCommand),
    /// Write an equivalent Maven or Gradle build (Maven now; Gradle M7)
    #[command(subcommand)]
    Export(ExportCommand),
    /// Check jx's environment: home, cache, network, Java, disk
    Doctor,
}

#[derive(Subcommand)]
enum JdkCommand {
    /// Download and install Eclipse Temurin (default: the verified current LTS)
    Install {
        /// Feature release number, e.g. 25
        feature: Option<u32>,
    },
    /// List installed jx JDKs
    List,
    /// Pin a feature release in this project's manifest
    Use { version: String },
    /// Delete an installed JDK (jx's own store only)
    Remove { version: String },
}

#[derive(Subcommand)]
enum ExportCommand {
    /// Write a pom.xml from project.toml and the locked versions
    Maven {
        /// Maven groupId for the generated project
        #[arg(long, default_value = "local.jx", value_name = "ID")]
        group_id: String,
        /// Overwrite an existing pom.xml
        #[arg(long)]
        force: bool,
    },
    /// Write build.gradle.kts and settings.gradle.kts (planned M7)
    Gradle,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let when = cli.color.into();
    match run(&cli, color::enabled(when)) {
        Ok(code) => code,
        Err(e) => {
            if !cli.quiet {
                eprintln!("{}", e.render(when, cli.verbose));
            }
            ExitCode::from(e.exit_code() as u8)
        }
    }
}

fn run(cli: &Cli, colors: bool) -> Result<ExitCode, JxError> {
    match &cli.command {
        Command::Doctor => doctor::run(cli, colors),
        Command::New { name, lib } => scaffold::new_project(name, *lib),
        Command::Init => scaffold::init_project(),
        Command::Build => build::build(cli).map(|()| ExitCode::SUCCESS),
        Command::Run { debug, port, args } => build::run(
            cli,
            if *debug {
                Some(port.unwrap_or(5005))
            } else {
                None
            },
            args.clone(),
        ),
        Command::Jdk(jdk) => run_jdk(cli, jdk).map(|()| ExitCode::SUCCESS),
        Command::Clean => clean_project(),
        Command::Import { .. } => Err(unimplemented(
            "import",
            "M7",
            "use Maven or Gradle until then",
        )),
        Command::Add {
            dependency,
            dev,
            platform,
            alias,
        } => deps::add(cli, dependency, *dev, *platform, alias.clone()).map(|()| ExitCode::SUCCESS),
        Command::Remove { alias, dev } => {
            deps::remove(cli, alias, *dev).map(|()| ExitCode::SUCCESS)
        }
        Command::Lock => deps::lock(cli).map(|()| ExitCode::SUCCESS),
        Command::Fetch => deps::fetch(cli).map(|()| ExitCode::SUCCESS),
        Command::Tree => deps::tree(cli).map(|()| ExitCode::SUCCESS),
        Command::Why { coord } => deps::why(cli, coord).map(|()| ExitCode::SUCCESS),
        Command::Test => build::test(cli),
        Command::Package { fat } => build::package(cli, *fat).map(|()| ExitCode::SUCCESS),
        Command::Image { full } => build::image(cli, *full).map(|()| ExitCode::SUCCESS),
        Command::Export(ExportCommand::Maven { group_id, force }) => {
            build::export_maven(cli, group_id, *force).map(|()| ExitCode::SUCCESS)
        }
        Command::Export(ExportCommand::Gradle) => Err(unimplemented(
            "export gradle",
            "M7",
            "hand-write the Gradle build meanwhile",
        )),
    }
}

fn run_jdk(cli: &Cli, jdk: &JdkCommand) -> Result<(), JxError> {
    let home = toolchain::default_home()?;
    match jdk {
        JdkCommand::Install { feature } => {
            let feature = feature.unwrap_or(toolchain::DEFAULT_FEATURE);
            let installed = toolchain::ensure_jdk(&home, feature)?;
            println!(
                "jx jdk install: temurin {} ready under {}",
                installed.version,
                installed.dir.display()
            );
            Ok(())
        }
        JdkCommand::List => {
            let installed = toolchain::list_installed(&home)?;
            if installed.is_empty() {
                println!(
                    "no jx-managed JDKs installed; `jx jdk install` fetches temurin {}",
                    toolchain::DEFAULT_FEATURE
                );
            } else {
                println!("installed JDKs under {}:", home.display());
                for j in installed {
                    println!("  temurin {}", j.version);
                }
            }
            Ok(())
        }
        JdkCommand::Use { version } => {
            let feature = parse_feature(version)?;
            let path = locate_manifest(cli)?;
            manifest::set_jdk(&path, feature)?;
            println!("project.toml: jdk = \"{feature}\"");
            Ok(())
        }
        JdkCommand::Remove { version } => {
            toolchain::remove_installed(&home, version)?;
            println!("removed temurin {version}");
            Ok(())
        }
    }
}

pub(crate) fn parse_feature(version: &str) -> Result<u32, JxError> {
    version
        .parse::<u32>()
        .map_err(|_| {
            JxError::new(
                Kind::Usage,
                format!("jdk version `{version}` is not a plain feature release"),
            )
            .hint("V1 pins feature releases, e.g. `25` or `21`")
        })
        .and_then(|f| {
            (8..=99).contains(&f).then_some(f).ok_or_else(|| {
                JxError::new(Kind::Usage, "feature releases are between 8 and 99")
                    .hint(format!("no JDK {version} exists"))
            })
        })
}

/// The manifest to operate on: --manifest-path, else a parent-walk.
fn locate_manifest(cli: &Cli) -> Result<PathBuf, JxError> {
    if let Some(p) = &cli.manifest_path {
        if !p.is_file() {
            return Err(JxError::new(
                Kind::Manifest,
                "--manifest-path does not name an existing file",
            )
            .at(p.display().to_string()));
        }
        return Ok(p.clone());
    }
    let cwd = std::env::current_dir()
        .map_err(|e| JxError::new(Kind::Io, "cannot read the current directory").caused_by(e))?;
    manifest::find_manifest(&cwd)
}

fn clean_project() -> Result<ExitCode, JxError> {
    let here = std::env::current_dir()
        .map_err(|e| JxError::new(Kind::Io, "cannot read the current directory").caused_by(e))?;
    let target = here.join("target");
    if target.is_dir() {
        std::fs::remove_dir_all(&target).map_err(|e| {
            JxError::new(Kind::Io, "cannot remove target/")
                .at(target.display().to_string())
                .caused_by(e)
        })?;
    }
    Ok(ExitCode::SUCCESS)
}

fn unimplemented(cmd: &str, milestone: &str, interim: &str) -> JxError {
    JxError::new(
        Kind::Usage,
        format!("`jx {cmd}` is not implemented yet (planned for milestone {milestone})"),
    )
    .hint(format!("{interim}; see docs/README.md for the roadmap"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unimplemented_errors_carry_milestones() {
        let e = unimplemented("new", "M1", "hint");
        assert!(format!("{e}").contains("milestone M1"));
        assert!(
            e.hint
                .as_deref()
                .is_some_and(|h| h.contains("docs/README.md"))
        );
    }

    #[test]
    fn feature_parse_is_strict() {
        assert_eq!(parse_feature("25").unwrap(), 25);
        assert!(parse_feature("7").is_err());
        assert!(parse_feature("25.0").is_err());
        assert!(parse_feature("abc").is_err());
        assert!(parse_feature("1000").is_err());
    }
}
