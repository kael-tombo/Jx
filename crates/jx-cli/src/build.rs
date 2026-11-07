//! CLI orchestration of build, test, run and package: manifest → classpaths →
//! JDK → javac → (optionally) JUnit, the main-class launch or a dist tree.
//! Entrypoint discovery lives in `jx-run`, compilation and fingerprints in
//! `jx-build`, jar and dist assembly in `jx-package`.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use jx_common::error::{JxError, Kind};
use jx_manifest::{self as manifest, BuildEngine, Framework};
use jx_toolchain as toolchain;

use crate::deps::{self, Classpaths};
use crate::{Cli, engine, framework, locate_manifest};

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

/// The JDK for a jx-owned build. A `[jdk]` pin is honored exactly when that
/// release is installed; otherwise the newest build for the feature is used
/// with a warning. `pin` is `None` for delegated builds, which never read
/// `jx.lock`.
pub(crate) fn ensure_jdk(
    cli: &Cli,
    feature: u32,
    pin: Option<&jx_lock::PinnedJdk>,
) -> Result<toolchain::InstalledJdk, JxError> {
    let home = toolchain::default_home()?;
    if let Some(pin) = pin {
        if pin.feature != feature {
            return Err(JxError::new(
                Kind::Lock,
                format!(
                    "jx.lock pins JDK {}, but project.toml requests {}",
                    pin.feature, feature
                ),
            )
            .at("jx.lock")
            .hint("run `jx lock` to re-derive the [jdk] entry"));
        }
        if pin.provider != "temurin" {
            return Err(JxError::new(
                Kind::Toolchain,
                format!(
                    "jx.lock pins provider `{}`, which this jx does not install",
                    pin.provider
                ),
            )
            .hint("V1 installs Eclipse Temurin only; change the pin deliberately"));
        }
        if let Some(found) = toolchain::installed_exact(&home, &pin.provider, &pin.release)? {
            return Ok(found);
        }
    }
    let found = if let Some(found) = toolchain::installed_for_feature(&home, feature)? {
        found
    } else if cli.offline {
        return Err(JxError::new(
            Kind::Toolchain,
            format!("--offline: no jx JDK {feature} is installed and downloads are disabled"),
        )
        .hint(format!(
            "run `jx jdk install {feature}` once with a network; everything after works offline"
        )));
    } else {
        toolchain::ensure_jdk(&home, feature)?
    };
    if let Some(pin) = pin
        && found.version != pin.release
        && !cli.quiet
    {
        eprintln!(
            "warning: jx.lock pins temurin {}, but the available JDK {feature} is {}; using it",
            pin.release, found.version
        );
        eprintln!("  hint: run `jx lock` to record the available build, or restore the pinned JDK");
    }
    Ok(found)
}

/// The JDK for a command that owns its build: the manifest's feature and the
/// project's `[jdk]` pin, when one exists.
pub(crate) fn ensure_project_jdk(
    cli: &Cli,
    manifest_dir: &Path,
    feature: u32,
) -> Result<toolchain::InstalledJdk, JxError> {
    let pin = lock_pin(manifest_dir)?;
    ensure_jdk(cli, feature, pin.as_ref())
}

fn lock_pin(manifest_dir: &Path) -> Result<Option<jx_lock::PinnedJdk>, JxError> {
    let path = manifest_dir.join("jx.lock");
    if !path.is_file() {
        return Ok(None);
    }
    Ok(jx_lock::read(&path)?.jdk)
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
    if let Some(version) = framework::missing_micronaut_processor(
        loaded.manifest.framework,
        &deps.main_jars,
        !deps.processors.is_empty(),
    ) {
        eprintln!(
            "warning: this Micronaut project has no annotation processor; generated bean metadata will be missing"
        );
        eprintln!("  hint: run `jx add --processor io.micronaut:micronaut-inject-java:{version}`");
    }
    let feature = loaded.manifest.jdk_feature()?;
    let classes = target_dir(loaded).join("classes");
    let compiled = jx_build::compile_if_stale(&jx_build::CompileRequest {
        javac: &jdk.javac(),
        feature,
        sources_root: &loaded.manifest_dir.join("src/main/java"),
        resources_root: Some(&loaded.manifest_dir.join("src/main/resources")),
        out: &classes,
        classpath: &deps.main,
        processor_path: &deps.processors,
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
        processor_path: &deps.processors,
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

/// Parse `--filter` values into console-launcher selectors. Surefire's
/// wildcard grammar is not reproduced: an unreadable selector is a usage
/// error rather than a silently empty run.
fn junit_selectors(filters: &[String]) -> Result<Vec<(String, String)>, JxError> {
    let mut selectors = Vec::new();
    for raw in filters {
        for part in raw.split(',') {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            let selector = if let Some(package) = part.strip_suffix(".*") {
                if package.is_empty() || package.contains('*') || package.contains('#') {
                    return Err(unreadable_filter(part));
                }
                ("--select-package".to_string(), package.to_string())
            } else if let Some((class, method)) = part.split_once('#') {
                if class.is_empty()
                    || method.is_empty()
                    || class.contains('*')
                    || method.contains('*')
                    || method.contains('#')
                {
                    return Err(unreadable_filter(part));
                }
                ("--select-method".to_string(), format!("{class}#{method}"))
            } else if part.contains('*') {
                return Err(unreadable_filter(part));
            } else {
                ("--select-class".to_string(), part.to_string())
            };
            if !selectors.contains(&selector) {
                selectors.push(selector);
            }
        }
    }
    Ok(selectors)
}

fn unreadable_filter(filter: &str) -> JxError {
    JxError::new(Kind::Usage, format!("`{filter}` is not a usable test filter")).hint(
        "use a fully qualified class (`app.LibTest`), `app.LibTest#method`, or a \
package ending in `.*`; other wildcards are refused so a filter never falls back to the whole suite",
    )
}

fn junit_missing() -> JxError {
    JxError::new(
        Kind::Build,
        "the JUnit Platform console launcher is not on the test classpath",
    )
    .hint(r#"run `jx add --dev org.junit.platform:junit-platform-console-standalone`; it bundles the Jupiter engine"#)
}

/// After the build file and tool are found but before the JDK is installed,
/// so a misconfigured delegate never downloads anything. Returns `None` when
/// jx owns the build.
fn delegated(
    cli: &Cli,
    loaded: &Loaded,
    goal: jx_interop::delegate::Goal,
    tool_args: &[String],
) -> Result<Option<ExitCode>, JxError> {
    use jx_interop::delegate::{self, Engine, Goal};

    let external = match engine::selected(cli, &loaded.manifest) {
        BuildEngine::Jx => return Ok(None),
        BuildEngine::Maven => Engine::Maven,
        BuildEngine::Gradle => Engine::Gradle,
    };
    if cli.locked && matches!(goal, Goal::Build | Goal::Test) {
        return Err(JxError::new(
            Kind::Lock,
            "--locked is a jx.lock guarantee, but this project delegates its build",
        )
        .hint("drop --locked, or pass `--engine jx` to build from project.toml and jx.lock"));
    }
    let plan = delegate::plan(&delegate::Request {
        project_dir: &loaded.manifest_dir,
        engine: external,
        goal,
        offline: cli.offline,
        extra_args: tool_args,
    })?;
    let jdk = ensure_jdk(cli, loaded.manifest.jdk_feature()?, None)?;
    if !cli.quiet {
        println!(
            "delegating to {}: {}",
            external.label(),
            plan.command_line()
        );
        println!("  Java: {}", jdk.dir.display());
    }
    let code = delegate::execute(&plan, &jdk.dir)?;
    Ok(Some(ExitCode::from(code as u8)))
}

/// Trailing arguments only have meaning when an external build owns the
/// command; refusing them is safer than dropping them.
fn tool_arguments_need_delegate(command: &str) -> JxError {
    JxError::new(
        Kind::Usage,
        format!("`jx {command}` tool arguments need a delegated build"),
    )
    .hint(
        "pass `--engine maven` or `--engine gradle` for a project with that build file; \
         jx's own commands take no trailing tool arguments",
    )
}

/// `jx build`
pub fn build(cli: &Cli, tool_args: &[String]) -> Result<ExitCode, JxError> {
    let loaded = load(cli)?;
    if let Some(code) = delegated(cli, &loaded, jx_interop::delegate::Goal::Build, tool_args)? {
        return Ok(code);
    }
    if !tool_args.is_empty() {
        return Err(tool_arguments_need_delegate("build"));
    }
    let deps = deps::classpaths(cli)?;
    let feature = loaded.manifest.jdk_feature()?;
    let jdk = ensure_project_jdk(cli, &loaded.manifest_dir, feature)?;
    compile_main(&loaded, &jdk, &deps)?;
    Ok(ExitCode::SUCCESS)
}

/// `jx test`: compile main and tests, then run them with the JUnit Platform
/// console launcher. `--filter` narrows the run to classes, methods or
/// packages; Surefire XML lands in `target/surefire-reports` unless
/// `--reports-dir` names another directory. Its exit code becomes jx's exit
/// code, so a failing test suite fails CI without a second wrapper.
pub fn test(
    cli: &Cli,
    filters: &[String],
    reports_dir: Option<&Path>,
    tool_args: &[String],
) -> Result<ExitCode, JxError> {
    let selectors = junit_selectors(filters)?;
    let loaded = load(cli)?;
    if !matches!(engine::selected(cli, &loaded.manifest), BuildEngine::Jx)
        && (!filters.is_empty() || reports_dir.is_some())
    {
        return Err(JxError::new(
            Kind::Usage,
            "test filters and report directories belong to jx's own test runner",
        )
        .hint(
            "drop them, pass the delegated tool's own flags after `--`, or run with `--engine jx`",
        ));
    }
    if let Some(code) = delegated(cli, &loaded, jx_interop::delegate::Goal::Test, tool_args)? {
        return Ok(code);
    }
    if !tool_args.is_empty() {
        return Err(tool_arguments_need_delegate("test"));
    }
    let deps = deps::classpaths(cli)?;
    let feature = loaded.manifest.jdk_feature()?;
    let jdk = ensure_project_jdk(cli, &loaded.manifest_dir, feature)?;
    let (main_classes, main_recompiled) = compile_main(&loaded, &jdk, &deps)?;
    let test_classes = compile_tests(&loaded, &jdk, &deps, &main_classes, main_recompiled)?;
    let launcher = deps.junit_launcher.clone().ok_or_else(junit_missing)?;

    let mut classpath = vec![test_classes.display().to_string()];
    classpath.push(main_classes.display().to_string());
    classpath.extend(deps.test.iter().map(|path| path.display().to_string()));
    let reports = reports_dir.map_or_else(
        || target_dir(&loaded).join("surefire-reports"),
        Path::to_path_buf,
    );
    if !cli.quiet {
        let shown = reports
            .strip_prefix(&loaded.manifest_dir)
            .unwrap_or(&reports);
        println!("  reports: {}", shown.display());
    }
    let mut command = Command::new(jdk.java());
    command.arg("-jar").arg(&launcher);
    // JUnit Platform 1.10 introduced the `execute` command and 1.14 deprecates
    // the implicit delegation. Older launchers only accept the flat form, so
    // choose from the locked launcher version rather than probing with a JVM.
    if deps
        .junit_launcher_version
        .as_deref()
        .is_some_and(junit_supports_execute)
    {
        command.arg("execute");
    }
    command
        .arg("--class-path")
        .arg(classpath.join(jx_run::classpath_separator()));
    if selectors.is_empty() {
        command.arg("--scan-class-path");
    } else {
        for (option, value) in &selectors {
            command.arg(option).arg(value);
        }
    }
    let status = command
        .arg("--fail-if-no-tests")
        .arg("--reports-dir")
        .arg(&reports)
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

/// Apply Vert.x/JavaFX launch rules to an already-resolved main class.
fn framework_entry(
    loaded: &Loaded,
    classes: &Path,
    main_class: &str,
    main_jars: &[deps::LockedJar],
) -> Result<framework::Entry, JxError> {
    let mains = jx_run::discover_main(classes)?;
    let class_exists = jx_run::compiled_class_exists(classes, main_class);
    framework::resolve_entry(
        loaded.manifest.framework,
        main_class,
        loaded.manifest.main_class().is_some(),
        &mains,
        main_jars,
        class_exists,
    )
}

/// `[package] framework = "spring-boot"` needs the loader on the runtime
/// classpath: no Boot starter depends on it, and jx cannot invent the
/// launcher classes.
fn boot_loader_missing(version: Option<&str>) -> JxError {
    let coordinate = version.map_or_else(
        || "org.springframework.boot:spring-boot-loader".to_string(),
        |version| format!("org.springframework.boot:spring-boot-loader:{version}"),
    );
    JxError::new(
        Kind::Manifest,
        "`[package] framework = \"spring-boot\"` needs `org.springframework.boot:spring-boot-loader`",
    )
    .hint(format!(
        "jx writes the launcher into the jar but cannot supply it; run `jx add {coordinate}`"
    ))
}

/// A launcher-based framework cannot package an unnamed entry point.
fn framework_main_required(framework: Framework) -> JxError {
    match framework {
        Framework::Vertx => JxError::new(
            Kind::Build,
            "`[package] framework = \"vertx\"` needs `[run] main` for its main verticle",
        )
        .hint("pin the verticle, e.g. `[run] main = \"com.example.MainVerticle\"`"),
        Framework::JavaFx => JxError::new(
            Kind::Build,
            "`[package] framework = \"javafx\"` needs `[run] main` for its launcher class",
        )
        .hint("pin a launcher with a static main that calls the JavaFX application"),
        _ => JxError::new(Kind::Internal, "framework entry without a main class")
            .hint("report this as a jx bug"),
    }
}

/// `jx package`: compile main sources, then write a thin `target/dist`, a
/// single fat jar, or the layout `[package] framework` selects.
pub fn package(cli: &Cli, fat: bool) -> Result<(), JxError> {
    let loaded = load(cli)?;
    engine::require_jx(
        cli,
        &loaded.manifest,
        "jx package",
        "the delegated build owns compilation and packaging",
    )?;

    let framework = loaded.manifest.framework;
    match framework {
        Framework::Quarkus => {
            return Err(JxError::new(
                Kind::Build,
                "Quarkus packaging is not jx's to write",
            )
            .hint(
                "`quarkus:build` generates `quarkus-app/` while augmenting; run that build step, or remove `[package] framework` to write a plain jar",
            ));
        }
        Framework::JakartaEe => {
            return Err(JxError::new(
                Kind::Build,
                "a Jakarta EE WAR needs container-provided APIs",
            )
            .hint(
                "jx has no `provided` scope, so a WAR would bundle the container's own Jakarta APIs; build the WAR with the container's tooling",
            ));
        }
        Framework::SpringBoot if fat => {
            return Err(JxError::new(
                Kind::Build,
                "`--fat` cannot be combined with Spring Boot packaging",
            )
            .hint(
                "`[package] framework = \"spring-boot\"` already writes one executable jar; drop `--fat`",
            ));
        }
        Framework::JavaFx if fat => {
            return Err(JxError::new(
                Kind::Build,
                "`--fat` cannot be combined with JavaFX packaging",
            )
            .hint(
                "`[package] framework = \"javafx\"` writes a portable thin jar; merging platform-specific natives is not portable",
            ));
        }
        _ => {}
    }
    if matches!(framework, Framework::Vertx | Framework::JavaFx)
        && loaded.manifest.main_class().is_none()
    {
        return Err(framework_main_required(framework));
    }
    let fat = fat || matches!(framework, Framework::Micronaut | Framework::Vertx);

    let deps = deps::classpaths(cli)?;
    // Fail before compiling: without the loader no Boot jar can be written.
    let loader = if framework == Framework::SpringBoot {
        match framework::find_boot_loader(&deps.main_jars) {
            Some(jar) => Some(jar),
            None => {
                let version = deps
                    .main_jars
                    .iter()
                    .find(|jar| jar.group_id == "org.springframework.boot")
                    .map(|jar| jar.version.as_str());
                return Err(boot_loader_missing(version));
            }
        }
    } else {
        None
    };
    let feature = loaded.manifest.jdk_feature()?;
    if let Some(loader) = loader {
        framework::require_boot_jdk(&loader.version, feature)?;
    }
    let jdk = ensure_project_jdk(cli, &loaded.manifest_dir, feature)?;
    let (classes, _) = compile_main(&loaded, &jdk, &deps)?;
    let main_class = package_main(&classes, loaded.manifest.main_class())?;
    let entry = match framework {
        Framework::Vertx | Framework::JavaFx => {
            let Some(main) = main_class.as_deref() else {
                return Err(framework_main_required(framework));
            };
            Some(framework_entry(&loaded, &classes, main, &deps.main_jars)?)
        }
        _ => None,
    };
    let extra = entry
        .as_ref()
        .map(framework::Entry::manifest_extra)
        .unwrap_or_default();
    let jar_main = match entry.as_ref() {
        Some(entry) => Some(entry.main_class()),
        None => main_class.as_deref(),
    };
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

    let dist = target_dir(&loaded).join("dist");
    let output = if let Some(loader) = loader {
        let Some(main_class) = main_class.as_deref() else {
            return Err(
                JxError::new(Kind::Build, "Spring Boot packaging needs a start class").hint(
                    "set `[run] main` in project.toml; an executable Boot jar is not a library",
                ),
            );
        };
        // The loader lives at the archive root, not in BOOT-INF/lib.
        // Nested jars keep Maven `artifact-version[-classifier].jar` names:
        // the cache files are content-addressed, and the Boot launcher
        // scans `BOOT-INF/lib` for nested archives.
        let mut dependencies = Vec::with_capacity(deps.main_jars.len());
        for jar in &deps.main_jars {
            if jar.group_id == "org.springframework.boot" && jar.artifact_id == "spring-boot-loader"
            {
                continue;
            }
            dependencies.push(jx_package::spring_boot::NestedJar {
                path: &jar.path,
                name: jx_package::artifact_file_name(
                    &jar.artifact_id,
                    &jar.version,
                    jar.classifier.as_deref(),
                )?,
            });
        }
        let request = jx_package::spring_boot::SpringBootRequest {
            dist_dir: &dist,
            classes_dir: &classes,
            jar_name: &jar_name,
            main_class,
            loader_jar: &loader.path,
            loader_version: &loader.version,
            dependencies: &dependencies,
        };
        jx_package::spring_boot::package(&request)?
    } else {
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
        jx_package::package(&jx_package::PackageRequest {
            mode: if fat {
                jx_package::PackageMode::Fat
            } else {
                jx_package::PackageMode::Thin
            },
            dist_dir: &dist,
            classes_dir: &classes,
            jar_name: &jar_name,
            main_class: jar_main,
            extra_manifest: &extra,
            dependencies: &dependencies,
        })?
    };
    let display = output
        .jar
        .strip_prefix(&loaded.manifest_dir)
        .unwrap_or(&output.jar);
    let plural = if output.dependency_count == 1 {
        "y"
    } else {
        "ies"
    };
    if framework == Framework::SpringBoot {
        println!(
            "packaged {} ({} dependencies nested in BOOT-INF/lib)",
            display.display(),
            output.dependency_count
        );
        if output.unsafe_entries > 0 {
            println!(
                "  boot jar: refused {} unsafe entry names",
                output.unsafe_entries
            );
        }
    } else if fat {
        println!(
            "packaged {} ({} dependenc{plural} merged)",
            display.display(),
            output.dependency_count
        );
        if let Some(framework::Entry::Vertx { launcher, verticle }) = entry.as_ref() {
            println!(
                "  vertx launcher: Main-Class: {}; Main-Verticle: {}",
                launcher.main_class(),
                verticle
            );
        }
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
    if jar_main.is_some() {
        println!("  run: java -jar {}", output.jar.display());
    } else {
        println!("  library jar (no Main-Class)");
    }
    Ok(())
}

/// `jx image`: package the thin dist, then add a jlink runtime beside the
/// application jar. The app must have a main class; a library can still be
/// packaged, but there is no program for the image to run.
///
/// The previous `target/dist` is moved aside before packaging and restored if
/// anything after that fails, so a failed image build does not destroy a
/// working app/runtime pair.
pub fn image(cli: &Cli, full: bool) -> Result<(), JxError> {
    let loaded = load(cli)?;
    engine::require_jx(
        cli,
        &loaded.manifest,
        "jx image",
        "the delegated build owns compilation and packaging",
    )?;
    if matches!(
        loaded.manifest.framework,
        Framework::SpringBoot | Framework::Micronaut | Framework::Vertx
    ) {
        return Err(JxError::new(
            Kind::Build,
            "`jx image` needs the thin `target/dist` layout, not a framework executable",
        )
        .hint(
            "use `jx package` for this framework; `jx image` cannot jlink a Boot, fat or launcher-based jar",
        ));
    }
    let dist = target_dir(&loaded).join("dist");
    let backup = target_dir(&loaded).join(format!(".jx-dist-backup-{}", std::process::id()));
    if backup.exists() {
        remove_dist_path(&backup).map_err(|e| {
            JxError::new(Kind::Io, "cannot clear a stale image backup")
                .at(backup.display().to_string())
                .caused_by(e)
        })?;
    }
    let had_dist = dist.exists();
    if had_dist {
        std::fs::rename(&dist, &backup).map_err(|e| {
            JxError::new(
                Kind::Io,
                "cannot move the previous dist aside for the image",
            )
            .at(dist.display().to_string())
            .caused_by(e)
            .hint("close any running program that holds files under target/dist")
        })?;
    }

    let result = image_into(cli, full);
    match result {
        Ok(()) => {
            if had_dist && let Err(e) = remove_dist_path(&backup) {
                eprintln!(
                    "warning: image is ready, but the previous dist backup could not be removed: {e}"
                );
            }
            Ok(())
        }
        Err(error) => {
            if let Err(e) = remove_dist_path(&dist) {
                eprintln!("warning: could not remove the incomplete target/dist: {e}");
            }
            if had_dist && let Err(e) = std::fs::rename(&backup, &dist) {
                eprintln!("warning: could not restore the previous target/dist: {e}");
            }
            Err(error)
        }
    }
}

fn remove_dist_path(path: &std::path::Path) -> std::io::Result<()> {
    match std::fs::metadata(path) {
        Ok(metadata) if metadata.is_dir() => std::fs::remove_dir_all(path),
        Ok(_) => std::fs::remove_file(path),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

fn image_into(cli: &Cli, full: bool) -> Result<(), JxError> {
    package(cli, false)?;
    let loaded = load(cli)?;
    let deps = deps::classpaths(cli)?;
    let feature = loaded.manifest.jdk_feature()?;
    let jdk = ensure_project_jdk(cli, &loaded.manifest_dir, feature)?;
    let classes = target_dir(&loaded).join("classes");
    jx_run::decide_main(&classes, loaded.manifest.main_class()).map_err(|e| {
        e.hint("jx image needs an application; add `[run] main` or keep using `jx package` for a library")
    })?;

    let dist = target_dir(&loaded).join("dist");
    let jar_name = jx_package::jar_file_name(&loaded.manifest.name, &loaded.manifest.version)?;
    let jar = dist.join(jar_name);
    if !jar.is_file() {
        return Err(JxError::new(
            Kind::Internal,
            "jx package succeeded but the application jar is missing",
        )
        .at(jar.display().to_string())
        .hint("run `jx package` again; if this repeats, report it as a jx bug"));
    }
    let classpath = deps
        .main
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join(jx_run::classpath_separator());
    let output = jx_package::image::build(&jx_package::image::ImageRequest {
        jdeps: &jdk.jdeps(),
        jlink: &jdk.jlink(),
        classes_dir: &classes,
        classpath: &classpath,
        output_dir: &dist.join("runtime"),
        feature,
        full,
    })?;

    let display = output
        .runtime
        .strip_prefix(&loaded.manifest_dir)
        .unwrap_or(&output.runtime);
    let description = match output.mode {
        jx_package::image::ImageMode::Trimmed => {
            format!("trimmed runtime: {}", output.modules.join(", "))
        }
        jx_package::image::ImageMode::Full => {
            format!("full runtime, {} linkable modules", output.modules.len())
        }
        jx_package::image::ImageMode::FullFallback => format!(
            "full runtime; jdeps could not trim: {}",
            output
                .fallback_reason
                .as_deref()
                .unwrap_or("unknown reason")
        ),
    };
    println!("image: {} ({description})", display.display());
    println!(
        "  run: {} -jar {}",
        jx_package::image::runtime_java(&output.runtime).display(),
        jar.display()
    );
    if output.mode == jx_package::image::ImageMode::Trimmed {
        println!(
            "  note: jdeps reads class references only; use `jx image --full` if reflection loads more modules"
        );
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

/// Map direct dependencies to Gradle entries. The lookup and version rules
/// are the Maven exporter's; only the notation differs, so both exports name
/// exactly the same locked artifacts.
fn gradle_dependencies(
    specs: &[manifest::DependencySpec],
    jars: &[deps::LockedJar],
    test: bool,
) -> Result<Vec<jx_package::gradle::GradleDependency>, JxError> {
    let mapped = maven_dependencies(specs, jars, test.then_some("test"))?;
    Ok(mapped
        .into_iter()
        .map(|dependency| {
            let mut out = jx_package::gradle::GradleDependency::new(
                dependency.group_id,
                dependency.artifact_id,
                dependency.version,
            );
            if let Some(classifier) = dependency.classifier {
                out = out.classifier(classifier);
            }
            if dependency.scope == Some("test") {
                out = out.test();
            }
            out
        })
        .collect())
}

/// Launcher-based Vert.x entries are executable only with launcher packaging,
/// which the exporters deliberately do not reproduce.
fn check_vertx_export(
    loaded: &Loaded,
    classes: &Path,
    main_class: Option<&String>,
    main_jars: &[deps::LockedJar],
) -> Result<(), JxError> {
    if loaded.manifest.framework != Framework::Vertx {
        return Ok(());
    }
    if let Some(main) = main_class {
        framework::ensure_exportable(&framework_entry(loaded, classes, main, main_jars)?)?;
    }
    Ok(())
}

/// `jx export maven`: compile once (so a discovered main class is included),
/// then write a POM that mirrors project.toml and the locked versions.
pub fn export_maven(cli: &Cli, group_id: &str, force: bool) -> Result<(), JxError> {
    let loaded = load(cli)?;
    engine::require_jx(
        cli,
        &loaded.manifest,
        "jx export maven",
        "jx would write a second build next to the Maven build this project delegates to",
    )?;
    let path = loaded.manifest_dir.join("pom.xml");
    if path.exists() && !force {
        return Err(JxError::new(Kind::Io, "pom.xml already exists")
            .at(path.display().to_string())
            .hint("move it aside, or rerun with `jx export maven --force` to overwrite"));
    }
    let deps = deps::classpaths(cli)?;
    let feature = loaded.manifest.jdk_feature()?;
    let jdk = ensure_project_jdk(cli, &loaded.manifest_dir, feature)?;
    let (classes, _) = compile_main(&loaded, &jdk, &deps)?;
    let main_class = package_main(&classes, loaded.manifest.main_class())?;
    check_vertx_export(&loaded, &classes, main_class.as_ref(), &deps.main_jars)?;

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

/// `jx export gradle`: compile once (so a discovered main class is included),
/// then write `build.gradle.kts` and `settings.gradle.kts` from project.toml
/// and the locked versions.
pub fn export_gradle(cli: &Cli, group_id: &str, force: bool) -> Result<(), JxError> {
    let loaded = load(cli)?;
    engine::require_jx(
        cli,
        &loaded.manifest,
        "jx export gradle",
        "jx would write a second build next to the Gradle build this project delegates to",
    )?;
    let build_path = loaded.manifest_dir.join("build.gradle.kts");
    let settings_path = loaded.manifest_dir.join("settings.gradle.kts");
    let groovy_path = loaded.manifest_dir.join("build.gradle");
    if groovy_path.exists() {
        return Err(JxError::new(
            Kind::Io,
            "a Groovy build.gradle already exists; jx would create a second build",
        )
        .at(groovy_path.display().to_string())
        .hint("move or delete build.gradle, then run `jx export gradle` again"));
    }
    for path in [&build_path, &settings_path] {
        if path.exists() && !force {
            return Err(JxError::new(
                Kind::Io,
                format!(
                    "{} already exists",
                    path.file_name().unwrap_or_default().to_string_lossy()
                ),
            )
            .at(path.display().to_string())
            .hint("move it aside, or rerun with `jx export gradle --force` to overwrite"));
        }
    }

    let deps = deps::classpaths(cli)?;
    let feature = loaded.manifest.jdk_feature()?;
    let jdk = ensure_project_jdk(cli, &loaded.manifest_dir, feature)?;
    let (classes, _) = compile_main(&loaded, &jdk, &deps)?;
    let main_class = package_main(&classes, loaded.manifest.main_class())?;
    check_vertx_export(&loaded, &classes, main_class.as_ref(), &deps.main_jars)?;

    let mut dependencies =
        gradle_dependencies(&loaded.manifest.dependencies, &deps.all_jars, false)?;
    dependencies.extend(gradle_dependencies(
        &loaded.manifest.dev_dependencies,
        &deps.all_jars,
        true,
    )?);
    let mut overrides = Vec::new();
    for spec in &loaded.manifest.overrides {
        if let Some(jar) = deps.all_jars.iter().find(|jar| {
            jar.group_id == spec.group_id
                && jar.artifact_id == spec.artifact_id
                && jar.classifier.is_none()
        }) {
            overrides.push(jx_package::gradle::GradleDependency::new(
                &jar.group_id,
                &jar.artifact_id,
                &jar.version,
            ));
        }
    }

    let main_count = dependencies
        .iter()
        .filter(|dependency| !dependency.test)
        .count();
    let test_count = dependencies.len() - main_count;
    let constraint_count = overrides.len();
    let export = jx_package::gradle::export(&jx_package::gradle::GradleProject {
        group_id: group_id.to_string(),
        artifact_id: loaded.manifest.name.clone(),
        version: loaded.manifest.version.clone(),
        java_release: feature,
        main_class,
        dependencies,
        overrides,
    })?;

    let produced_settings = !settings_path.exists();
    write_export(&settings_path, &export.settings)?;
    if let Err(error) = write_export(&build_path, &export.build) {
        // Do not leave a settings file behind when the build script failed.
        if produced_settings {
            let _ = std::fs::remove_file(&settings_path);
        }
        return Err(error);
    }
    println!(
        "exported build.gradle.kts and settings.gradle.kts ({main_count} main, {test_count} test, {constraint_count} constraints)"
    );
    println!("  build with: gradle build");
    Ok(())
}

fn write_export(path: &std::path::Path, text: &str) -> Result<(), JxError> {
    std::fs::write(path, text).map_err(|e| {
        JxError::new(Kind::Io, "cannot write the Gradle build")
            .at(path.display().to_string())
            .caused_by(e)
    })
}

/// `jx run [-- args]`, with optional `--debug [--port N]`.
pub fn run(cli: &Cli, debug_port: Option<u16>, argv: Vec<String>) -> Result<ExitCode, JxError> {
    let loaded = load(cli)?;
    engine::require_jx(
        cli,
        &loaded.manifest,
        "jx run",
        "the delegated build owns compilation and the runtime classpath",
    )?;
    let deps = deps::classpaths(cli)?;
    if loaded.manifest.framework == Framework::Quarkus {
        framework::reject_quarkus_run(&deps.main_jars)?;
    }
    let feature = loaded.manifest.jdk_feature()?;
    if loaded.manifest.framework == Framework::SpringBoot
        && let Some(loader) = framework::find_boot_loader(&deps.main_jars)
    {
        framework::require_boot_jdk(&loader.version, feature)?;
    }
    let jdk = ensure_project_jdk(cli, &loaded.manifest_dir, feature)?;
    let (classes, _) = compile_main(&loaded, &jdk, &deps)?;
    let main_class = jx_run::decide_main(&classes, loaded.manifest.main_class())?;
    let entry = match loaded.manifest.framework {
        Framework::Vertx | Framework::JavaFx => {
            framework_entry(&loaded, &classes, &main_class, &deps.main_jars)?
        }
        _ => framework::Entry::Direct {
            main_class: main_class.clone(),
        },
    };
    if let Some(port) = debug_port {
        println!("jx run: waiting for a debugger on port {port}…");
    }
    let runner = jx_run::PlainMainRunner;
    let mut classpath = vec![classes.display().to_string()];
    classpath.extend(deps.main.iter().map(|path| path.display().to_string()));
    let spec = jx_run::RunSpec {
        java: jdk.java(),
        classpath,
        main_class: entry.main_class().to_string(),
        argv: entry.run_args(&argv),
        debug_port,
    };
    let code = jx_run::Runner::run(&runner, &spec)?;
    Ok(ExitCode::from(code as u8))
}

/// `jx clean`: remove `<project>/target`, or ask a delegated engine to clean
/// its own output. A directory with no manifest keeps the old behaviour and
/// removes `target/` below the current directory.
pub fn clean(cli: &Cli, tool_args: &[String]) -> Result<ExitCode, JxError> {
    let cwd = std::env::current_dir()
        .map_err(|e| JxError::new(Kind::Io, "cannot read the current directory").caused_by(e))?;
    let manifest_path = match locate_manifest(cli) {
        Ok(path) => path,
        Err(error) => {
            if cli.manifest_path.is_some() {
                return Err(error);
            }
            if cli
                .engine
                .is_some_and(|engine| BuildEngine::from(engine).is_delegated())
            {
                return Err(JxError::new(
                    Kind::Manifest,
                    "engine delegation needs a project.toml for the JDK pin",
                )
                .hint("run `jx import` here first, then `jx clean`"));
            }
            if !tool_args.is_empty() {
                return Err(tool_arguments_need_delegate("clean"));
            }
            remove_target(&cwd.join("target"))?;
            return Ok(ExitCode::SUCCESS);
        }
    };
    let loaded = Loaded {
        manifest_dir: manifest_path
            .parent()
            .expect("manifest path always has a parent")
            .to_path_buf(),
        manifest: manifest::parse(&manifest_path)?,
    };
    if let Some(code) = delegated(cli, &loaded, jx_interop::delegate::Goal::Clean, tool_args)? {
        return Ok(code);
    }
    if !tool_args.is_empty() {
        return Err(tool_arguments_need_delegate("clean"));
    }
    remove_target(&loaded.manifest_dir.join("target"))?;
    Ok(ExitCode::SUCCESS)
}

fn remove_target(target: &Path) -> Result<(), JxError> {
    if target.is_dir() {
        std::fs::remove_dir_all(target).map_err(|e| {
            JxError::new(Kind::Io, "cannot remove target/")
                .at(target.display().to_string())
                .caused_by(e)
        })?;
    }
    Ok(())
}

/// Whether this console-launcher version accepts the `execute` subcommand,
/// introduced in JUnit Platform 1.10.
fn junit_supports_execute(version: &str) -> bool {
    let Ok(version) = jx_maven::ComparableVersion::parse(version) else {
        return false;
    };
    let Ok(minimum) = jx_maven::ComparableVersion::parse("1.10.0") else {
        return false;
    };
    version.compare(&minimum) != std::cmp::Ordering::Less
}

#[cfg(test)]
mod tests {
    use super::{boot_loader_missing, junit_selectors, junit_supports_execute};

    #[test]
    fn a_missing_boot_loader_names_the_matching_coordinate() {
        let error = boot_loader_missing(Some("3.4.0"));
        assert!(error.message.contains("spring-boot-loader"), "{error}");
        assert!(
            error
                .hint
                .as_deref()
                .is_some_and(|hint| hint
                    .contains("jx add org.springframework.boot:spring-boot-loader:3.4.0")),
            "{error}"
        );

        let unknown = boot_loader_missing(None);
        assert!(
            unknown.hint.as_deref().is_some_and(
                |hint| hint.contains("jx add org.springframework.boot:spring-boot-loader")
            ),
            "{unknown}"
        );
    }

    #[test]
    fn junit_execute_is_used_only_from_platform_1_10() {
        assert!(!junit_supports_execute("1.8.2"));
        assert!(!junit_supports_execute("1.9.3"));
        assert!(junit_supports_execute("1.10.0"));
        assert!(junit_supports_execute("1.10.5"));
        assert!(junit_supports_execute("1.14.4"));
        assert!(junit_supports_execute("2.0.0"));
        assert!(!junit_supports_execute("not-a-version"));
    }

    #[test]
    fn filters_map_to_console_selectors() {
        let selectors = junit_selectors(&[
            "app.LibTest".to_string(),
            "app.LibTest#addsTwoNumbers".to_string(),
            "app.*".to_string(),
            "app.OtherTest, app.OtherTest#works".to_string(),
        ])
        .unwrap();
        assert_eq!(
            selectors,
            vec![
                ("--select-class".to_string(), "app.LibTest".to_string()),
                (
                    "--select-method".to_string(),
                    "app.LibTest#addsTwoNumbers".to_string()
                ),
                ("--select-package".to_string(), "app".to_string()),
                ("--select-class".to_string(), "app.OtherTest".to_string()),
                (
                    "--select-method".to_string(),
                    "app.OtherTest#works".to_string()
                ),
            ]
        );
    }

    #[test]
    fn repeated_filters_are_deduplicated() {
        let selectors = junit_selectors(&[
            "app.LibTest#works".to_string(),
            "app.LibTest#works".to_string(),
        ])
        .unwrap();
        assert_eq!(selectors.len(), 1);
    }

    #[test]
    fn an_empty_filter_still_scans_the_classpath() {
        assert!(junit_selectors(&[]).unwrap().is_empty());
        assert!(
            junit_selectors(&[String::new(), " , ".to_string()])
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn wildcard_filters_are_refused() {
        for bad in [
            "*",
            ".*",
            "app.*Test",
            "app.LibTest#*",
            "app.LibTest#a#b",
            "app.*#works",
        ] {
            let error = junit_selectors(&[bad.to_string()]).unwrap_err();
            assert_eq!(error.exit_code(), 2, "`{bad}` must be a usage error");
        }
    }
}
