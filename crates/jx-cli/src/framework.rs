//! Framework-aware launch and packaging rules.
//!
//! `[package] framework` used to select only a jar layout. Vert.x, JavaFX,
//! Spring Boot 4, Micronaut and Quarkus need launch-time rules too: a Vert.x
//! verticle is not a static main, a JavaFX application needs a separate
//! launcher on the classpath, Boot 4 needs a modern JDK, Micronaut needs its
//! Java annotation processor, and Quarkus needs augmentation before it can run.

use std::collections::{HashMap, HashSet};

use jx_common::error::{JxError, Kind};
use jx_manifest::Framework;

use crate::deps::LockedJar;

/// Vert.x 5 application launcher, from `io.vertx:vertx-launcher-application`.
pub const VERTX_MODERN_LAUNCHER: &str = "io.vertx.launcher.application.VertxApplication";
/// Vert.x 3/4 command launcher, from `io.vertx:vertx-core`.
pub const VERTX_LEGACY_LAUNCHER: &str = "io.vertx.core.Launcher";
/// Java annotation processor that generates Micronaut bean metadata.
const MICRONAUT_PROCESSOR: &str = "micronaut-inject-java";

/// Which Vert.x launcher the locked dependencies supply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VertxLauncher {
    Modern,
    Legacy,
}

impl VertxLauncher {
    pub fn main_class(self) -> &'static str {
        match self {
            VertxLauncher::Modern => VERTX_MODERN_LAUNCHER,
            VertxLauncher::Legacy => VERTX_LEGACY_LAUNCHER,
        }
    }

    /// Program arguments before the user's own `jx run -- args`. The legacy
    /// command launcher needs its explicit `run` command; the modern
    /// application launcher takes the verticle directly.
    pub fn run_args(self, verticle: &str, user_args: &[String]) -> Vec<String> {
        let mut args = match self {
            VertxLauncher::Modern => vec![verticle.to_string()],
            VertxLauncher::Legacy => vec!["run".to_string(), verticle.to_string()],
        };
        args.extend(user_args.iter().cloned());
        args
    }
}

/// A runnable entry point after framework launch rules are applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Entry {
    Direct {
        main_class: String,
    },
    Vertx {
        launcher: VertxLauncher,
        verticle: String,
    },
}

impl Entry {
    pub fn main_class(&self) -> &str {
        match self {
            Entry::Direct { main_class } => main_class,
            Entry::Vertx { launcher, .. } => launcher.main_class(),
        }
    }

    pub fn run_args(&self, user_args: &[String]) -> Vec<String> {
        match self {
            Entry::Direct { .. } => user_args.to_vec(),
            Entry::Vertx { launcher, verticle } => launcher.run_args(verticle, user_args),
        }
    }

    /// Extra fat-jar manifest headers. Only a launcher-based Vert.x entry
    /// needs one; a hand-written Vert.x main is already the jar's main class.
    pub fn manifest_extra(&self) -> Vec<(&'static str, &str)> {
        match self {
            Entry::Direct { .. } => Vec::new(),
            Entry::Vertx { verticle, .. } => vec![("Main-Verticle", verticle)],
        }
    }
}

/// Apply framework launch rules to an already-resolved main class.
///
/// `pinned` tells whether `[run] main` named the class. `mains` are the
/// compiled classes with a static `main`; `class_exists` tells whether the
/// resolved class was compiled at all. Only Vert.x and JavaFX reinterpret a
/// resolved class: every other framework runs the resolved static main.
pub fn resolve_entry(
    framework: Framework,
    main_class: &str,
    pinned: bool,
    mains: &[String],
    main_jars: &[LockedJar],
    class_exists: bool,
) -> Result<Entry, JxError> {
    match framework {
        Framework::Vertx => {
            if mains.iter().any(|main| main == main_class) {
                return Ok(Entry::Direct {
                    main_class: main_class.to_string(),
                });
            }
            if !pinned {
                return Err(JxError::new(
                    Kind::Build,
                    "a Vert.x project needs `[run] main` for its main verticle",
                )
                .hint("pin the verticle, e.g. `[run] main = \"com.example.MainVerticle\"`"));
            }
            if !class_exists {
                return Err(JxError::new(
                    Kind::Build,
                    format!("no compiled class for `[run] main = \"{main_class}\"`"),
                )
                .at(main_class.to_string())
                .hint("run `jx build` first, then check the spelling against target/classes"));
            }
            validate_entry_class("Main-Verticle", main_class)?;
            Ok(Entry::Vertx {
                launcher: select_vertx_launcher(main_jars)?,
                verticle: main_class.to_string(),
            })
        }
        Framework::JavaFx => {
            if !pinned {
                return Err(JxError::new(
                    Kind::Build,
                    "`[package] framework = \"javafx\"` needs `[run] main` for its launcher class",
                )
                .hint("pin a launcher with a static main that calls the JavaFX application"));
            }
            if !mains.iter().any(|main| main == main_class) {
                if !class_exists {
                    return Err(JxError::new(
                        Kind::Build,
                        format!("no compiled class for `[run] main = \"{main_class}\"`"),
                    )
                    .at(main_class.to_string())
                    .hint("run `jx build` first, then check the spelling against target/classes"));
                }
                return Err(JxError::new(
                    Kind::Build,
                    format!("`{main_class}` cannot be a JavaFX launcher"),
                )
                .at(main_class.to_string())
                .hint(
                    "use a separate launcher class with a static main; the JavaFX \
                     Application subclass itself cannot be launched from the classpath",
                ));
            }
            validate_entry_class("Main-Class", main_class)?;
            require_javafx(main_jars)?;
            Ok(Entry::Direct {
                main_class: main_class.to_string(),
            })
        }
        _ => Ok(Entry::Direct {
            main_class: main_class.to_string(),
        }),
    }
}

/// The Boot loader jar when a Spring Boot project declares it.
pub fn find_boot_loader(main_jars: &[LockedJar]) -> Option<&LockedJar> {
    find_artifact(main_jars, "org.springframework.boot", "spring-boot-loader")
}

/// Choose the Vert.x launcher from the locked runtime jars.
pub fn select_vertx_launcher(main_jars: &[LockedJar]) -> Result<VertxLauncher, JxError> {
    if has_artifact(main_jars, "io.vertx", "vertx-launcher-application") {
        return Ok(VertxLauncher::Modern);
    }
    if let Some(core) = find_artifact(main_jars, "io.vertx", "vertx-core") {
        if version_major(&core.version).is_some_and(|major| major >= 5) {
            return Err(JxError::new(
                Kind::Manifest,
                "Vert.x 5 needs `io.vertx:vertx-launcher-application` for `jx run` and `jx package`",
            )
            .hint(format!(
                "run `jx add io.vertx:vertx-launcher-application:{}`; the legacy core launcher is deprecated in Vert.x 5",
                core.version
            )));
        }
        return Ok(VertxLauncher::Legacy);
    }
    Err(JxError::new(
        Kind::Manifest,
        "a Vert.x project needs `io.vertx:vertx-core` or `io.vertx:vertx-launcher-application`",
    )
    .hint("add the Vert.x runtime, e.g. `jx add io.vertx:vertx-core:5`"))
}

/// A launcher-based Vert.x entry cannot be approximated by the Maven/Gradle
/// exporters: they write a plain jar main without launcher packaging.
pub fn ensure_exportable(entry: &Entry) -> Result<(), JxError> {
    if matches!(entry, Entry::Vertx { .. }) {
        return Err(JxError::new(
            Kind::Build,
            "`jx export` cannot reproduce Vert.x launcher packaging",
        )
        .hint(
            "add Maven Shade or Gradle Shadow configuration with Main-Class and \
             Main-Verticle by hand, or delegate that build to Maven/Gradle",
        ));
    }
    Ok(())
}

/// Spring Boot 4 requires a modern JDK; the loader itself keeps the Boot 3.2
/// layout, so no separate packaging writer is needed.
pub fn require_boot_jdk(loader_version: &str, feature: u32) -> Result<(), JxError> {
    if version_major(loader_version).is_some_and(|major| major >= 4) && feature < 17 {
        return Err(JxError::new(
            Kind::Build,
            format!("Spring Boot {loader_version} needs JDK 17 or newer"),
        )
        .at(format!("spring-boot-loader:{loader_version}"))
        .hint("set `jdk = \"17\"` (or newer) in project.toml, matching the Boot baseline"));
    }
    Ok(())
}

/// A plain `jx run` cannot execute Quarkus bytecode before augmentation.
pub fn reject_quarkus_run(main_jars: &[LockedJar]) -> Result<(), JxError> {
    if uses_group(main_jars, "io.quarkus") {
        return Err(JxError::new(
            Kind::Build,
            "`jx run` cannot execute a Quarkus application before augmentation",
        )
        .hint("run the Quarkus dev/build step, or remove `[package] framework` for a plain jar"));
    }
    Ok(())
}

/// The Micronaut runtime version to suggest when `[processors]` is empty.
/// Micronaut generates its bean metadata at compile time, so a Micronaut
/// application without an annotation processor compiles but does not wire.
pub fn missing_micronaut_processor(
    framework: Framework,
    main_jars: &[LockedJar],
    has_processors: bool,
) -> Option<String> {
    if framework != Framework::Micronaut || has_processors {
        return None;
    }
    main_jars
        .iter()
        .find(|jar| {
            (jar.group_id == "io.micronaut" || jar.group_id.starts_with("io.micronaut."))
                && jar.artifact_id != MICRONAUT_PROCESSOR
        })
        .map(|jar| jar.version.clone())
}

/// Check a JavaFX project before running or writing its portable thin jar.
pub fn require_javafx(main_jars: &[LockedJar]) -> Result<(), JxError> {
    let mut javafx = false;
    let mut classifiers: HashMap<(String, String), HashSet<String>> = HashMap::new();
    for jar in main_jars {
        if jar.group_id != "org.openjfx" {
            continue;
        }
        javafx = true;
        if let Some(classifier) = jar.classifier.as_deref() {
            classifiers
                .entry((jar.group_id.clone(), jar.artifact_id.clone()))
                .or_default()
                .insert(classifier.to_string());
        }
    }
    if !javafx {
        return Err(JxError::new(
            Kind::Manifest,
            "`[package] framework = \"javafx\"` needs an `org.openjfx` dependency",
        )
        .hint(
            "add the JavaFX modules the app uses, e.g. `jx add org.openjfx:javafx-controls:21`",
        ));
    }
    for ((group_id, artifact_id), values) in &classifiers {
        if values.len() > 1 {
            let mut values: Vec<&String> = values.iter().collect();
            values.sort();
            let values = values
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            return Err(JxError::new(
                Kind::Build,
                format!("{group_id}:{artifact_id} has more than one platform classifier ({values})"),
            )
            .hint("declare only the current platform's classifier; a portable thin jar cannot bundle every native build"));
        }
    }
    Ok(())
}

fn has_artifact(main_jars: &[LockedJar], group_id: &str, artifact_id: &str) -> bool {
    main_jars
        .iter()
        .any(|jar| jar.group_id == group_id && jar.artifact_id == artifact_id)
}

fn find_artifact<'a>(
    main_jars: &'a [LockedJar],
    group_id: &str,
    artifact_id: &str,
) -> Option<&'a LockedJar> {
    main_jars
        .iter()
        .find(|jar| jar.group_id == group_id && jar.artifact_id == artifact_id)
}

fn uses_group(main_jars: &[LockedJar], group_id: &str) -> bool {
    main_jars
        .iter()
        .any(|jar| jar.group_id == group_id || jar.group_id.starts_with(&format!("{group_id}.")))
}

/// Leading numeric release (`4.1.0` -> `4`). Unparsable lock versions fall
/// back to the older behaviour instead of inventing a requirement.
fn version_major(version: &str) -> Option<u64> {
    version
        .split(['.', '-', '+'])
        .next()?
        .trim()
        .parse::<u64>()
        .ok()
}

fn validate_entry_class(header: &str, class: &str) -> Result<(), JxError> {
    if class.chars().any(char::is_whitespace) {
        return Err(JxError::new(
            Kind::Build,
            format!("{header} must be a class name without whitespace"),
        )
        .at(class.to_string())
        .hint("fix `[run] main` in project.toml"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn jar(group_id: &str, artifact_id: &str, version: &str) -> LockedJar {
        classified_jar(group_id, artifact_id, version, None)
    }

    fn classified_jar(
        group_id: &str,
        artifact_id: &str,
        version: &str,
        classifier: Option<&str>,
    ) -> LockedJar {
        LockedJar {
            group_id: group_id.to_string(),
            artifact_id: artifact_id.to_string(),
            version: version.to_string(),
            classifier: classifier.map(str::to_string),
            path: PathBuf::from("fake.jar"),
        }
    }

    #[test]
    fn a_hand_written_vertx_main_stays_direct() {
        let mains = vec!["app.Bootstrap".to_string()];
        let entry = resolve_entry(
            Framework::Vertx,
            "app.Bootstrap",
            true,
            &mains,
            &[jar("io.vertx", "vertx-core", "4.5.0")],
            true,
        )
        .unwrap();
        assert_eq!(
            entry,
            Entry::Direct {
                main_class: "app.Bootstrap".to_string()
            }
        );
        assert!(entry.manifest_extra().is_empty());
    }

    #[test]
    fn verticles_select_their_locked_launcher() {
        let mains: Vec<String> = Vec::new();
        let modern = resolve_entry(
            Framework::Vertx,
            "app.MainVerticle",
            true,
            &mains,
            &[
                jar("io.vertx", "vertx-core", "5.1.0"),
                jar("io.vertx", "vertx-launcher-application", "5.1.0"),
            ],
            true,
        )
        .unwrap();
        assert_eq!(
            modern,
            Entry::Vertx {
                launcher: VertxLauncher::Modern,
                verticle: "app.MainVerticle".to_string()
            }
        );
        assert_eq!(modern.main_class(), VERTX_MODERN_LAUNCHER);
        assert_eq!(
            modern.run_args(&["--instances".to_string(), "2".to_string()]),
            vec![
                "app.MainVerticle".to_string(),
                "--instances".to_string(),
                "2".to_string()
            ]
        );
        assert_eq!(
            modern.manifest_extra(),
            vec![("Main-Verticle", "app.MainVerticle")]
        );

        let legacy = resolve_entry(
            Framework::Vertx,
            "app.MainVerticle",
            true,
            &mains,
            &[jar("io.vertx", "vertx-core", "4.5.0")],
            true,
        )
        .unwrap();
        assert_eq!(
            legacy,
            Entry::Vertx {
                launcher: VertxLauncher::Legacy,
                verticle: "app.MainVerticle".to_string()
            }
        );
        assert_eq!(
            legacy.run_args(&[]),
            vec!["run".to_string(), "app.MainVerticle".to_string()]
        );
    }

    #[test]
    fn vertx_needs_a_launcher_and_a_compiled_verticle() {
        let mains: Vec<String> = Vec::new();
        let missing = resolve_entry(
            Framework::Vertx,
            "app.MainVerticle",
            true,
            &mains,
            &[],
            true,
        )
        .unwrap_err();
        assert!(missing.message.contains("needs `io.vertx"), "{missing}");
        assert!(missing.hint.is_some_and(|hint| hint.contains("jx add")));

        let absent = resolve_entry(
            Framework::Vertx,
            "app.Missing",
            true,
            &mains,
            &[jar("io.vertx", "vertx-core", "4.5.0")],
            false,
        )
        .unwrap_err();
        assert!(absent.message.contains("no compiled class"), "{absent}");

        let modern_without_launcher = resolve_entry(
            Framework::Vertx,
            "app.MainVerticle",
            true,
            &mains,
            &[jar("io.vertx", "vertx-core", "5.1.0")],
            true,
        )
        .unwrap_err();
        assert!(
            modern_without_launcher
                .message
                .contains("vertx-launcher-application"),
            "{modern_without_launcher}"
        );
    }

    #[test]
    fn javafx_needs_a_launcher_and_one_platform() {
        let mains = vec!["app.Launcher".to_string()];
        let direct = resolve_entry(
            Framework::JavaFx,
            "app.Launcher",
            true,
            &mains,
            &[classified_jar(
                "org.openjfx",
                "javafx-graphics",
                "21.0.6",
                Some("linux"),
            )],
            true,
        )
        .unwrap();
        assert_eq!(
            direct,
            Entry::Direct {
                main_class: "app.Launcher".to_string()
            }
        );

        let unpinned = resolve_entry(
            Framework::JavaFx,
            "app.Launcher",
            false,
            &mains,
            &[jar("org.openjfx", "javafx-controls", "21.0.6")],
            true,
        )
        .unwrap_err();
        assert!(unpinned.message.contains("[run] main"), "{unpinned}");

        let application = resolve_entry(
            Framework::JavaFx,
            "app.Application",
            true,
            &mains,
            &[jar("org.openjfx", "javafx-controls", "21.0.6")],
            true,
        )
        .unwrap_err();
        assert!(
            application.message.contains("cannot be a JavaFX launcher"),
            "{application}"
        );

        let split_platforms = require_javafx(&[
            classified_jar("org.openjfx", "javafx-graphics", "21.0.6", Some("win")),
            classified_jar("org.openjfx", "javafx-graphics", "21.0.6", Some("linux")),
        ])
        .unwrap_err();
        assert!(
            split_platforms.message.contains("platform classifier"),
            "{split_platforms}"
        );
    }

    #[test]
    fn boot_4_needs_a_modern_jdk() {
        assert!(require_boot_jdk("4.0.0", 17).is_ok());
        let old = require_boot_jdk("4.0.0", 11).unwrap_err();
        assert!(old.message.contains("JDK 17"), "{old}");
        assert!(require_boot_jdk("3.4.0", 11).is_ok());
    }

    #[test]
    fn micronaut_warns_only_without_processors() {
        let runtime = jar("io.micronaut.http", "micronaut-http", "4.8.0");
        assert_eq!(
            missing_micronaut_processor(Framework::Micronaut, &[runtime.clone()], false),
            Some("4.8.0".to_string())
        );
        assert_eq!(
            missing_micronaut_processor(Framework::Micronaut, &[runtime], true),
            None
        );
        assert_eq!(
            missing_micronaut_processor(
                Framework::Plain,
                &[jar("org.example", "lib", "1.0")],
                false
            ),
            None
        );
    }

    #[test]
    fn quarkus_run_is_refused_before_a_misleading_launch() {
        let refused = reject_quarkus_run(&[jar("io.quarkus", "quarkus-core", "3.20.0")]);
        assert!(refused.is_err());
        assert!(reject_quarkus_run(&[jar("org.example", "lib", "1.0")]).is_ok());
    }

    #[test]
    fn vertx_launcher_entries_cannot_be_exported() {
        let entry = Entry::Vertx {
            launcher: VertxLauncher::Modern,
            verticle: "app.MainVerticle".to_string(),
        };
        assert!(ensure_exportable(&entry).is_err());
        assert!(
            ensure_exportable(&Entry::Direct {
                main_class: "app.Main".to_string(),
            })
            .is_ok()
        );
    }
}
