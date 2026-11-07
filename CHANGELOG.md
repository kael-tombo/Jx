# Changelog

All notable changes to jx are recorded here. The project follows
[Semantic Versioning](https://semver.org/). V1's exit-code contract is in
[docs/exit-codes.md](docs/exit-codes.md).

## [Unreleased]

### Added

* `jx why` checks the annotation-processor graph when `[processors]` is
  declared, labels processor paths, and shows both paths when one artifact
  serves both roles. An explicit version limits the answer to graphs that
  resolve to that version.
* `jx build`, `jx test` and `jx clean` forward arguments after `--` to a
  delegated Maven or Gradle build (`jx build -- --parallel`,
  `jx test -- --tests app.LibTest`). Arguments are passed as separate argv
  entries, never through a shell. Under the jx engine trailing arguments are
  a usage error instead of being dropped; jx's own `--filter` and
  `--reports-dir` remain refused in delegated mode.
* `jx lock` records the `[jdk]` pin — provider, feature, exact release and
  install-archive sha256 — from the newest installed JDK for the manifest's
  `jdk` feature, replacing an earlier pin for that feature without ever
  downloading a JDK as a side effect. Build, run, test, package and export
  prefer that exact installed release and warn before falling back to a
  different build of the same feature.
* Framework launch rules stay faithful to each runtime. `vertx` runs a
  hand-written main directly and launches a verticle through the locked
  Vert.x launcher (`io.vertx:vertx-launcher-application` on Vert.x 5,
  `io.vertx.core.Launcher` before it) with `Main-Verticle` in the fat jar;
  `javafx` runs a separate launcher class from a portable thin jar;
  Boot 4 keeps the Boot 3.2+ layout and needs JDK 17 or newer; Micronaut
  warns without `io.micronaut:micronaut-inject-java`; `jx run` refuses
  Quarkus before augmentation; and `jx export`/`jx image` refuse entries
  they cannot reproduce. `javafx`, `javaee` and `j2ee` are accepted as
  framework names.
* `jx update` refreshes wanted dependency prefixes and reports each concrete
  version transition. With a direct target it pins every other direct root
  to its locked version, so one update does not refresh the whole manifest;
  exact pins and platform prefixes are explicit exceptions.
* `jx import` (Maven, Gradle and jot) sets `[package] framework` from the
  imported dependency groups — `org.springframework.boot`, `io.quarkus`,
  `io.micronaut`, `io.vertx`, `org.openjfx` and `jakarta.platform` — and
  reports the detection in the import notes. Previously every import came in
  as `plain`, so an imported Quarkus app would launch unaugmented bytecode
  instead of being refused, and Boot/Vert.x/JavaFX apps lost their
  packaging and launch rules. Test-only dependencies never trigger
  detection.
* `jx fetch` now installs the exact JDK release recorded in `jx.lock` from
  Adoptium's release endpoint and verifies the archive against the pinned
  sha256. An installed copy or a verified cached archive needs no network;
  a mismatched exact release is an error, never an upgrade to the latest
  build of that feature.
* The live Maven Central workflow now runs on Linux and Windows; the Windows
  job covers the MSVC release ABI while pull-request CI remains hermetic.
* `[package] framework` selects a runnable layout without the framework's
  build plugin. `spring-boot` writes the Boot 3.2+ executable jar
  (`BOOT-INF/classes`, `BOOT-INF/lib`, loader at the archive root) and names
  the `spring-boot-loader` coordinate to add when it is missing; `micronaut`
  and `vertx` select the merged fat jar; `quarkus` and `jakarta-ee` are
  refused with the build step that owns their layout. Framework packaging
  never changes dependency resolution or compilation.
* `jx test` takes `--filter` with a fully qualified class, `Class#method` or
  a package ending in `.*` (repeatable, comma-separated), and always writes
  Surefire XML to `target/surefire-reports` unless `--reports-dir` moves it.
  A wildcard selector jx cannot translate is a usage error, never a silently
  empty run. Filters are refused under a delegated engine, which owns its
  own test selection.
* `jx import` converts Gradle `annotationProcessor` and
  `testAnnotationProcessor` declarations with a literal version into
  `[processors]` entries, so an imported Gradle build runs its Java
  annotation processors through jx's own `javac` step. Kotlin `kapt`/`ksp`,
  the buildscript classpath and versionless processors are counted in a note
  and still need `[processors]` by hand.
* `jx import` reads `maven-compiler-plugin`'s `<annotationProcessorPaths>`
  from the effective POM (parents and `${property}` interpolation included)
  and writes the entries into `[processors]`. A `<path>` without an explicit
  version, or the same coordinate twice, fails the import before anything is
  written; `<annotationProcessors>` narrowing is reported because jx runs
  every processor on the path.

### Fixed

* Real dependency graphs no longer lose artifacts. The lockfile reader
  accepts dots in artifact names (`jakarta.annotation-api`), and a
  `group:artifact` declared twice in one POM keeps its first declaration
  instead of letting a later test-scoped duplicate (as in
  `logback-classic`) demote `logback-core` and `slf4j-api` out of the
  transitive graph. Found by running a real Spring Boot 4 application.
* Spring Boot executable jars now start with `java -jar`. jx writes
  explicit directory entries for `BOOT-INF/`, nests dependencies under
  their Maven `artifact-version.jar` names instead of content hashes, and
  stores nested jars uncompressed the way the Boot loader memory-maps
  them. Verified with a real Boot 4.1 application on JDK 25.
* Fat and thin jars now carry explicit directory entries, not just files.
  Micronaut discovers bean definitions by listing the
  `META-INF/micronaut/io.micronaut.inject.BeanDefinitionReference/`
  resource directory, which resolves to nothing in a jar without directory
  entries: `jx run` worked but the packaged fat jar died with
  `NoSuchBeanException: ApplicationEventPublisher<StartupEvent>`. Found by
  running a real Micronaut 4 application; verified with its fat jar on
  JDK 25.
* Linux JDK installs accept Temurin archives whose `legal/` entries are
  relative symlinks. The extractor resolves a link from its own directory and
  only rejects targets that climb above the archive root; previously any `..`
  in a target failed the 25.0.5+8 tarball.
* The generated Homebrew formula installs `jx` from the extracted buildpath.
  Homebrew enters the archive's single top directory, so the old `jx-*/jx`
  glob could never match. The packaging guide now installs from a tap, because
  Homebrew 7 rejects standalone formula files.

## [1.0.0] - 2026-11-01

The first public V1 release. jx is a single-binary, Cargo/uv-style front door
for Java: one `project.toml`, one `jx.lock`, and one command path for JDKs,
dependencies, builds, tests, packaging and Maven/Gradle interop.

### Added

* `jx jdk install/list/use/remove`: verified Eclipse Temurin downloads under
  `~/.jx/jdks`, never touching the system JDK.
* `jx new` and `jx init`: app and library scaffolding that needs no JDK.
* `jx add/remove/lock/fetch/tree/why`: Maven Central resolution with
  nearest-wins conflict reporting and a SHA-256-pinned lockfile.
* `jx build`, `jx test`, `jx run`: fingerprint-skipped `javac`, separate
  main/test classpaths, JUnit 5 execution and a plain-main runner with
  `--debug`/`--port`. Annotation processors declared in `[processors]`
  resolve as their own graph and run through `javac -processorpath`; their
  jars are pinned by `jx.lock` schema 3 and kept off the application
  classpath.
* `jx package` (thin jar and `--fat`), `jx image` (jdeps-trimmed jlink runtime
  with a full-runtime fallback), `jx clean`.
* `jx import` for Maven POMs, the declarative Gradle Kotlin/Groovy subset
  and Java-only `jot.toml` projects (including `libs.versions.toml` catalogs
  and `[processors]`);
  `jx export maven/gradle`; and opt-in delegated `build`/`test`/`clean` via
  `[build] engine` or `--engine`.
* `jx script`: opt-in delegation of `.java`, `.jsh`, `.kt`, `.groovy` and
  markdown scripts to JBang, with `JX_JBANG`, `--offline` forwarding, a pinned
  JDK when the script is inside a jx project, and JBang's own exit code.
* Release-generated Chocolatey, Homebrew, Scoop and winget manifests, every
  one pinned to the archive hashes in `SHA256SUMS`; a Chocolatey `.nupkg` is
  attached for testing but not pushed to the community feed.
* `jx doctor`: home, cache, network, system Java and disk diagnostics, plus a
  read-only report of the optional JBang (including a `JX_JBANG` override) and
  platform package-manager tools. Their absence is informational.
* Release archives for Linux x64/arm64, macOS x64/arm64 and Windows x64, a
  `SHA256SUMS` file, GitHub build provenance attestations, `install.sh` and
  `install.ps1`. Both installers verify the checksum before installing.
* A tag-driven release workflow that refuses a tag which does not match the
  workspace version.

### Fixed

* `jx test` now invokes the JUnit Platform console launcher through its
  `execute` command when the locked launcher is 1.10 or newer, removing the
  deprecated-delegation warning on current JUnit while keeping older pinned
  launchers working through the legacy invocation.
* `jx image --full` now works on Temurin 24+ JDKs, which no longer ship
  `jmods/` (JEP 493), by linking the JDK's explicit module list; the
  `jdk.jlink` and `jdk.jpackage` tool modules are omitted because a
  JMOD-less JDK cannot link them. jlink failures now include stdout when the
  tool writes its diagnostic there.
* `jx image` now exposes cached artifacts to jdeps through `.jar`-named
  scratch links, so projects with dependencies produce a trimmed runtime
  instead of falling back to a full one.

### Security

* JDK downloads stream their SHA-256 and are bounded by the provider's stated
  size or a 512 MiB fallback; cached archives are re-hashed by streaming.
* JDK extraction is bounded at 4 GiB expanded and 200,000 entries, rejects
  special files, and keeps the existing path-traversal checks.
* HTTPS→HTTP redirects are refused by both HTTP clients; repository
  credentials are never forwarded across redirects.
* Repository and mirror URLs with embedded `user:password@` are rejected, and
  proxy credentials are redacted from diagnostics.
* A one-time warning is printed when credentials would cross plain HTTP.
* The full review is in [docs/security-checklist.md](docs/security-checklist.md).

### Known limitations

* Annotation processors need explicit versions in `[processors]` (no
  platform BOMs), a GA at different versions in the app and processor graphs
  must be aligned with `[overrides]`, and `jx why` does not cover the
  processor graph.
* `jx update` is selective for direct roots only; it does not update a
  transitive artifact by itself, and exact pins or platform prefixes need an
  explicit manifest change. Without a target it refreshes every wanted
  prefix.
* Delegated `run`/`package` are not supported; use the build tool or
  `--engine jx`.
* `jx script` needs JBang installed; it does not bundle or download JBang,
  and it does not read JBang catalogs, aliases or `//DEPS`.
* `jx import` accepts Java-only, single-module jot projects. jot workspaces,
  path dependencies, Kotlin toolchains and non-Temurin pins are refused;
  jot's default flat layout (including an omitted `layout`) is imported with a
  note that its sources must move to the Maven layout.
* OS package-manager manifests are generated and attached to a release, but
  Chocolatey, Homebrew, Scoop and winget submissions remain manual.
* Release artifacts are not maintainer-signed, and macOS/Windows binaries are
  not code-signed or notarized yet.
* Fuzzing and reproducible-build verification are not complete.

The detailed, milestone-by-milestone list is in
[docs/KNOWN_LIMITATIONS.md](docs/KNOWN_LIMITATIONS.md).
