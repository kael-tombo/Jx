//! The jx command line. Command wiring lives here; behaviour lives in the
//! workspace crates behind the extension points listed in docs/README.md.

mod doctor;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};
use jx_common::color::{self, When};
use jx_common::error::{JxError, Kind};

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
small manifest.\n\nStatus: M0 (foundations). `jx doctor` and `--version` are live; \
the remaining commands arrive milestone by milestone and explain themselves until then."
)]
struct Cli {
    /// Auto, always or never colour stderr output. NO_COLOR is always honoured.
    #[arg(long, value_enum, default_value_t = ColorArg::Auto)]
    color: ColorArg,
    /// Work without network access (no effect yet; planned M2).
    #[arg(long)]
    offline: bool,
    /// Fail if the lock would change (no effect yet; planned M3).
    #[arg(long)]
    locked: bool,
    /// Path to project.toml (no effect yet).
    #[arg(long, value_name = "PATH")]
    manifest_path: Option<PathBuf>,
    /// Print full error cause chains.
    #[arg(short = 'v', long)]
    verbose: bool,
    /// Print less; reports only warnings and failures.
    #[arg(short = 'q', long)]
    quiet: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Scaffold a new jx project (planned M1)
    New { name: String },
    /// Initialise jx in an existing directory (planned M1)
    Init,
    /// Convert an existing Maven or Gradle build (planned M7)
    Import { path: Option<PathBuf> },
    /// Add a dependency to the manifest (planned M3)
    Add { dependency: String },
    /// Remove a dependency (planned M3)
    Remove { alias: String },
    /// Resolve and write jx.lock (planned M3)
    Lock,
    /// Download the pinned JDK and locked artifacts (planned M3)
    Fetch,
    /// Print the resolved dependency graph (planned M3)
    Tree,
    /// Explain why an artifact is in the graph (planned M3)
    Why { coord: String },
    /// Compile the project (planned M4)
    Build,
    /// Build and run the main class (planned M1, minimal)
    Run,
    /// Compile and run tests (planned M4)
    Test,
    /// Remove build outputs (planned M4)
    Clean,
    /// Manage JDK installations under ~/.jx/jdks (planned M1)
    #[command(subcommand)]
    Jdk(JdkCommand),
    /// Write an equivalent Maven or Gradle build (planned M5/M7)
    #[command(subcommand)]
    Export(ExportCommand),
    /// Check jx's environment: home, cache, network, Java, disk
    Doctor,
}

#[derive(Subcommand)]
enum JdkCommand {
    /// Download and install a JDK (planned M1)
    Install,
    /// List installed JDKs (planned M1)
    List,
    /// Pin a JDK in the manifest (planned M1)
    Use { version: String },
    /// Delete an installed JDK (planned M1)
    Remove { version: String },
}

#[derive(Subcommand)]
enum ExportCommand {
    /// Write an equivalent pom.xml (planned M5)
    Maven,
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
    // Global flags in scope for M0 are color/verbose/quiet. Confirm any others
    // are not silently ignored: name their milestone in the tiny warning.
    if cli.offline || cli.locked || cli.manifest_path.is_some() {
        eprintln!(
            "{}--offline / --locked / --manifest-path are planned for milestones M2-M3 \
and currently have no effect{}",
            if colors { "\x1b[33;1m" } else { "" },
            if colors { "\x1b[0m" } else { "" }
        );
    }
    match &cli.command {
        Command::Doctor => doctor::run(cli, colors),
        Command::New { .. } => Err(unimplemented("new", "M1", "run `jx doctor` meanwhile")),
        Command::Init => Err(unimplemented("init", "M1", "run `jx doctor` meanwhile")),
        Command::Import { .. } => Err(unimplemented(
            "import",
            "M7",
            "use Maven or Gradle until then",
        )),
        Command::Add { .. } => Err(unimplemented(
            "add",
            "M3",
            "edit pom.xml or build.gradle until then",
        )),
        Command::Remove { .. } => Err(unimplemented(
            "remove",
            "M3",
            "edit pom.xml or build.gradle until then",
        )),
        Command::Lock => Err(unimplemented(
            "lock",
            "M3",
            "use Maven or Gradle until then",
        )),
        Command::Fetch => Err(unimplemented(
            "fetch",
            "M3",
            "install a JDK manually until then",
        )),
        Command::Tree => Err(unimplemented(
            "tree",
            "M3",
            "see `mvn dependency:tree` / `gradle dependencies`",
        )),
        Command::Why { .. } => Err(unimplemented(
            "why",
            "M3",
            "see `mvn dependency:tree` / `gradle dependencies`",
        )),
        Command::Build | Command::Run | Command::Test | Command::Clean => Err(unimplemented(
            "build/run/test/clean",
            "M1-M4",
            "use Maven or Gradle until then",
        )),
        Command::Jdk(_) => Err(unimplemented(
            "jdk <command>",
            "M1",
            "install a JDK manually until then",
        )),
        Command::Export(_) => Err(unimplemented(
            "export <maven|gradle>",
            "M5/M7",
            "hand-write the build meanwhile",
        )),
    }
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
}
