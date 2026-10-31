//! CLI orchestration of build, test, run and package: manifest → classpaths →
//! JDK → javac → (optionally) JUnit, the main-class launch or a dist tree.
//! Entrypoint discovery lives in `jx-run`, compilation and fingerprints in
//! `jx-build`, jar and dist assembly in `jx-package`.

use std::path::PathBuf;
use std::process::{Command, ExitCode};

use jx_common::error::{JxError, Kind};
use jx_manifest as manifest;
use jx_toolchain as toolchain;

use crate::deps::{self, Classpaths};
use crate::{Cli, locate_manifest};

struct Loaded {
    manifest_dir: PathBuf,
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

fn target_dir(loaded: &Loaded) -> PathBuf {
    loaded.manifest_dir.join("target")
}

fn build_cache(loaded: &Loaded) -> PathBuf {
    target_dir(loaded).join("jx-cache")
}

/// Compile `src/main/java` and copy resources. Returns the classes dir and
/// whether javac actually ran; tests must recompile whenever it did.
fn compile_main(
    loaded: &Loaded,
    jdk: &toolchain::InstalledJdk,
    deps: &Classpaths,
) -> Result<(PathBuf, bool), JxError> {
    let feature = loaded.manifest.jdk_feature()?;
    let classes = target_dir(loaded).join("classes");
    let compiled = jx_build::compile_if_stale(&jx_build::CompileRequest {
        javac: &jdk.javac(),
        feature,
        sources_root: &loaded.manifest_dir.join("src/main/java"),
        resources_root: Some(&loaded.manifest_dir.join("src/main/resources")),
        out: &classes,
        classpath: &deps.main,
        cache_dir: &build_cache(loaded).join("main-compile"),
        label: "compile",
        force: false,
    })?;
    let n = jx_build::copy_resources(&loaded.manifest_dir.join("src/main/resources"), &classes)?;
    if n > 0 {
        println!("  copied {n} resources");
    }
    Ok((classes, compiled))
}

/// Compile `src/test/java` against the main classes and every dependency.
/// Recompiled when the tests changed or main sources were recompiled.
fn compile_tests(
    loaded: &Loaded,
    jdk: &toolchain::InstalledJdk,
    deps: &Classpaths,
    main_classes: &std::path::Path,
    main_recompiled: bool,
) -> Result<PathBuf, JxError> {
    let source_root = loaded.manifest_dir.join("src/test/java");
    if !source_root.is_dir() {
        return Err(JxError::new(
            Kind::Build,
            format!("no test sources under {}", source_root.display()),
        )
        .hint("put tests in src/test/java (JUnit 5), then run `jx test` again"));
    }
    let feature = loaded.manifest.jdk_feature()?;
    let classes = target_dir(loaded).join("test-classes");
    let mut classpath = Vec::with_capacity(deps.test.len() + 1);
    classpath.push(main_classes.to_path_buf());
    classpath.extend(deps.test.iter().cloned());
    jx_build::compile_if_stale(&jx_build::CompileRequest {
        javac: &jdk.javac(),
        feature,
        sources_root: &source_root,
        resources_root: Some(&loaded.manifest_dir.join("src/test/resources")),
        out: &classes,
        classpath: &classpath,
        cache_dir: &build_cache(loaded).join("test-compile"),
        label: "test compile",
        force: main_recompiled,
    })?;
    let n = jx_build::copy_resources(&loaded.manifest_dir.join("src/test/resources"), &classes)?;
    if n > 0 {
        println!("  copied {n} test resources");
    }
    Ok(classes)
}

fn junit_missing() -> JxError {
    JxError::new(
        Kind::Build,
        "the JUnit Platform console launcher is not on the test classpath",
    )
    .hint(r#"run `jx add --dev org.junit.platform:junit-platform-console-standalone`; it bundles the Jupiter engine"#)
}

/// `jx build`
pub fn build(cli: &Cli) -> Result<(), JxError> {
    let loaded = load(cli)?;
    let deps = deps::classpaths(cli)?;
    let feature = loaded.manifest.jdk_feature()?;
    let jdk = ensure_jdk(cli, feature)?;
    compile_main(&loaded, &jdk, &deps)?;
    Ok(())
}

/// `jx test`: compile main and tests, then scan the test classpath with the
/// JUnit Platform console launcher. Its exit code becomes jx's exit code, so
/// a failing test suite fails CI without a second wrapper.
pub fn test(cli: &Cli) -> Result<ExitCode, JxError> {
    let loaded = load(cli)?;
    let deps = deps::classpaths(cli)?;
    let feature = loaded.manifest.jdk_feature()?;
    let jdk = ensure_jdk(cli, feature)?;
    let (main_classes, main_recompiled) = compile_main(&loaded, &jdk, &deps)?;
    let test_classes = compile_tests(&loaded, &jdk, &deps, &main_classes, main_recompiled)?;
    let launcher = deps.junit_launcher.clone().ok_or_else(junit_missing)?;

    let mut classpath = vec![test_classes.display().to_string()];
    classpath.push(main_classes.display().to_string());
    classpath.extend(deps.test.iter().map(|path| path.display().to_string()));
    let status = Command::new(jdk.java())
        .arg("-jar")
        .arg(&launcher)
        .arg("--class-path")
        .arg(classpath.join(jx_run::classpath_separator()))
        .arg("--scan-class-path")
        .arg("--fail-if-no-tests")
        .arg("--disable-banner")
        .status()
        .map_err(|e| {
            JxError::new(Kind::Build, "cannot launch the JUnit console launcher")
                .at(launcher.display().to_string())
                .caused_by(e)
                .hint("run `jx doctor`; the JDK install may be damaged")
        })?;
    let code = status.code().unwrap_or(1);
    Ok(ExitCode::from(code as u8))
}

/// The optional main class for packaging: pinned, discovered when unique,
/// absent for a library, and an error when discovery is ambiguous.
fn package_main(
    classes: &std::path::Path,
    pinned: Option<&String>,
) -> Result<Option<String>, JxError> {
    if let Some(pinned) = pinned {
        return Ok(Some(pinned.clone()));
    }
    let candidates = jx_run::discover_main(classes)?;
    match candidates.len() {
        0 => Ok(None),
        1 => Ok(candidates.into_iter().next()),
        _ => jx_run::decide_main(classes, None).map(Some),
    }
}

/// `jx package`: compile main sources, then write a thin `target/dist` or a
/// single fat jar with the locked main dependencies merged in.
pub fn package(cli: &Cli, fat: bool) -> Result<(), JxError> {
    let loaded = load(cli)?;
    let deps = deps::classpaths(cli)?;
    let feature = loaded.manifest.jdk_feature()?;
    let jdk = ensure_jdk(cli, feature)?;
    let (classes, _) = compile_main(&loaded, &jdk, &deps)?;
    let main_class = package_main(&classes, loaded.manifest.main_class())?;
    let base_jar = jx_package::jar_file_name(&loaded.manifest.name, &loaded.manifest.version)?;
    let jar_name = if fat {
        format!(
            "{}-all.jar",
            base_jar
                .strip_suffix(".jar")
                .expect("jar_file_name ends in .jar")
        )
    } else {
        base_jar
    };

    let mut relative_paths = Vec::new();
    if !fat {
        relative_paths.reserve(deps.main_jars.len());
        for jar in &deps.main_jars {
            let file_name = jx_package::artifact_file_name(
                &jar.artifact_id,
                &jar.version,
                jar.classifier.as_deref(),
            )?;
            relative_paths.push(jx_package::dependency_relative_path(
                &jar.group_id,
                &file_name,
            )?);
        }
    }
    let dependencies: Vec<jx_package::DependencyJar<'_>> = deps
        .main_jars
        .iter()
        .enumerate()
        .map(|(index, jar)| jx_package::DependencyJar {
            relative_path: relative_paths.get(index).map_or("", String::as_str),
            source: &jar.path,
        })
        .collect();
    let dist = target_dir(&loaded).join("dist");
    let output = jx_package::package(&jx_package::PackageRequest {
        mode: if fat {
            jx_package::PackageMode::Fat
        } else {
            jx_package::PackageMode::Thin
        },
        dist_dir: &dist,
        classes_dir: &classes,
        jar_name: &jar_name,
        main_class: main_class.as_deref(),
        dependencies: &dependencies,
    })?;
    let display = output
        .jar
        .strip_prefix(&loaded.manifest_dir)
        .unwrap_or(&output.jar);
    let plural = if output.dependency_count == 1 {
        "y"
    } else {
        "ies"
    };
    if fat {
        println!(
            "packaged {} ({} dependenc{plural} merged)",
            display.display(),
            output.dependency_count
        );
        if output.duplicate_entries > 0 {
            println!(
                "  fat jar: kept the first of {} duplicate entries",
                output.duplicate_entries
            );
        }
        if output.versioned_entries > 0 {
            println!(
                "  fat jar: skipped {} multi-release entries",
                output.versioned_entries
            );
        }
        if output.unsafe_entries > 0 {
            println!(
                "  fat jar: refused {} unsafe entry names",
                output.unsafe_entries
            );
        }
    } else {
        println!(
            "packaged {} ({} dependenc{plural})",
            display.display(),
            output.dependency_count
        );
    }
    if main_class.is_some() {
        println!("  run: java -jar {}", output.jar.display());
    } else {
        println!("  library jar (no Main-Class)");
    }
    Ok(())
}

/// Map one manifest declaration to a POM dependency using its locked
/// version. Platform BOMs are skipped: their effect is already baked into the
/// concrete versions of every other dependency.
fn maven_dependencies(
    specs: &[manifest::DependencySpec],
    jars: &[deps::LockedJar],
    scope: Option<&'static str>,
) -> Result<Vec<jx_package::maven::MavenDependency>, JxError> {
    let mut out = Vec::new();
    for spec in specs {
        if spec.platform {
            continue;
        }
        let jar = jars
            .iter()
            .find(|jar| {
                jar.group_id == spec.group_id
                    && jar.artifact_id == spec.artifact_id
                    && jar.classifier.as_deref() == spec.classifier.as_deref()
            })
            .ok_or_else(|| {
                JxError::new(
                    Kind::Resolve,
                    format!(
                        "{}:{} has no locked artifact to export",
                        spec.group_id, spec.artifact_id
                    ),
                )
                .hint("run `jx lock` to refresh jx.lock, then export again")
            })?;
        let mut dependency =
            jx_package::maven::MavenDependency::new(&jar.group_id, &jar.artifact_id, &jar.version);
        if let Some(classifier) = &jar.classifier {
            dependency = dependency.classifier(classifier);
        }
        if scope == Some("test") {
            dependency = dependency.test_scope();
        }
        out.push(dependency);
    }
    Ok(out)
}

/// `jx export maven`: compile once (so a discovered main class is included),
/// then write a POM that mirrors project.toml and the locked versions.
pub fn export_maven(cli: &Cli, group_id: &str, force: bool) -> Result<(), JxError> {
    let loaded = load(cli)?;
    let path = loaded.manifest_dir.join("pom.xml");
    if path.exists() && !force {
        return Err(JxError::new(Kind::Io, "pom.xml already exists")
            .at(path.display().to_string())
            .hint("move it aside, or rerun with `jx export maven --force` to overwrite"));
    }
    let deps = deps::classpaths(cli)?;
    let feature = loaded.manifest.jdk_feature()?;
    let jdk = ensure_jdk(cli, feature)?;
    let (classes, _) = compile_main(&loaded, &jdk, &deps)?;
    let main_class = package_main(&classes, loaded.manifest.main_class())?;

    let mut dependencies = maven_dependencies(&loaded.manifest.dependencies, &deps.all_jars, None)?;
    dependencies.extend(maven_dependencies(
        &loaded.manifest.dev_dependencies,
        &deps.all_jars,
        Some("test"),
    )?);
    let mut overrides = Vec::new();
    for spec in &loaded.manifest.overrides {
        if let Some(jar) = deps.all_jars.iter().find(|jar| {
            jar.group_id == spec.group_id
                && jar.artifact_id == spec.artifact_id
                && jar.classifier.is_none()
        }) {
            overrides.push(jx_package::maven::MavenDependency::new(
                &jar.group_id,
                &jar.artifact_id,
                &jar.version,
            ));
        }
    }

    let main_count = dependencies
        .iter()
        .filter(|dependency| dependency.scope.is_none())
        .count();
    let test_count = dependencies.len() - main_count;
    let managed_count = overrides.len();
    let project = jx_package::maven::MavenProject {
        group_id: group_id.to_string(),
        artifact_id: loaded.manifest.name.clone(),
        version: loaded.manifest.version.clone(),
        java_release: feature,
        main_class,
        dependencies,
        overrides,
    };
    let pom = jx_package::maven::pom(&project)?;
    std::fs::write(&path, pom).map_err(|e| {
        JxError::new(Kind::Io, "cannot write pom.xml")
            .at(path.display().to_string())
            .caused_by(e)
    })?;
    println!("exported pom.xml ({main_count} main, {test_count} test, {managed_count} managed)");
    println!("  build with: mvn -f pom.xml package");
    Ok(())
}

/// `jx run [-- args]`, with optional `--debug [--port N]`.
pub fn run(cli: &Cli, debug_port: Option<u16>, argv: Vec<String>) -> Result<ExitCode, JxError> {
    let loaded = load(cli)?;
    let deps = deps::classpaths(cli)?;
    let feature = loaded.manifest.jdk_feature()?;
    let jdk = ensure_jdk(cli, feature)?;
    let (classes, _) = compile_main(&loaded, &jdk, &deps)?;
    let main_class = jx_run::decide_main(&classes, loaded.manifest.main_class())?;
    if let Some(port) = debug_port {
        println!("jx run: waiting for a debugger on port {port}…");
    }
    let runner = jx_run::PlainMainRunner;
    let mut classpath = vec![classes.display().to_string()];
    classpath.extend(deps.main.iter().map(|path| path.display().to_string()));
    let spec = jx_run::RunSpec {
        java: jdk.java(),
        classpath,
        main_class,
        argv,
        debug_port,
    };
    let code = jx_run::Runner::run(&runner, &spec)?;
    Ok(ExitCode::from(code as u8))
}
