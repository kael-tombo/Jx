# 0011. Gradle import and export

Date: 2026-11-01
Status: Accepted

## Context and problem

Gradle is the default build tool for new Java projects, so Maven-only
interop would leave the larger half of the ecosystem outside jx. Unlike a
POM, a Gradle build is a program: `build.gradle` and `build.gradle.kts` are
Groovy and Kotlin scripts that can compute coordinates, branch on the
environment, read files and apply plugins that change dependency management.

Two escapes are needed, just as for Maven:

* **Entry:** `jx import` should turn a Gradle build into `project.toml`
  without running Gradle.
* **Exit:** `jx export gradle` should write a Gradle build from the manifest
  and lockfile so a team can leave.

An importer that evaluates a script partially would silently drop
dependencies. An exporter that invents plugin behaviour would produce a
build that succeeds but packages differently. Both are worse than a refusal.

## Decision

### Import

`jx import [path]` now detects `pom.xml`, `build.gradle.kts` or
`build.gradle`. A directory with both builds imports the POM and prints
which file it chose; passing the Gradle script explicitly converts Gradle.
The `settings.gradle(.kts)` and `gradle.properties` files beside the script
are read for the project name and version.

The reader is a small lexer plus a declaration reader, not an evaluator. It
understands only the declarative subset jx can represent:

* `dependencies { }` calls for `implementation`, `api`, `compileOnly`,
  `compileOnlyApi`, `runtimeOnly`, `testImplementation`,
  `testCompileOnly` and `testRuntimeOnly`, in Kotlin string form, Groovy
  string form, and Groovy map form (`group:`, `name:`, `version:`,
  `classifier:`).
* `platform(...)` / `enforcedPlatform(...)` become `{ platform = … }`
  entries. They are always written to `[dependencies]`, because jx's
  platform supply applies to both tables.
* `constraints { }` become `[overrides]` at their declared versions.
* Literal variables from `ext.name = …`, `ext { name = … }` and
  `def`/`val`/`var name = …` are substituted into coordinates. A reference
  that cannot be resolved this way is an error, never a dropped dependency.
* `java { toolchain { languageVersion = JavaLanguageVersion.of(N) } }`,
  `sourceCompatibility`/`targetCompatibility`/`release` (including
  `JavaVersion.VERSION_*`) select the `jdk` pin.
* `application { mainClass = … }` / `mainClassName` become `[run] main`.
* `version` (build script or `gradle.properties`) becomes the project
  version; `rootProject.name` becomes the name. Missing values fall back to
  the directory name and `0.1.0`, with a note.

Versionless dependencies are only accepted when a platform BOM is imported.
The one inferred case is the Spring Boot plugin: when
`id("org.springframework.boot")` has a literal version and dependencies have
no versions, the import writes `org.springframework.boot:spring-boot-dependencies`
at that plugin version as a platform, because that is the BOM the plugin
applies. The note says so, and `jx lock` then verifies every artifact is
actually managed.

Everything else stops the import with a location and a remedy: string
interpolation that is not a known literal, `project(...)`, `files(...)`,
`fileTree(...)`, version catalogs (`libs.*`), dynamic or range versions,
SNAPSHOTs, `if`/`for`/`while` around dependency declarations, `allprojects`
or `subprojects` dependency blocks, and a `mainClass` that is not a literal.

Java annotation processors declared with `annotationProcessor` or
`testAnnotationProcessor` and a literal version are imported into
`[processors]`; jx runs them on `javac -processorpath` for main and test
compilation, so one coordinate declared for both is written once.

Non-fatal approximations are counted and printed: plugins (never migrated),
Gradle repositories (jx trusts `config.toml`), the `buildscript` classpath,
annotation processors that cannot be read statically (`kapt`, `ksp`, a
processor with no literal version — add those to `[processors]` by hand),
unknown configurations,
dependency configuration blocks (excludes/transitive flags), runtime and
compileOnly scope widening, and multi-module settings.

### Export

`jx export gradle` compiles once (so a discovered main class is included)
and writes two files:

* `build.gradle.kts`: `application` when a main class exists, otherwise
  `java-library`; `group` (default `local.jx`, `--group-id` to change) and
  `version`; a `java { toolchain { languageVersion = … } }` block; then
  `mavenCentral()`; `implementation` and `testImplementation` entries at the
  exact locked versions, classifiers preserved; `[overrides]` as
  `constraints`; the `application { mainClass = … }` block; and
  `tasks.test { useJUnitPlatform() }` when test dependencies exist.
* `settings.gradle.kts`: `rootProject.name`.

Platform BOMs are not re-exported: their effect is already in the concrete
versions, and the manifest does not record the locked BOM artifact.

An existing `build.gradle.kts` or `settings.gradle.kts` is only replaced
with `--force`. A Groovy `build.gradle` is never coexisted with: the export
refuses until it is moved or deleted, because two build scripts in one
directory are ambiguous.

## Options considered

* **Evaluate Gradle scripts by running Gradle** — would be exact, but makes
  `jx import` depend on Gradle, on the right Gradle version and on plugin
  resolution from the network. It also contradicts jx's promise that the
  first run needs no other build tool.
* **Parse with a general Groovy/Kotlin parser** — more grammar than the
  problem needs, and still unable to evaluate values. The declaration reader
  fails loudly on anything outside its subset.
* **Refuse Spring Boot projects without an explicit platform() line** — the
  most conservative option, but it excludes the most common Gradle Java
  application. The plugin's BOM is documented behaviour, so the import
  writes it explicitly and notes it; a wrong guess then fails at `jx lock`,
  not silently.
* **Emit `mavenCentral()` (export) instead of copying `config.toml`
  repositories** — mirrors and credentials are machine configuration, not
  project configuration. Writing them into a repository would leak private
  URLs. Users with private repositories must add them to the generated
  build, and the ADR records that gap.
* **Emit platform BOMs on export** — the manifest's BOM version may be a
  prefix, and the BOM artifact is not in the lockfile. Concrete dependency
  versions are the accurate representation.

## Consequences

* A typical Gradle application — Kotlin DSL or Groovy, literal versions or
  `ext` variables, a platform BOM or the Spring Boot plugin — imports into a
  manifest that resolves. Spring Petclinic's current Gradle build was the
  acceptance case: it imports with no errors and `jx lock` records 162
  artifacts.
* A build that computes coordinates, uses a version catalog, or branches on
  the environment cannot be imported; the error names the construct. Those
  projects keep Gradle and can use the delegated-build mode in ADR 0012.
* Plugin behaviour beyond the Spring Boot BOM (shading, native images,
  custom source sets, annotation processing) is not reproduced. The notes
  say which plugins were seen.
* The generated Kotlin DSL was verified manually against Gradle 8.14.3:
  `gradle build` and `gradle run` succeed for a scaffolded application,
  including locked dependencies and an `[overrides]` constraint. CI has no
  Gradle distribution yet, so M8 should add a pinned Gradle job when release
  binaries exist.
* Two effective-POM bugs surfaced while validating against Spring Petclinic
  and are fixed in the same change: dependency `groupId`/`artifactId` and
  exclusions are now interpolated, `${project.<property>}` falls back to a
  declared property, and `provided` dependencies are no longer transitive.
