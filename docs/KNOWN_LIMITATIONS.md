# Known limitations

jx must never claim support for something untested. This file records honest
gaps, milestone by milestone.

## Current (V1 1.0.0)

Live commands: `jx --version`, `jx doctor`, `jx jdk install/list/use/remove`,
`jx new`, `jx init`, `jx import` (Maven, Gradle and jot), `jx add`, `jx remove`,
`jx lock`, `jx update`, `jx fetch`, `jx tree`, `jx why`, `jx build`, `jx test`, `jx run`,
`jx script`, `jx package`, `jx image`, `jx export maven`, `jx export gradle`,
`jx clean`, plus delegated `jx build`/`test`/`clean` through `[build] engine`
or the global `--engine` flag. M2 delivered the repository client (fetch with
always-on checksum verification, mirrors, proxies, auth references, content
cache), POM/`maven-metadata.xml` models, Maven version ordering and the
effective-POM loader. M3 added the resolver, the lockfile, the six dependency
commands, and the lock fast path that puts cached jars on javac's and java's
classpath. M4 added fingerprint-skipped javac builds, separate main/test
dependency classpaths, test compilation, JUnit 5 execution and annotation
processors from `[processors]` (ADR 0014). M5 added thin
jars, fat jars, the `target/dist` layout and Maven POM export. M6 added
`jx image`, which builds a jdeps-trimmed jlink runtime beside the app and
falls back to a full runtime when analysis cannot trim. M7 added Maven and
Gradle import, Maven and Gradle export, and delegated builds; compatibility
mode is the `[build] engine` / `--engine` switch. The post-V1 adapters are
JBang script delegation (ADR 0015), `jot.toml` import (ADR 0016) and generated
OS package manifests (ADR 0017); the last are release artifacts, not
publications. M8 added the release archives, the checksummed installers, the
tag-driven release workflow and the [security review](security-checklist.md).

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
- **`jx update` refreshes wanted prefixes, not exact pins.** A concrete
  version stays put until the manifest changes. `jx update <direct>` pins
  every other direct root to its locked version; a prefix on a platform
  entry is refused, and a transitive-only update is not supported. Without
  a target, every prefix is re-expanded.
- **Manual manifest edits are detected by mtime** on the build path: if
  `project.toml` is newer than `jx.lock`, `jx build`/`run` re-lock (or fail
  under `--locked`). A manifest restored with an older timestamp can leave a
  stale lock until `jx lock` is run explicitly.

### Repository client (M2)

- **The client is exercised by integration tests** against a local fake
  repository (fetch, mirrors, auth, cache, effective POMs) and by the CLI's
  M3 end-to-end tests. An ignored live test,
  `cargo test -p jx-cli --test live_maven -- --ignored`, exercises `why`,
  `lock` and offline `fetch` against Maven Central. It requires network
  access; pull-request CI stays hermetic, while the `Live Maven Central`
  workflow runs it weekly and on manual dispatch on Linux and Windows.
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
- **The lockfile's `[jdk]` pin tracks the installed toolchain**: `jx lock`
  records the newest installed JDK for the manifest's `jdk` feature, its
  exact release and the cached install archive's sha256, replacing an
  earlier pin for that feature. It never downloads a JDK as a side effect,
  so a machine with none installed keeps a matching existing pin, or stays
  unpinned until a build, `jx jdk install` or `jx fetch` installs one.
  `jx build`, `jx run`, `jx test` and the package/export paths prefer that
  exact installed release and warn before falling back to another build of
  the same feature; `jx fetch` installs the exact release the lock names,
  verifying its cached or downloaded archive against the locked sha256.

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
- **Annotation processors run from `[processors]` only.** Every entry needs
  an explicit version and cannot be a `{ platform = … }` BOM: javac receives
  the resolved jars on `-processorpath` for main and test compilation, and
  never scans the application classpath for processors. The processor graph
  resolves independently from the app graph (ADR 0014). A GA present in both
  graphs at different versions makes `jx lock` fail with an override hint,
  because one lockfile records one version per GA; the same version in both
  roles becomes one shared lock entry (`processor = true`, still an app
  artifact). `jx tree` prints a processor section; `jx why` checks both
  graphs, labels processor paths and shows both paths when one artifact
  serves both roles. The processor path is not exported to Maven or Gradle.
- **`jx test` requires the JUnit Platform console standalone on the test
  classpath.** It is usually added with
  `jx add --dev org.junit.platform:junit-platform-console-standalone`. The
  command scans the test classes and propagates the launcher's exit code.
  Surefire XML is always written to `target/surefire-reports` and
  `--reports-dir` moves it. `--filter` narrows the run to a fully qualified
  class, `Class#method` or a package ending in `.*` (repeatable and
  comma-separated); wildcard selectors elsewhere, JUnit configuration flags
  and parallel-execution switches are not supported, and an unreadable
  filter is refused rather than guessed at. Filters are refused when a
  delegated engine owns the build, because that tool selects its own tests.
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
- **POM repositories, plugin configuration, resource directories and
  Maven's main-class settings are not migrated.** jx only trusts
  repositories in `config.toml`; a main class is discovered at build time or
  pinned with `[run] main`. `maven-compiler-plugin`'s
  `<annotationProcessorPaths>` is the one plugin setting that is migrated:
  each `<path>` with an explicit version becomes a `[processors]` entry,
  inherited through the parent chain until a POM declares its own list.
  `<annotationProcessors>` narrowing, processor `<arguments>`, `<executions>`
  and every other plugin configuration are not reproduced; the note names
  the class list when a POM narrows it.
- **Import validates the generated text with `jx-manifest` before writing.**
  It refuses to replace an existing manifest without `--force`; `--dry-run`
  prints the conversion and notes without touching the filesystem.
- **Import sets `[package] framework` from the dependency groups.**
  `org.springframework.boot`, `io.quarkus`, `io.micronaut`, `io.vertx`,
  `org.openjfx` and `jakarta.platform` (in that precedence order) select
  their framework; annotation processors count, test-only dependencies do
  not, and narrower `jakarta.*` groups stay plain. The detection is noted,
  and removing `[package] framework` restores a plain jar when the guess is
  wrong.

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
  Quarkus plugin also gets a note that its plugin build is not reproduced.
  `[package] framework` is set from the dependency groups as in the Maven
  import above, never from a plugin. Repositories, the
  `buildscript` classpath, custom
  configurations and dependency configuration blocks (excludes, transitive
  flags) are not imported; each is noted.
- **Scopes are widened.** `runtimeOnly` and `compileOnly` become
  `[dependencies]`, and test-only configurations become
  `[dev-dependencies]`.
- **Java annotation processors are imported.** `annotationProcessor` and
  `testAnnotationProcessor` declarations with a literal version become
  `[processors]` entries, and jx runs them through `javac -processorpath`
  for main and test compilation; one coordinate declared for both is
  written once. `kapt`, `ksp`, the `buildscript` classpath and a processor
  without a literal version are counted in a note and must be added to
  `[processors]` by hand. Processor arguments (`-A…`) and Gradle's
  generating-processor configuration are not reproduced.
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
  jx prints one progress line unless `--quiet`. Arguments after `--` are
  forwarded verbatim as separate arguments after the mapped goal, so
  `jx build -- --parallel`, `jx test -- --tests app.LibTest` and
  `jx clean -- --refresh-dependencies` reach the tool without a shell. jx's
  own `--filter` and `--reports-dir` stay refused under delegation because
  the tool owns test selection. With the jx engine, trailing arguments are a
  usage error rather than silently ignored.
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

### Tool composition

- **Maven/Gradle delegation is the only external build-tool adapter.** It is
  opt-in, diagnosed when the tool is absent, and returns the tool's own exit
  code (ADR 0012). That shape is the rule for later adapters (ADR 0013).
- **JBang is delegated, not absorbed.** `jx script <file> [args...]` starts
  `jbang`, forwards `--offline` and returns JBang's exit code. It resolves
  `JX_JBANG` first, then `jbang` on `PATH`; a missing tool is an error with
  the install page, reported before any JDK download. The nearest manifest
  above the script supplies the pinned JDK via `JAVA_HOME`; outside one,
  JBang's own Java policy is untouched. jx does not
  read JBang catalogs, aliases or `//DEPS`, and there is no fallback runner
  (ADR 0015). CI uses a fake JBang; a real run with JBang 0.143.0 and JDK 17
  on Windows x64 ran `hello.java` with two arguments and returned 0.
- **jot import is one-way and deliberately partial.** `jx import` reads
  Java-only, single-module `jot.toml` projects, including a version catalog
  beside the manifest or at the workspace root and annotation processors, and
  refuses workspaces, path dependencies, Kotlin toolchains and non-Temurin
  pins. jot's default flat
  layout (including an omitted `layout`) is imported with a note that its
  sources must move to the Maven layout; `[publish]` and `[project] group`
  are not imported, and `jx export` is the publishing exit (ADR 0016).
- **OS installers are neither wrapped nor detected as JDK providers.**
  Chocolatey, Homebrew, Scoop, winget, SDKMAN and the platform installer
  remain the owners of OS-level installs. The release workflow generates
  manifests for the first four and attaches a Chocolatey `.nupkg`, but does
  not submit them; jx installs and lists only its own Temurin store, so a JDK
  those tools installed is not adopted by `jx build` yet (ADR 0017).
- **No package manager, runtime or build-tool clone.** jx will not grow an OS
  installer, a bundled JRE, a Maven/Gradle script evaluator or a JBang-style
  script runner. An adapter is added only when it removes a wheel jx would
  otherwise reimplement, and it needs an ADR describing ownership, absence
  diagnostics and fake-tool plus real-tool verification.

### Framework packaging

- **`[package] framework` writes a layout and launch entry, not a plugin build.**
  It does not run the Spring Boot, Micronaut, Vert.x or Quarkus plugins and
  does not read their configuration: `spring-boot` writes the Boot
  3.2-and-later executable layout (including Boot 4), `micronaut` selects the
  merged fat jar, `vertx` selects the merged fat jar launched through the
  locked Vert.x launcher, `javafx` writes a portable thin jar for a separate
  launcher class, and `quarkus` and `jakarta-ee` are refused with the build
  step that owns the layout.
- **Spring Boot needs its loader declared.** No Boot starter depends on
  `org.springframework.boot:spring-boot-loader`, so the project must add it
  at the same Boot version; `jx package` names the exact `jx add` command.
  The 3.2+ `org.springframework.boot.loader.launch` layout is required, and
  an older loader is refused before `target/dist` is touched. Boot 4 needs
  JDK 17 or newer. Boot's `repackage` options (excludes, layers,
  `loader.path`, per-script start classes) are not reproduced.
- **Micronaut needs its annotation processor declared.** jx warns when a
  Micronaut application has no `io.micronaut:micronaut-inject-java`
  processor, because the project compiles but bean metadata is missing.
- **Vert.x needs a launcher and a verticle.** A hand-written Vert.x main
  runs directly, but a verticle needs `[run] main`, Vert.x 5 additionally
  needs `io.vertx:vertx-launcher-application`, and launcher-based entries
  cannot be reproduced by `jx export`. Both `micronaut` and `vertx` write
  `target/dist/<name>-<version>-all.jar`, the name their shadow/fat-jar
  builds use; there is no variant selection beyond that.
- **JavaFX stays on the classpath through a launcher.** The `Application`
  subclass cannot be launched directly, so `[run] main` must name a separate
  launcher class; a project with more than one platform classifier for the
  same JavaFX artifact is refused because one thin jar cannot bundle every
  native build. `jx image` still needs the thin `target/dist` layout, so it
  refuses Boot, Micronaut and Vert.x executables.

### Toolchain (from M1)

- **The JDK provider is Eclipse Temurin over the Adoptium API only.** Other
  providers are behind `JdkProvider` but not implemented, and OS-installed
  JDKs are not adopted yet (ADR 0013).
- **JDK archives download with only the API-provided sha256 as
  verification.** GPG signing is an open owner action in the
  [security checklist](security-checklist.md); it is not configured.
- **`--offline` refuses a missing pinned JDK** when it is neither installed
  nor cached; online `jx fetch` installs the exact release from Adoptium and
  verifies it against the sha256 in `jx.lock`. A cached archive is re-hashed
  before extraction.
- **`jx run` and `jx test` share the pinned JDK**; locked dependencies now
  reach both classpaths, and `jx lock` records the newest installed JDK of
  the pinned feature. Build paths use that exact installed release when it
  is present and warn (unless `--quiet`) before falling back to another
  build of the same feature. `jx fetch` restores the locked release, not the
  latest build.

### Doctor, config, cache

- **config.toml and project.toml have strict diagnostics** (unknown keys get
  did-you-mean; auth must be `env:` references; dependency coordinates are
  validated at parse time).
- **Doctor reports the optional adapters from `PATH` only.** JBang is looked
  up with a `JX_JBANG` override first, and the platform package managers are
  listed by name; nothing is executed and no version is parsed. A missing tool
  is informational and never changes the exit code, because jx only delegates
  to them.
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
- **OS package manifests are generated from `SHA256SUMS`** by
  `scripts/render-packages.mjs` and attached to the release. A Windows job
  packs the Chocolatey source into a `.nupkg` and attaches it too. None of
  them is pushed to Chocolatey, Homebrew, Scoop or winget; those submissions
  need the maintainer's account and review. The winget manifests pass
  Microsoft's `winget validate` locally. A Windows CI job runs the renderer
  and then `choco pack` and the PowerShell/XML checks with the runner's real
  Chocolatey and Ruby, so template breakage fails on every push rather than at
  the tag; the Ubuntu site job supplies the `ruby -c` check for the Homebrew
  formula. Homebrew's own audit and an actual Scoop install are still
  unverified, and the release jobs are first exercised by a real version tag.
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
