# Known limitations

jx must never claim support for something untested. This file records honest
gaps, milestone by milestone.

## Current (V1 1.0.0; annotation processors still deferred from M4)

Live commands: `jx --version`, `jx doctor`, `jx jdk install/list/use/remove`,
`jx new`, `jx init`, `jx import` (Maven and Gradle), `jx add`, `jx remove`,
`jx lock`, `jx fetch`, `jx tree`, `jx why`, `jx build`, `jx test`, `jx run`,
`jx package`, `jx image`, `jx export maven`, `jx export gradle`, `jx clean`,
plus delegated `jx build`/`test`/`clean` through `[build] engine` or the
global `--engine` flag. M2 delivered the repository client (fetch with
always-on checksum verification, mirrors, proxies, auth references, content
cache), POM/`maven-metadata.xml` models, Maven version ordering and the
effective-POM loader. M3 added the resolver, the lockfile, the six dependency
commands, and the lock fast path that puts cached jars on javac's and java's
classpath. M4 added fingerprint-skipped javac builds, separate main/test
dependency classpaths, test compilation and JUnit 5 execution. M5 added thin
jars, fat jars, the `target/dist` layout and Maven POM export. M6 added
`jx image`, which builds a jdeps-trimmed jlink runtime beside the app and
falls back to a full runtime when analysis cannot trim. M7 added Maven and
Gradle import, Maven and Gradle export, and delegated builds; compatibility
mode is the `[build] engine` / `--engine` switch. The post-V1 JBang and
OS-installer adapters are described in ADR 0013 and are not wired yet. M8
added the release archives, the checksummed installers, the tag-driven release
workflow and the [security review](security-checklist.md).

### Manifest

- **Dependency tables are parsed and validated** (`docs/manifest.md` rules:
  coordinate shape, `=` exact pins, SNAPSHOT rejection, `platform` BOM
  entries, dupes within a table) and `[overrides]` too (`group:artifact`
  keys, version values, SNAPSHOT rejection). Both are consumed by the
  resolver (`jx-resolve`, below).
- **`jx add`/`jx remove` edit the file as text**, preserving comments,
  ordering and line endings. Both commands resolve and rewrite `jx.lock`
  afterwards; a failure restores the manifest byte for byte. One coordinate
  per invocation, and a versionless add needs a reachable version listing
  (it writes the latest stable release).

### Resolver (M3 core, `jx-resolve`)

- **Nearest-wins with loser-subtree alignment, every fork reported** — the
  conflict list is part of the resolution result; nothing trims silently
  (ADR 0004). Equal depth = first declaration wins, also reported.
- **Wanted prefixes expand against the repositories' listings** to the
  highest *stable* release (pre-releases never auto-picked; `=` pins exact).
  Verified by unit + fake-repo tests, not against a real Maven remote (no
  network in CI).
- **Platform BOMs supply versions** to same-table entries that name none;
  the BOM itself is never a build artifact.
- **Exclusions are honored below the edge that declares them**; a nearer
  winning path's inclusion is not undone by a losing path's exclusion (V1
  simplification, Maven-shaped).
- **Transitive SNAPSHOTs are rejected** with an actionable message; V1's
  lockfile pins bytes, which re-resolving snapshot builds cannot satisfy.
- **POM-declared `<repositories>` are parsed but deliberately not fetched
  from** — `config.toml` is the only trust anchor in V1.
- **Main and test scopes are separated** for compiling and running: main
  code receives only main artifacts, tests receive main plus
  `[dev-dependencies]`. There is no separate runtime versus compile scope
  yet — Maven's `provided`/`runtime` scopes are not represented in the
  manifest. `test` and `provided` dependencies of a dependency are not
  transitive (Maven's rule), so build-only tools no longer leak into every
  consumer's graph.
- **Artifacts are assumed to be jars.** POM dependencies with a non-jar
  `type` (AAR, test-jar, …) are fetched at the `.jar` path today; the
  manifest grammar has no `type` field in V1.
- **`jx tree`/`jx why` re-resolve** (from the content cache when offline);
  they do not reconstruct the graph from `jx.lock`. The lockfile stores
  artifacts, not parent edges or a manifest hash.
- **`jx lock` re-expands wanted prefixes**, so it is the explicit refresh
  command in V1; there is no separate `jx update`. A concrete version stays
  put only while the resolved artifact set is unchanged.
- **Manual manifest edits are detected by mtime** on the build path: if
  `project.toml` is newer than `jx.lock`, `jx build`/`run` re-lock (or fail
  under `--locked`). A manifest restored with an older timestamp can leave a
  stale lock until `jx lock` is run explicitly.

### Repository client (M2)

- **The client is exercised by integration tests** against a local fake
  repository (fetch, mirrors, auth, cache, effective POMs) and by the CLI's
  M3 end-to-end tests; live Maven Central is not part of CI.
- **Effective-POM support is reached by the CLI through the resolver**:
  parent-chain fetching, inheritance merging, `${...}` interpolation and BOM
  (`scope=import`) consolidation live in `jx_maven::PomLoader`. Dependency
  group/artifact ids, exclusions and `${project.<property>}` spellings are
  interpolated too; before M7 only versions and classifiers were. POM
  `<repositories>` are parsed and merged but nothing fetches from them yet.
- **Snapshot resolution is releases-only offline**: `--offline` refuses
  snapshots, and the lockfile does not record a timestamped snapshot form
  yet, so snapshot builds cannot be reproduced offline.
- **Checksums are unconditional** (Maven 4 `fail` semantics): a repository
  that publishes neither response headers nor `.sha1`/`.sha256` sidecars is a
  hard error, never a warning. `?checksum=`-style URL overrides unsupported.
- **Response bodies are capped at 64 MiB.** A response that would run over
  the cap is refused with a "too large" message — never silently truncated,
  so an oversized artifact can no longer masquerade as a checksum mismatch.
- **Mirrors follow Maven's `mirrorOf` grammar, not globs**: `repo*` is a
  literal id, `external:*`/`!id` behave exactly as Maven's selector does.
- **Auth references are `env:VAR` / `bearer:env:VAR` only**; the secret value
  is only ever read from the environment at fetch time.
- **SHA-1 is a transport check only**; the integrity chain is the
  sha256-pinned lockfile written by `jx lock`/`jx add` and re-checked by
  `jx fetch` and every build's classpath fast path.
- **The lockfile's `[jdk]` pin is read but not written yet**: `jx lock`
  records dependencies only, so `jx fetch` validates a JDK pin only when one
  exists (hand-written or from a future release).

### POM model (M2)

- **No active profiles.** Profile-gated repositories/dependencies/properties
  are ignored entirely (Maven honours them); recorded in ADR 0003.
- **POM repositories are parsed but not fetched from** — V1 merges and
  diagnoses them, but `config.toml` remains the only trust anchor (ADR 0003).

### Build and test (M4)

- **Compilation is whole-source-set, not per-file.** When anything in a
  command's fingerprint changes, every source in that set is recompiled and
  the output directory is cleared first, so deleted sources cannot leave
  stale classes. An unchanged main build skips javac entirely; unchanged
  test sources skip test compilation only when the main build also skipped.
- **Annotation processors are not supported yet.** `[processors]` remains a
  planned manifest table; `javac` runs without `-processorpath` or
  `-processor` flags. This is the part of M4 still open.
- **`jx test` requires the JUnit Platform console standalone on the test
  classpath.** It is usually added with
  `jx add --dev org.junit.platform:junit-platform-console-standalone`. The
  command scans the test classes and propagates the launcher's exit code;
  there are no test filters, JUnit configuration flags or Surefire XML
  reports yet.
- **Test resources** under `src/test/resources` are copied into
  `target/test-classes`, matching the main-resource rule.
- **Build fingerprints are local, not remote.** A fresh checkout or a
  `jx clean` recompiles; jx does not share compilation outputs between
  machines.
- **`jx build` does not produce a jar**; use `jx package` for artifacts.

### Packaging (M5)

- **Thin packaging** writes `target/dist/<name>-<version>.jar` with the
  project's classes and resources; dependencies are separate jars under
  `target/dist/lib/<group path>/`.
- **Fat packaging** (`jx package --fat`) writes one
  `target/dist/<name>-<version>-all.jar` and no `lib/`. Project entries win
  duplicate class/resource paths; dependency duplicates are first-wins in
  lock order. `META-INF/services` files are merged and de-duplicated.
  Dependency manifests, signature files, `META-INF/INDEX.LIST` and
  `module-info.class` are omitted; multi-release `META-INF/versions/` entries
  are skipped and counted. These skips and duplicate counts are printed.
- **The thin `Class-Path` manifest uses relative paths** into `lib/`; the
  whole `target/dist` tree can be moved and still run with `java -jar` from
  any working directory. The thin jar is not standalone without its `lib/`
  tree.
- **Fat jars are not a general shaded-jar replacement yet.** Multi-release
  dependency behavior is lost, `module-info` is omitted, and duplicate
  resources do not get artifact-specific names. There is no jar signing.
- **`jx package` does not create a runtime.** Use `jx image` for a jlink
  runtime; packaging and image building share the same `target/dist`, and
  packaging rebuilds it.
- **Packaging is application and library friendly**: a discovered or pinned
  main class becomes `Main-Class`; a library with no main class still gets a
  jar, without that header. Ambiguous main classes fail rather than choosing
  one silently.
- **No launch scripts or native installers.** Users run `java -jar` today.
- **`jx clean` removes `target/dist`** with the rest of `target/`.

### Runtime images (M6)

- **`jx image` builds a trimmed runtime at `target/dist/runtime`** from the
  pinned JDK, then prints the exact `runtime/bin/java -jar` command. The
  application must have a main class; libraries should stay with
  `jx package`.
- **`jdeps` sees class references, not reflection or dynamic loading.** A
  trimmed image can still fail if the app loads modules by name. jx prints a
  note and offers `jx image --full`; it does not claim runtime correctness.
- **`--ignore-missing-deps` is deliberate.** Optional annotation classes do
  not block analysis, but a genuinely missing dependency can also leave the
  module set incomplete. Use `--full` when the app needs a guaranteed
  runtime.
- **Fallback is explicit.** When `jdeps` cannot run or produces no module
  list, jx builds a full runtime from the JDK's own module list and reports
  that mode. The full image is much larger; `--full` skips analysis
  deliberately.
- **A JMOD-less JDK cannot link its jlink/jpackage tools.** Temurin 24+
  enables JEP 493 and no longer ships `jmods/`. Full and fallback images
  therefore omit `jdk.jlink` and `jdk.jpackage` (the latter requires the
  former); the remaining modules link normally. An application that invokes
  `jlink` or `jpackage` at runtime needs a JDK with packaged modules.
- **Images are atomic and verified.** `jlink` writes a temporary directory,
  jx starts it with `--list-modules` and checks every requested module, then
  replaces the previous runtime. Before packaging, the whole previous
  `target/dist` is backed up and restored if any later step fails, so a failed
  image does not destroy a working app/runtime pair.
- **No `--compress`, cross-compilation, signing or jpackage.** The image is
  for the current platform and pinned JDK; compression flags changed across
  JDK releases, so none is passed. JDK 8 is refused before writing.
- **`jx package` removes `target/dist/runtime`** because it rebuilds `dist`
  from scratch. Run `jx image` again when the runtime is needed.

### Maven import (M7)

- **`jx import [pom]` converts an effective POM into `project.toml`.** Parent
  POMs at `<relativePath>` are read from disk; missing parents and imported
  BOMs are fetched through the checksum-verified repository client. No Maven
  process or local Maven installation is required.
- **Profiles are ignored**, exactly as they are during normal resolution
  (ADR 0003). A dependency that only exists inside an active Maven profile
  does not appear in the imported manifest.
- **Only `jar` packaging is accepted.** Aggregator `pom` projects must point
  `jx import` at an application module; `war`, `ear` and other packagings are
  refused with an explanation.
- **Compile, runtime and provided dependencies become `[dependencies]`;
  test dependencies become `[dev-dependencies]`.** jx has no separate runtime
  or provided scope: those dependencies join the compile classpath and are
  also packaged. System-scope dependencies (a local `systemPath`) are skipped
  with a note because they cannot be resolved from a repository.
- **Non-jar artifact types are skipped with a note** (test-jar, AAR, native
  types). `optional` is imported as a normal dependency because jx does not
  model optionality.
- **Every imported direct version is exact-pinned (`=version`).** BOM imports
  are expanded at import time and are not written as platform entries, so the
  manifest records what Maven selected rather than a BOM chain.
- **Missing versions, unresolved properties, version ranges and SNAPSHOT
  dependencies fail the import.** Nothing is written: jx does not guess a
  replacement. Two classifiers of the same `group:artifact` are refused for
  the same reason.
- **Exclusions are not represented.** A Maven exclusion that removed a
  transitive artifact can be lost, so jx's resolved graph may be larger than
  Maven's. The import prints how many exclusions it saw.
- **Test dependencies are imported, but `jx test` still needs the JUnit
  Platform console standalone.** Most Maven POMs declare Surefire and
  `junit-jupiter` instead; the import note says to add
  `org.junit.platform:junit-platform-console-standalone` to
  `[dev-dependencies]` before running `jx test`.
- **POM repositories, plugin configuration, resource directories,
  annotation-processor configuration and Maven's main-class settings are not
  migrated.** jx only trusts repositories in `config.toml`; a main class is
  discovered at build time or pinned with `[run] main`.
- **Import validates the generated text with `jx-manifest` before writing.**
  It refuses to replace an existing manifest without `--force`; `--dry-run`
  prints the conversion and notes without touching the filesystem.

### Gradle import (M7)

- **Only the declarative subset is read.** `jx import app/build.gradle.kts`
  (or `build.gradle`, or a directory containing one) understands dependency
  declarations in string and Groovy map form, `platform(...)` imports,
  `constraints`, literal `ext`/`def`/`val` variables, the Java toolchain and
  the application main class. Gradle is never executed.
- **Anything that needs evaluation is refused**, with a file and line:
  unresolved interpolation, version catalogs (`libs.*`), `project(...)`,
  `files(...)`, `fileTree(...)`, dynamic versions/ranges, SNAPSHOTs, and
  dependency declarations inside `if`/`for`/`while` or `allprojects`/
  `subprojects` blocks. A dropped dependency would build differently, so the
  import stops instead of guessing.
- **Versionless dependencies need a platform BOM.** When the build applies
  the Spring Boot plugin at a literal version, the import adds
  `org.springframework.boot:spring-boot-dependencies` at that version as a
  platform, because that is the BOM the plugin applies; the note says so and
  `jx lock` verifies it. Other plugin-managed versions cannot be known
  statically and must be given an explicit version or platform.
- **Plugins are never migrated.** Their count is noted; a Spring Boot or
  Quarkus plugin also gets a note that executable-jar repackaging is not
  reproduced. Repositories, the `buildscript` classpath, custom
  configurations and dependency configuration blocks (excludes, transitive
  flags) are not imported; each is noted.
- **Scopes are widened.** `runtimeOnly` and `compileOnly` become
  `[dependencies]`, test-only configurations become `[dev-dependencies]`,
  and annotation processors are skipped (jx does not run them yet).
- **Constraints become `[overrides]`.** Gradle constraints are
  per-configuration; jx overrides apply to the whole graph. The note says
  so.
- **Both build files in one directory prefer Maven.** The import prints that
  choice; pass the Gradle script explicitly to convert Gradle. Multi-module
  settings are noted, and only the named module is imported.
- **Test dependencies are imported, but `jx test` still needs the JUnit
  Platform console standalone** in `[dev-dependencies]`.

### Maven export (M5)

- **`jx export maven` is an approximate conversion, not a round trip.** It
  writes direct dependencies with the concrete versions from `jx.lock`, dev
  dependencies as `test` scope, classifiers, `<maven.compiler.release>`, the
  jar main class and managed entries for `[overrides]` (ADR 0007). Executable
  exports also copy runtime dependencies to `target/lib` and write a relative
  `Class-Path`, so `java -jar` works after `mvn package`.
- **Platform BOMs are not emitted as imports.** Their effect is inlined into
  the concrete versions of the direct dependencies; Maven's transitive graph
  can still differ from jx's nearest-wins resolution.
- **Annotation-processor paths, jx fingerprints and runtime-image trimming
  are not exported.** The generated POM is a starting point for a normal
  Maven build.
- **The POM is compiled first**: export runs the normal build so a discovered
  main class can be written into the jar plugin.
- **An existing `pom.xml` is never overwritten without `--force`.**
- **The default Maven groupId is `local.jx`**; pass `--group-id` for a real
  coordinate.

### Gradle export (M7)

- **`jx export gradle` writes Kotlin DSL only**: `build.gradle.kts` plus
  `settings.gradle.kts`. A library gets `java-library`; an application gets
  `application` and its main class.
- **It compiles once**, like the Maven exporter, so a discovered main class
  is written. This needs the pinned JDK.
- **Dependencies use the concrete versions from `jx.lock`**: main entries as
  `implementation`, dev entries as `testImplementation`, classifiers
  preserved, and `[overrides]` as dependency `constraints`. Platform BOMs
  are not re-exported; their effect is already in the concrete versions.
- **Only `mavenCentral()` is written.** Repositories, mirrors and auth in
  `config.toml` are machine configuration and are not copied into a project
  file; add private repositories to the generated build by hand.
- **No plugin configuration, annotation processors or runtime images are
  exported.** The file is a working starting point, not a reproduction of
  jx's build.
- **An existing `build.gradle.kts` or `settings.gradle.kts` is only replaced
  with `--force`.** A Groovy `build.gradle` is refused outright so two build
  scripts never coexist.
- **CI does not run Gradle.** The generated Kotlin DSL was verified
  manually against Gradle 8.14.3 (`gradle build` and `gradle run`), but the
  automated suite still has no Gradle distribution.

### Delegated builds (M7)

- **`[build] engine = "maven"` or `"gradle"` delegates `build`, `test` and
  `clean` only.** Maven receives `compile`, `test`, `clean`; Gradle receives
  `classes`, `test`, `clean`. `jx run`, `package`, `image` and `export` are
  refused in this mode because they depend on jx's own classpath, packaging
  or files; use the build tool, or pass `--engine jx` for one command
  (ADR 0012).
- **The external build owns dependency resolution.** `add`, `remove`, `lock`,
  `fetch`, `tree` and `why` are refused while the engine is delegated, since
  editing or describing `project.toml` would not affect the build. The error
  names `--engine jx` and the manifest edit as escapes. Importing a build and
  then delegating is possible, but the imported dependency table is inactive
  until the engine is switched back.
- **Delegation needs `project.toml`.** The manifest supplies the JDK pin; a
  directory with only `pom.xml` or a Gradle build must be imported first.
  There is no manifest-less compatibility mode yet.
- **The tool must exist.** A project wrapper (`mvnw`/`mvnw.cmd`,
  `gradlew`/`gradlew.bat`) wins over `mvn`/`gradle` on `PATH`; neither being
  present is a diagnosed error. jx does not bundle Maven or Gradle, and a
  wrapper's first run may download its distribution even under `--offline`
  (the flag is forwarded to the tool, not to the wrapper download).
- **The pinned JDK is exported to the tool**, not just to jx: `JAVA_HOME` is
  the installed JDK and its `bin/` is first on `PATH`. A missing JDK is
  installed exactly as for a jx build, unless `--offline` is set.
- **The tool's exit code is jx's exit code.** Output is inherited, unmodified;
  jx prints one progress line unless `--quiet`. There is no argument
  pass-through yet, so build-tool flags and test filters need the tool itself.
- **`--locked` is refused for delegated `build` and `test`.** It is a
  jx.lock guarantee, and delegated commands never read jx.lock.
- **`jx clean` under a manifest cleans that project's `target/`**, even when
  invoked from a subdirectory; without a manifest it keeps the old behaviour
  and removes `target/` below the current directory.
- **Windows wrappers run through `cmd /C`; Unix wrappers need the executable
  bit.** A non-executable wrapper fails with the wrapper path and a
  `chmod +x` hint rather than a bare OS error.
- **CI uses fake wrappers and a fake installed JDK**, so neither Maven nor
  Gradle is required. Delegated build, test and clean were also run manually
  against Gradle 8.14.3 with a real JDK 17. Maven was not installed in the
  development environment, so a real Maven delegation run is still owed.

### Tool composition (after V1)

- **Maven/Gradle delegation is the only external build-tool adapter today.**
  It is opt-in, diagnosed when the tool is absent, and returns the tool's own
  exit code (ADR 0012). That shape is the rule for later adapters (ADR 0013).
- **JBang is not wired.** jx does not run `.java` files, JShell snippets or
  JBang catalogs, and there is no `jx script` command. Scripts and
  single-file programs stay with JBang; an optional adapter is post-V1 work,
  not a claimed feature.
- **OS installers are neither wrapped nor detected as JDK providers.**
  Chocolatey, Homebrew, Scoop, SDKMAN and the platform installer remain the
  owners of OS-level installs. jx installs and lists only its own Temurin
  store, so a JDK those tools installed is not adopted by `jx build` yet.
- **No package manager, runtime or build-tool clone.** jx will not grow an OS
  installer, a bundled JRE, a Maven/Gradle script evaluator or a JBang-style
  script runner. An adapter is added only when it removes a wheel jx would
  otherwise reimplement, and it needs an ADR describing ownership, absence
  diagnostics and fake-tool plus real-tool verification.

### Toolchain (from M1)

- **The JDK provider is Eclipse Temurin over the Adoptium API only.** Other
  providers are behind `JdkProvider` but not implemented, and OS-installed
  JDKs are not adopted yet (ADR 0013).
- **JDK archives download with only the API-provided sha256 as
  verification.** GPG signing is an open owner action in the
  [security checklist](security-checklist.md); it is not configured.
- **`--offline` refuses a missing pinned JDK** with a clear error.
- **`jx run` and `jx test` share the pinned JDK**; locked dependencies now
  reach both classpaths, but `jx lock` does not write the `[jdk]` pin yet —
  it only preserves an existing hand-written entry.

### Doctor, config, cache

- **config.toml and project.toml have strict diagnostics** (unknown keys get
  did-you-mean; auth must be `env:` references; dependency coordinates are
  validated at parse time).
- **Doctor's network probe is still DNS+TCP only**; the repository client's
  TLS/proxy behaviour is exercised by the M2 layer, not yet by `jx doctor`.
- **The cache layer serves four consumers**: JDK archives (M1), Maven
  artifacts via the repository client (M2/M3), the build classpath (M3) and
  dependency copies into `target/dist` (M5).

### Release and platform support

- **The release workflow publishes five native targets** when a `v*` tag
  matches the workspace version: Linux x64/arm64, macOS x64/arm64 and Windows
  x64, with `SHA256SUMS` and a GitHub build provenance attestation. The
  installers verify the checksum before installing.
- **Linux binaries are glibc builds, not static.** x64 builds on Ubuntu 22.04
  (glibc 2.35); arm64 builds on Ubuntu 24.04 (glibc 2.39) and will not run on
  an older arm64 distribution until a static build exists.
- **No maintainer signature and no OS code signing.** Release archives are not
  GPG/minisign-signed, and the macOS and Windows binaries are not notarized or
  code-signed. Gatekeeper and SmartScreen warnings are expected. Checksums and
  attestations are the available signals.
- **No reproducible-build check.** The workflow does not set
  `SOURCE_DATE_EPOCH` or compare an independent rebuild.
- **Fuzzing is not part of CI.** Archive extraction, POM parsing and the Gradle
  declaration parser have targeted tests, not a fuzz campaign.
- **The release approver and independent security reviewer are owner actions**;
  see the sign-off table in the [security checklist](security-checklist.md).

### CI / release

- **CI runs fmt/clippy/tests on Linux, macOS and Windows, plus cargo-deny**,
  but milestone end-to-end scenarios are not exercised in CI yet.
- **The tag workflow is the only release path.** A local `cargo build
  --release` produces a working binary, but no archive, checksum or
  attestation until the tag workflow runs.
