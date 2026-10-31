//! `jx image`: a trimmed Java runtime built from the pinned JDK.
//!
//! `jdeps --print-module-deps` proposes a module set; `jlink` turns it into a
//! runtime under `target/dist/runtime`. If jdeps cannot inspect the project,
//! jx builds a full runtime with `ALL-MODULE-PATH` and reports why. The
//! generated runtime is verified by starting it with `--list-modules`; jx
//! never leaves a half-written image behind.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use jx_common::error::{JxError, Kind};

/// How the runtime was selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageMode {
    /// `jdeps` produced a module set and `jlink` used it.
    Trimmed,
    /// `--full` asked for every module in the pinned JDK.
    Full,
    /// `jdeps` failed or produced no modules; jx fell back to every module.
    FullFallback,
}

/// Everything needed to build a runtime image.
pub struct ImageRequest<'a> {
    /// The pinned JDK's `jdeps` executable.
    pub jdeps: &'a Path,
    /// The pinned JDK's `jlink` executable.
    pub jlink: &'a Path,
    /// Compiled main classes; `jdeps` reads these.
    pub classes_dir: &'a Path,
    /// The main dependency classpath, using the platform separator.
    pub classpath: &'a str,
    /// Where the runtime directory should be written.
    pub output_dir: &'a Path,
    /// The JDK feature release, also passed to `jdeps --multi-release`.
    pub feature: u32,
    /// Skip analysis and include every JDK module.
    pub full: bool,
}

/// The generated runtime and the decision that produced it.
#[derive(Debug)]
pub struct ImageOutput {
    pub runtime: PathBuf,
    pub mode: ImageMode,
    /// Sorted module names requested from jlink; empty for full runtimes.
    pub modules: Vec<String>,
    /// Present only for [`ImageMode::FullFallback`].
    pub fallback_reason: Option<String>,
}

/// Build the runtime atomically: jlink writes a temporary sibling, jx verifies
/// it, and only then does it replace the previous runtime.
pub fn build(request: &ImageRequest<'_>) -> Result<ImageOutput, JxError> {
    if request.feature < 9 {
        return Err(JxError::new(
            Kind::Toolchain,
            format!(
                "runtime images need JDK 9 or newer; this project pins JDK {}",
                request.feature
            ),
        )
        .hint("pin a newer JDK, or use `jx package` for a plain jar without a runtime"));
    }
    if let Some(parent) = request.output_dir.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            JxError::new(Kind::Io, "cannot create the runtime image directory")
                .at(parent.display().to_string())
                .caused_by(e)
        })?;
    }

    let mut fallback_reason = None;
    let modules = if request.full {
        Vec::new()
    } else {
        match trim_modules(request) {
            Ok(modules) if !modules.is_empty() => modules,
            Ok(_) => {
                fallback_reason = Some("jdeps produced no module list".to_string());
                Vec::new()
            }
            Err(reason) => {
                fallback_reason = Some(reason);
                Vec::new()
            }
        }
    };
    let mode = if request.full {
        ImageMode::Full
    } else if fallback_reason.is_some() {
        ImageMode::FullFallback
    } else {
        ImageMode::Trimmed
    };
    let added = if mode == ImageMode::Trimmed {
        modules.join(",")
    } else {
        "ALL-MODULE-PATH".to_string()
    };

    let temp = temp_dir(request.output_dir);
    remove_path(&temp)?;
    let result = run_jlink(request, &temp, &added).and_then(|()| verify_runtime(&temp, &modules));
    if let Err(error) = result {
        let _ = remove_path(&temp);
        return Err(error);
    }

    remove_path(request.output_dir)?;
    std::fs::rename(&temp, request.output_dir).map_err(|e| {
        JxError::new(Kind::Io, "cannot move the verified runtime into place")
            .at(request.output_dir.display().to_string())
            .caused_by(e)
            .hint("close any running image process, then retry")
    })?;
    Ok(ImageOutput {
        runtime: request.output_dir.to_path_buf(),
        mode,
        modules,
        fallback_reason,
    })
}

/// The `java` executable inside a generated runtime.
pub fn runtime_java(runtime: &Path) -> PathBuf {
    runtime.join("bin").join(exe_name("java"))
}

fn exe_name(tool: &str) -> String {
    if std::env::consts::OS == "windows" {
        format!("{tool}.exe")
    } else {
        tool.to_string()
    }
}

/// Ask jdeps for the module closure. Any failure is a reason to fall back to
/// a full runtime, not a hard jx failure.
fn trim_modules(request: &ImageRequest<'_>) -> Result<Vec<String>, String> {
    let mut command = Command::new(request.jdeps);
    command
        .arg("--print-module-deps")
        .arg("--ignore-missing-deps")
        .arg("--multi-release")
        .arg(request.feature.to_string());
    if !request.classpath.is_empty() {
        command.arg("--class-path").arg(request.classpath);
    }
    command.arg(request.classes_dir);
    let output = command
        .output()
        .map_err(|e| format!("cannot run {}: {e}", request.jdeps.display()))?;
    if !output.status.success() {
        let detail = first_line(&output.stderr);
        let code = output.status.code().map_or_else(
            || "terminated by signal".to_string(),
            |c| format!("exit code {c}"),
        );
        return Err(if detail.is_empty() {
            format!("jdeps failed ({code})")
        } else {
            format!("jdeps failed ({code}): {detail}")
        });
    }
    let modules = parse_module_deps(&String::from_utf8_lossy(&output.stdout));
    if modules.is_empty() {
        return Err("jdeps output did not contain a module list".to_string());
    }
    Ok(modules)
}

fn run_jlink(request: &ImageRequest<'_>, output: &Path, added: &str) -> Result<(), JxError> {
    let result = Command::new(request.jlink)
        .arg("--add-modules")
        .arg(added)
        .arg("--output")
        .arg(output)
        .arg("--strip-debug")
        .arg("--no-header-files")
        .arg("--no-man-pages")
        .output()
        .map_err(|e| {
            JxError::new(
                Kind::Toolchain,
                "the pinned JDK does not include a usable jlink",
            )
            .at(request.jlink.display().to_string())
            .caused_by(e)
            .hint("install a full JDK 9+; `jx jdk install` fetches one with jmods")
        })?;
    if result.status.success() {
        return Ok(());
    }
    let detail = first_line(&result.stderr);
    Err(JxError::new(
        Kind::Build,
        if detail.is_empty() {
            "jlink could not create the runtime image".to_string()
        } else {
            format!("jlink could not create the runtime image: {detail}")
        },
    )
    .at(output.display().to_string())
    .hint("check that the pinned JDK is intact, or use `jx package` without a runtime"))
}

/// Start the generated runtime and make sure every requested module is there.
/// This catches a successful-but-incomplete jlink run before it replaces a
/// working image.
fn verify_runtime(runtime: &Path, modules: &[String]) -> Result<(), JxError> {
    let java = runtime_java(runtime);
    if !java.is_file() {
        return Err(JxError::new(
            Kind::Build,
            "jlink reported success but the image has no java executable",
        )
        .at(java.display().to_string())
        .hint("retry with `jx image --full`; if it persists, the pinned JDK is damaged"));
    }
    let output = Command::new(&java)
        .arg("--list-modules")
        .output()
        .map_err(|e| {
            JxError::new(Kind::Build, "cannot start the generated runtime")
                .at(java.display().to_string())
                .caused_by(e)
                .hint("retry with `jx image --full`; the image may be incomplete")
        })?;
    if !output.status.success() {
        let detail = first_line(&output.stderr);
        return Err(JxError::new(
            Kind::Build,
            if detail.is_empty() {
                "the generated runtime failed to start".to_string()
            } else {
                format!("the generated runtime failed to start: {detail}")
            },
        )
        .at(java.display().to_string())
        .hint("retry with `jx image --full`; the trimmed runtime is incomplete"));
    }
    let available = parse_available_modules(&String::from_utf8_lossy(&output.stdout));
    let missing: Vec<&str> = modules
        .iter()
        .map(String::as_str)
        .filter(|module| !available.contains(*module))
        .collect();
    if !missing.is_empty() {
        return Err(JxError::new(
            Kind::Build,
            format!(
                "the generated runtime is missing requested module(s): {}",
                missing.join(", ")
            ),
        )
        .at(runtime.display().to_string())
        .hint("retry with `jx image --full`, or report the jdeps output as a jx bug"));
    }
    Ok(())
}

/// Parse `jdeps --print-module-deps` stdout. Warnings belong on stderr, but
/// scanning backwards for a valid module list also tolerates a tool that
/// writes warnings to stdout.
fn parse_module_deps(stdout: &str) -> Vec<String> {
    for line in stdout.lines().rev() {
        let mut modules = BTreeSet::new();
        let mut valid = true;
        for raw in line.split(',') {
            let module = raw.trim();
            if !valid_module_name(module) {
                valid = false;
                break;
            }
            modules.insert(module.to_string());
        }
        if valid && !modules.is_empty() {
            return modules.into_iter().collect();
        }
    }
    Vec::new()
}

/// Parse `java --list-modules`: `java.base@25.0.1` (the `@version` suffix is
/// omitted by some builds).
fn parse_available_modules(stdout: &str) -> BTreeSet<String> {
    stdout
        .lines()
        .filter_map(|line| line.trim().split('@').next())
        .filter(|module| !module.is_empty())
        .map(str::to_string)
        .collect()
}

fn valid_module_name(module: &str) -> bool {
    !module.is_empty()
        && !module.starts_with('.')
        && !module.ends_with('.')
        && !module.split('.').any(str::is_empty)
        && module
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
}

fn first_line(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("")
        .chars()
        .take(240)
        .collect()
}

fn temp_dir(output: &Path) -> PathBuf {
    let name = output.file_name().map_or_else(
        || "runtime".to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    output.with_file_name(format!(".{name}.{}.tmp", std::process::id()))
}

fn remove_path(path: &Path) -> Result<(), JxError> {
    match std::fs::metadata(path) {
        Ok(metadata) if metadata.is_dir() => std::fs::remove_dir_all(path).map_err(|e| {
            JxError::new(Kind::Io, "cannot remove an old runtime image")
                .at(path.display().to_string())
                .caused_by(e)
                .hint("close any running image process, then retry")
        }),
        Ok(_) => std::fs::remove_file(path).map_err(|e| {
            JxError::new(Kind::Io, "cannot remove an old runtime image")
                .at(path.display().to_string())
                .caused_by(e)
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(
            JxError::new(Kind::Io, "cannot inspect the runtime image path")
                .at(path.display().to_string())
                .caused_by(e),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_deps_are_sorted_and_deduplicated() {
        assert_eq!(
            parse_module_deps("java.sql,java.base,java.sql\r\n"),
            vec!["java.base", "java.sql"]
        );
    }

    #[test]
    fn warnings_do_not_become_modules() {
        assert!(parse_module_deps("Warning: something\n").is_empty());
        assert!(parse_module_deps("").is_empty());
        assert!(parse_module_deps("java.base,not a module").is_empty());
        assert_eq!(
            parse_module_deps("warning: ignored\njava.base\nwarning after\n"),
            vec!["java.base"]
        );
    }

    #[test]
    fn available_modules_drop_version_suffixes() {
        let modules = parse_available_modules("java.base@25.0.1\njava.sql\n");
        assert!(modules.contains("java.base"));
        assert!(modules.contains("java.sql"));
        assert_eq!(modules.len(), 2);
    }

    #[test]
    fn module_names_are_conservative() {
        assert!(valid_module_name("java.base"));
        assert!(valid_module_name("jdk.incubator.foreign"));
        assert!(!valid_module_name(""));
        assert!(!valid_module_name(".hidden"));
        assert!(!valid_module_name("java..base"));
        assert!(!valid_module_name("java/base"));
        assert!(!valid_module_name("ALL-MODULE-PATH"));
    }

    #[test]
    fn runtime_java_is_below_bin() {
        let path = runtime_java(Path::new("image"));
        assert_eq!(path.parent().unwrap(), Path::new("image/bin"));
        assert!(
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("java")
        );
    }

    #[test]
    fn requires_a_runtime_capable_jdk() {
        let request = ImageRequest {
            jdeps: Path::new("jdeps"),
            jlink: Path::new("jlink"),
            classes_dir: Path::new("classes"),
            classpath: "",
            output_dir: Path::new("runtime"),
            feature: 8,
            full: false,
        };
        let error = build(&request).expect_err("JDK 8 has no jlink");
        assert_eq!(error.kind, Kind::Toolchain);
        assert!(error.hint.unwrap().contains("jx package"));
    }
}
