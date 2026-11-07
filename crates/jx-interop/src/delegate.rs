//! Delegated builds: run the project's own Maven or Gradle build with the JDK
//! jx pinned.
//!
//! jx never evaluates an external build. It locates the tool (a project
//! wrapper first, then `PATH`), maps one lifecycle goal to that tool's own
//! goal, and runs it with `JAVA_HOME` and `PATH` pointing at the installed
//! JDK. Dependency resolution, plugins and resource processing stay with the
//! external build, which is why the CLI refuses jx's manifest-based dependency
//! commands in this mode (ADR 0012).

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command;

use jx_common::error::{JxError, Kind};

/// The external build tool a project delegates to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Engine {
    Maven,
    Gradle,
}

impl Engine {
    /// Human name used in progress and error messages.
    pub fn label(self) -> &'static str {
        match self {
            Engine::Maven => "Maven",
            Engine::Gradle => "Gradle",
        }
    }

    /// The executable to look for on `PATH` when the project has no wrapper.
    pub fn binary(self) -> &'static str {
        match self {
            Engine::Maven => "mvn",
            Engine::Gradle => "gradle",
        }
    }

    /// Wrapper names in preference order for this platform.
    fn wrapper_names(self) -> &'static [&'static str] {
        match self {
            Engine::Maven if cfg!(windows) => &["mvnw.cmd", "mvnw"],
            Engine::Maven => &["mvnw"],
            Engine::Gradle if cfg!(windows) => &["gradlew.bat", "gradlew"],
            Engine::Gradle => &["gradlew"],
        }
    }

    fn build_file(self) -> &'static str {
        match self {
            Engine::Maven => "pom.xml",
            Engine::Gradle => "build.gradle or build.gradle.kts",
        }
    }

    fn wrapper_advice(self) -> &'static str {
        match self {
            Engine::Maven => {
                "install Maven so `mvn` is on PATH, or add the wrapper with `mvn wrapper:wrapper`"
            }
            Engine::Gradle => {
                "install Gradle so `gradle` is on PATH, or add the wrapper with `gradle wrapper`"
            }
        }
    }

    fn export_command(self) -> &'static str {
        match self {
            Engine::Maven => "jx --engine jx export maven",
            Engine::Gradle => "jx --engine jx export gradle",
        }
    }
}

/// One lifecycle step jx can hand to the external build.
///
/// The mapping is deliberately narrow: `run`, `package` and `image` have no
/// engine-neutral equivalent (Maven has no built-in run goal; the produced
/// artifacts and layouts differ), so the CLI refuses them in delegate mode
/// instead of guessing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Goal {
    Build,
    Test,
    Clean,
}

impl Goal {
    fn task(self, engine: Engine) -> &'static str {
        match (self, engine) {
            (Goal::Build, Engine::Maven) => "compile",
            (Goal::Build, Engine::Gradle) => "classes",
            (Goal::Test, _) => "test",
            (Goal::Clean, _) => "clean",
        }
    }
}

/// What to delegate. The CLI has already chosen the engine from `[build]` or
/// `--engine`; this crate only resolves and describes the invocation.
pub struct Request<'a> {
    pub project_dir: &'a Path,
    pub engine: Engine,
    pub goal: Goal,
    /// Pass the build tool's own offline flag.
    pub offline: bool,
    /// Raw arguments after `--`, appended after the mapped goal.
    pub extra_args: &'a [String],
}

/// A resolved invocation, ready to run once the JDK is available.
#[derive(Debug, Clone)]
pub struct Plan {
    engine: Engine,
    project_dir: PathBuf,
    program: PathBuf,
    args: Vec<String>,
    wrapper: bool,
}

impl Plan {
    /// A short command line for progress output, e.g. `mvnw -B compile`.
    pub fn command_line(&self) -> String {
        let program = if self.wrapper {
            self.program
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| self.program.display().to_string())
        } else {
            self.engine.binary().to_string()
        };
        if self.args.is_empty() {
            program
        } else {
            format!("{program} {}", self.args.join(" "))
        }
    }
}

/// Resolve the tool, build file and arguments for one delegated goal.
///
/// Runs no process: the caller installs the pinned JDK only after this
/// succeeds, so a missing build file never triggers a download.
pub fn plan(request: &Request<'_>) -> Result<Plan, JxError> {
    plan_with_path(request, std::env::var_os("PATH").as_deref())
}

/// The testable form of [`plan`]: `PATH` is supplied instead of read.
fn plan_with_path(request: &Request<'_>, path: Option<&OsStr>) -> Result<Plan, JxError> {
    let project_dir = request.project_dir;
    check_build_file(project_dir, request.engine)?;

    let (program, wrapper) = match find_wrapper(project_dir, request.engine) {
        Some(wrapper) => (wrapper, true),
        None => (
            find_in_path(request.engine.binary(), path)
                .ok_or_else(|| missing_tool(project_dir, request.engine))?,
            false,
        ),
    };

    let mut args = Vec::with_capacity(3);
    match request.engine {
        Engine::Maven => {
            args.push("-B".to_string());
            if request.offline {
                args.push("-o".to_string());
            }
        }
        Engine::Gradle => {
            args.push("--console=plain".to_string());
            if request.offline {
                args.push("--offline".to_string());
            }
        }
    }
    args.push(request.goal.task(request.engine).to_string());
    args.extend(request.extra_args.iter().cloned());

    Ok(Plan {
        engine: request.engine,
        project_dir: project_dir.to_path_buf(),
        program,
        args,
        wrapper,
    })
}

/// Run a planned invocation with `JAVA_HOME` and a `PATH` that put the pinned
/// JDK first. Returns the build tool's exit code.
pub fn execute(plan: &Plan, java_home: &Path) -> Result<i32, JxError> {
    let mut command = command_for(plan);
    command.current_dir(&plan.project_dir);
    command.env("JAVA_HOME", java_home);
    command.env("PATH", path_with(java_home.join("bin"))?);

    let status = command.status().map_err(|e| {
        let hint = if plan.wrapper {
            "make the wrapper executable (`chmod +x mvnw` / `gradlew`), or install the build tool"
        } else {
            "check that the build tool starts from a shell; `jx doctor` reports JDK and PATH problems"
        };
        JxError::new(
            Kind::Build,
            format!("cannot start the delegated {} build", plan.engine.label()),
        )
        .at(plan.program.display().to_string())
        .caused_by(e)
        .hint(hint)
    })?;
    Ok(status.code().unwrap_or(1))
}

fn check_build_file(project_dir: &Path, engine: Engine) -> Result<(), JxError> {
    let present = match engine {
        Engine::Maven => project_dir.join("pom.xml").is_file(),
        Engine::Gradle => {
            project_dir.join("build.gradle.kts").is_file()
                || project_dir.join("build.gradle").is_file()
        }
    };
    if present {
        return Ok(());
    }
    let expected = match engine {
        Engine::Maven => project_dir.join("pom.xml"),
        Engine::Gradle => project_dir.join("build.gradle.kts"),
    };
    Err(JxError::new(
        Kind::Build,
        format!("no {} to delegate to", engine.build_file()),
    )
    .at(expected.display().to_string())
    .hint(format!(
        "create the build file, run `{}`, or set `[build] engine = \"jx\"`",
        engine.export_command()
    )))
}

fn find_wrapper(project_dir: &Path, engine: Engine) -> Option<PathBuf> {
    engine
        .wrapper_names()
        .iter()
        .map(|name| project_dir.join(name))
        .find(|candidate| candidate.is_file())
}

fn missing_tool(project_dir: &Path, engine: Engine) -> JxError {
    JxError::new(
        Kind::Build,
        format!("no {} executable or wrapper found", engine.label()),
    )
    .at(project_dir.display().to_string())
    .hint(format!(
        "{}; or set `[build] engine = \"jx\"` to build with jx",
        engine.wrapper_advice()
    ))
}

/// Find `name` in the given `PATH`. Windows needs the executable extensions
/// `Command` would otherwise not append; Unix uses the bare name.
pub fn find_in_path(name: &str, path: Option<&OsStr>) -> Option<PathBuf> {
    let extensions: &[&str] = if cfg!(windows) {
        &[".exe", ".cmd", ".bat"]
    } else {
        &[""]
    };
    for dir in std::env::split_paths(path?) {
        if dir.as_os_str().is_empty() {
            continue;
        }
        for extension in extensions {
            let candidate = dir.join(format!("{name}{extension}"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

pub(crate) fn path_with(java_bin: PathBuf) -> Result<OsString, JxError> {
    let mut entries = vec![java_bin];
    if let Some(existing) = std::env::var_os("PATH") {
        entries.extend(std::env::split_paths(&existing));
    }
    std::env::join_paths(entries)
        .map_err(|e| JxError::new(Kind::Io, "cannot build the delegated build's PATH").caused_by(e))
}

fn command_for(plan: &Plan) -> Command {
    command_for_program(&plan.program, &plan.args)
}

/// Build a `Command` for a tool path. Windows `.cmd`/`.bat` wrappers need
/// `cmd /C`, because `CreateProcess` cannot execute them directly.
pub(crate) fn command_for_program(program: &Path, args: &[String]) -> Command {
    if is_windows_batch(program) {
        let mut command = Command::new("cmd");
        command.arg("/C").arg(program);
        command.args(args);
        command
    } else {
        let mut command = Command::new(program);
        command.args(args);
        command
    }
}

fn is_windows_batch(path: &Path) -> bool {
    cfg!(windows)
        && path.extension().is_some_and(|extension| {
            extension.eq_ignore_ascii_case("cmd") || extension.eq_ignore_ascii_case("bat")
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;

    fn project() -> tempfile::TempDir {
        tempfile::tempdir().expect("project dir")
    }

    fn write(dir: &Path, name: &str) {
        fs::write(dir.join(name), b"").expect("fixture file");
    }

    fn request<'a>(dir: &'a Path, engine: Engine, goal: Goal) -> Request<'a> {
        Request {
            project_dir: dir,
            engine,
            goal,
            offline: false,
            extra_args: &[],
        }
    }

    #[test]
    fn maven_build_maps_to_compile_and_prefers_the_wrapper() {
        let dir = project();
        write(dir.path(), "pom.xml");
        write(dir.path(), "mvnw");

        let plan = plan_with_path(&request(dir.path(), Engine::Maven, Goal::Build), None)
            .expect("wrapper is enough");
        assert_eq!(plan.args, ["-B", "compile"]);
        assert!(plan.wrapper);
        assert_eq!(plan.command_line(), "mvnw -B compile");
    }

    #[test]
    fn gradle_build_maps_to_classes_and_passes_offline() {
        let dir = project();
        write(dir.path(), "build.gradle.kts");
        write(dir.path(), "gradlew");

        let mut request = request(dir.path(), Engine::Gradle, Goal::Build);
        request.offline = true;
        let plan = plan_with_path(&request, None).expect("wrapper is enough");
        assert_eq!(plan.args, ["--console=plain", "--offline", "classes"]);
        assert_eq!(
            plan.command_line(),
            "gradlew --console=plain --offline classes"
        );
    }

    #[test]
    fn tool_arguments_are_forwarded_after_the_mapped_goal() {
        let dir = project();
        write(dir.path(), "pom.xml");
        write(dir.path(), "mvnw");
        write(dir.path(), "build.gradle.kts");
        write(dir.path(), "gradlew");

        let maven_args = vec!["-DskipTests".to_string(), "-Pfast".to_string()];
        let mut maven_request = request(dir.path(), Engine::Maven, Goal::Build);
        maven_request.extra_args = &maven_args;
        let plan = plan_with_path(&maven_request, None).expect("maven wrapper");
        assert_eq!(plan.args, ["-B", "compile", "-DskipTests", "-Pfast"]);

        let gradle_args = vec!["--tests".to_string(), "app.LibTest".to_string()];
        let mut gradle_request = request(dir.path(), Engine::Gradle, Goal::Test);
        gradle_request.extra_args = &gradle_args;
        let plan = plan_with_path(&gradle_request, None).expect("gradle wrapper");
        assert_eq!(
            plan.args,
            ["--console=plain", "test", "--tests", "app.LibTest"]
        );
    }

    #[test]
    fn test_and_clean_map_to_each_engines_own_goal() {
        let dir = project();
        write(dir.path(), "pom.xml");
        write(dir.path(), "build.gradle");
        write(dir.path(), "mvnw");
        write(dir.path(), "gradlew");

        let maven_test = plan_with_path(&request(dir.path(), Engine::Maven, Goal::Test), None)
            .expect("maven test");
        assert_eq!(maven_test.args, ["-B", "test"]);
        let maven_clean = plan_with_path(&request(dir.path(), Engine::Maven, Goal::Clean), None)
            .expect("maven clean");
        assert_eq!(maven_clean.args, ["-B", "clean"]);

        let gradle_test = plan_with_path(&request(dir.path(), Engine::Gradle, Goal::Test), None)
            .expect("gradle test");
        assert_eq!(gradle_test.args, ["--console=plain", "test"]);
        let gradle_clean = plan_with_path(&request(dir.path(), Engine::Gradle, Goal::Clean), None)
            .expect("gradle clean");
        assert_eq!(gradle_clean.args, ["--console=plain", "clean"]);
    }

    #[test]
    fn a_missing_build_file_is_named_before_any_tool_lookup() {
        let dir = project();
        let error = plan_with_path(&request(dir.path(), Engine::Maven, Goal::Build), None)
            .expect_err("no pom");
        assert!(error.message.contains("pom.xml"), "{error}");
        assert!(
            error
                .hint
                .as_ref()
                .is_some_and(|hint| hint.contains("export maven")),
            "{error}"
        );

        let error = plan_with_path(&request(dir.path(), Engine::Gradle, Goal::Build), None)
            .expect_err("no gradle build");
        assert!(error.message.contains("build.gradle"), "{error}");
        assert!(
            error
                .hint
                .as_ref()
                .is_some_and(|hint| hint.contains("export gradle")),
            "{error}"
        );
    }

    #[test]
    fn a_missing_tool_is_diagnosed_with_install_advice() {
        let dir = project();
        write(dir.path(), "pom.xml");

        let error = plan_with_path(
            &request(dir.path(), Engine::Maven, Goal::Build),
            Some(OsStr::new("")),
        )
        .expect_err("no mvn and no wrapper");
        assert!(error.message.contains("no Maven executable"), "{error}");
        assert!(
            error
                .hint
                .as_ref()
                .is_some_and(|hint| hint.contains("install Maven")),
            "{error}"
        );
    }

    #[test]
    fn path_lookup_uses_the_platforms_executable_names() {
        let bin = project();
        let name = if cfg!(windows) { "mvn.cmd" } else { "mvn" };
        write(bin.path(), name);
        let path = std::env::join_paths([bin.path()]).expect("path");

        let found = find_in_path("mvn", Some(&path)).expect("mvn found");
        assert_eq!(found, bin.path().join(name));
        assert!(find_in_path("gradle", Some(&path)).is_none());
    }

    #[test]
    fn windows_batch_detection_only_matches_batch_files() {
        assert!(!is_windows_batch(Path::new("mvnw")));
        assert!(!is_windows_batch(Path::new("mvnw.sh")));
        assert_eq!(
            is_windows_batch(Path::new("mvnw.cmd")),
            cfg!(windows),
            "only Windows needs cmd.exe to run .cmd wrappers"
        );
    }
}
