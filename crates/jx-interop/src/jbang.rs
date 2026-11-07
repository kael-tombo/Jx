//! JBang script delegation.
//!
//! jx does not run Java scripts itself. `jx script` hands a `.java`, `.jsh`,
//! `.kt`, `.groovy` or markdown file to JBang, which owns script resolution,
//! `//DEPS` handling and its own script cache. The adapter is opt-in, names
//! JBang when it is missing, and returns JBang's exit code unchanged.
//!
//! When the script sits under a jx project, the CLI installs and exports that
//! project's pinned JDK so JBang runs against the same Java as the rest of the
//! build. Without a manifest, the adapter leaves JBang's own Java policy
//! alone (ADR 0015).

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use jx_common::error::{JxError, Kind};

use crate::delegate;

/// One script invocation. `args` are the arguments after the script path and
/// are passed to the script, not to jx.
pub struct Request<'a> {
    pub script: &'a Path,
    pub cwd: &'a Path,
    pub args: &'a [String],
    /// Pass JBang's global offline flag.
    pub offline: bool,
}

/// A resolved JBang invocation, ready to run once the JDK is available.
#[derive(Debug, Clone)]
pub struct Plan {
    program: PathBuf,
    cwd: PathBuf,
    args: Vec<String>,
}

impl Plan {
    /// A short command line for progress output.
    pub fn command_line(&self) -> String {
        let program = self
            .program
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.program.display().to_string());
        if self.args.is_empty() {
            program
        } else {
            format!("{program} {}", self.args.join(" "))
        }
    }
}

/// Resolve JBang and the script. Runs no process: the caller installs the
/// pinned JDK only after this succeeds.
pub fn plan(request: &Request<'_>) -> Result<Plan, JxError> {
    let override_path = std::env::var_os("JX_JBANG").map(PathBuf::from);
    plan_with(request, std::env::var_os("PATH").as_deref(), override_path)
}

fn plan_with(
    request: &Request<'_>,
    path: Option<&OsStr>,
    override_path: Option<PathBuf>,
) -> Result<Plan, JxError> {
    if !request.script.is_file() {
        return Err(JxError::new(Kind::Io, "the JBang script does not exist")
            .at(request.script.display().to_string())
            .hint(
                "pass a .java, .jsh, .kt, .groovy or markdown file; `jx script` delegates to JBang",
            ));
    }

    let program = match override_path {
        Some(program) if program.is_file() => program,
        Some(program) => {
            return Err(JxError::new(
                Kind::Io,
                "JX_JBANG does not name an existing executable",
            )
            .at(program.display().to_string())
            .hint("unset JX_JBANG to use `jbang` from PATH, or point it at the installed jbang executable"));
        }
        None => delegate::find_in_path("jbang", path).ok_or_else(|| {
            JxError::new(Kind::Build, "no JBang executable found")
                .at(request.cwd.display().to_string())
                .hint(format!(
                    "install JBang so `jbang` is on PATH with {} (https://www.jbang.dev/download/), or set JX_JBANG to its executable; jx delegates scripts instead of running them itself",
                    install_hint()
                ))
        })?,
    };

    let mut args = Vec::with_capacity(request.args.len() + 2);
    if request.offline {
        args.push("--offline".to_string());
    }
    args.push(request.script.to_string_lossy().into_owned());
    args.extend(request.args.iter().cloned());

    Ok(Plan {
        program,
        cwd: request.cwd.to_path_buf(),
        args,
    })
}

/// The native package-manager command for JBang on this platform. jx does
/// not install tools; naming the command keeps the fix one copy away.
pub fn install_hint() -> &'static str {
    if cfg!(windows) {
        "`winget install jbangdev.jbang`, `choco install jbang` or `scoop install jbang`"
    } else {
        "`brew install jbang`"
    }
}

/// Run a planned invocation. `java_home` is the project's pinned JDK when the
/// script is inside a jx project; `None` leaves JBang's Java choice untouched.
/// Returns JBang's exit code.
pub fn execute(plan: &Plan, java_home: Option<&Path>) -> Result<i32, JxError> {
    let mut command = delegate::command_for_program(&plan.program, &plan.args);
    command.current_dir(&plan.cwd);
    if let Some(java_home) = java_home {
        command.env("JAVA_HOME", java_home);
        command.env("PATH", delegate::path_with(java_home.join("bin"))?);
    }

    let status = command.status().map_err(|e| {
        JxError::new(Kind::Build, "cannot start JBang")
            .at(plan.program.display().to_string())
            .caused_by(e)
            .hint("check that `jbang --version` works from a shell, or set JX_JBANG to the executable")
    })?;
    Ok(status.code().unwrap_or(1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn script() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("script dir");
        fs::write(dir.path().join("hello.java"), "class Hello {}\n").expect("script");
        dir
    }

    fn request<'a>(dir: &'a Path, script: &'a Path, args: &'a [String]) -> Request<'a> {
        Request {
            script,
            cwd: dir,
            args,
            offline: false,
        }
    }

    #[test]
    fn override_is_used_and_offline_comes_before_the_script() {
        let dir = script();
        let tool = dir.path().join("jbang");
        fs::write(&tool, b"").expect("tool");
        let args = ["first".to_string(), "second".to_string()];
        let script = dir.path().join("hello.java");

        let mut request = request(dir.path(), &script, &args);
        request.offline = true;
        let plan = plan_with(&request, None, Some(tool.clone())).expect("override");

        assert_eq!(plan.program, tool);
        assert_eq!(
            plan.args,
            [
                "--offline",
                dir.path().join("hello.java").to_string_lossy().as_ref(),
                "first",
                "second"
            ]
        );
        assert!(plan.command_line().contains("hello.java first second"));
    }

    #[test]
    fn a_script_outside_a_project_is_still_resolved() {
        let dir = script();
        let name = if cfg!(windows) { "jbang.exe" } else { "jbang" };
        let tool = dir.path().join(name);
        fs::write(&tool, b"").expect("tool");
        let path = std::env::join_paths([dir.path()]).expect("path");
        let script = dir.path().join("hello.java");

        let plan =
            plan_with(&request(dir.path(), &script, &[]), Some(&path), None).expect("PATH lookup");
        assert_eq!(plan.program, tool);
        assert_eq!(plan.args, [script.to_string_lossy()]);
    }

    #[test]
    fn a_missing_script_is_named_before_tool_lookup() {
        let dir = script();
        let missing = dir.path().join("missing.java");
        let request = Request {
            script: &missing,
            cwd: dir.path(),
            args: &[],
            offline: false,
        };

        let error = plan_with(&request, None, None).expect_err("missing script");
        assert!(error.message.contains("does not exist"), "{error}");
        assert!(
            error
                .location
                .as_deref()
                .is_some_and(|p| p.ends_with("missing.java"))
        );
    }

    #[test]
    fn a_missing_jbang_names_the_install_path() {
        let dir = script();
        let script = dir.path().join("hello.java");
        let error = plan_with(
            &request(dir.path(), &script, &[]),
            Some(OsStr::new("")),
            None,
        )
        .expect_err("no jbang");

        assert!(error.message.contains("no JBang executable"), "{error}");
        assert!(
            error
                .hint
                .as_deref()
                .is_some_and(|hint| hint.contains("jbang.dev") && hint.contains("JX_JBANG")),
            "{error}"
        );
    }

    #[test]
    fn the_install_hint_names_the_native_package_manager() {
        let hint = install_hint();
        if cfg!(windows) {
            assert!(hint.contains("winget install jbangdev.jbang"), "{hint}");
            assert!(hint.contains("choco install jbang"), "{hint}");
            assert!(hint.contains("scoop install jbang"), "{hint}");
        } else {
            assert_eq!(hint, "`brew install jbang`");
        }
    }

    #[test]
    fn an_invalid_override_is_diagnosed() {
        let dir = script();
        let script = dir.path().join("hello.java");
        let error = plan_with(
            &request(dir.path(), &script, &[]),
            None,
            Some(dir.path().join("not-jbang")),
        )
        .expect_err("invalid override");

        assert!(error.message.contains("JX_JBANG"), "{error}");
        assert!(
            error
                .hint
                .as_deref()
                .is_some_and(|hint| hint.contains("PATH")),
            "{error}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn execute_exports_the_pinned_java_home_and_returns_the_tool_code() {
        use std::os::unix::fs::PermissionsExt;

        let dir = script();
        let log = dir.path().join("invocation.log");
        let tool = dir.path().join("jbang");
        fs::write(
            &tool,
            format!(
                "#!/bin/sh\nprintf '%s|%s|%s\\n' \"$JAVA_HOME\" \"$1\" \"$2\" > '{}'\nexit 7\n",
                log.display()
            ),
        )
        .expect("tool");
        let mut permissions = fs::metadata(&tool).expect("metadata").permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&tool, permissions).expect("executable");

        let plan = Plan {
            program: tool,
            cwd: dir.path().to_path_buf(),
            args: vec!["--offline".into(), "hello.java".into()],
        };
        let java_home = dir.path().join("jdk");
        fs::create_dir_all(&java_home).expect("java home");

        let code = execute(&plan, Some(&java_home)).expect("jbang runs");
        assert_eq!(code, 7);
        let logged = fs::read_to_string(&log).expect("log");
        assert!(logged.starts_with(&format!("{}|--offline|hello.java", java_home.display())));
    }
}
