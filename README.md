# jx — a Cargo/uv-style front door for Java

> **Status: V1 (1.0.0), release hardening complete.** The source tree covers
> the five-minute path, dependency locking, build/test/run, packaging, runtime
> images and Maven/Gradle interop. Release archives, installers and the
> security record are ready; binaries are published from version tags. See
> [docs/KNOWN_LIMITATIONS.md](docs/KNOWN_LIMITATIONS.md) for the honest state
> and [docs/security-checklist.md](docs/security-checklist.md) for the review.

`jx` is a single-binary command line tool, written in Rust, that manages JDKs,
resolves dependencies from Maven Central into a reproducible lockfile, and
builds, tests, runs and packages Java projects from one small manifest.
North star: on a machine with nothing installed,
`jx new hello && cd hello && jx run` prints a greeting in under five minutes.

## Principles

1. One tool, one manifest, one lockfile.
2. Smooth beats complete — protect the five-minute happy path first.
3. Every error says what happened, where, and what to try next.
4. Sit on top of the ecosystem — Maven Central, the JDK's own tools,
   Maven/Gradle, JBang, the platform installers — and always offer an exit
   (`jx export maven`). Never reimplement a tool that already does the job.
5. Fast: no JVM starts unless needed.
6. Never silent about risk: conflicts, changed checksums and unsupported POM
   features are loud.

## Install

Prebuilt archives are published from `v*` tags. Every release ships five
platforms, a `SHA256SUMS` file and a GitHub build provenance attestation:

* Linux x64 (glibc 2.35 or newer), Linux arm64 (glibc 2.39 or newer)
* macOS x64 and Apple silicon
* Windows x64

Download the archive for your platform from
<https://github.com/kael-tombo/Jx/releases>, then verify it before running:

```sh
sha256sum --check --ignore-missing SHA256SUMS
gh attestation verify jx-v1.0.0-x86_64-unknown-linux-gnu.tar.gz \
  --repo kael-tombo/Jx
tar -xzf jx-v1.0.0-x86_64-unknown-linux-gnu.tar.gz
install -m 755 jx-v1.0.0-x86_64-unknown-linux-gnu/jx ~/.local/bin/jx
```

The installers do the same checksum check, need no `sudo`, and install into
`~/.local/bin` (Unix) or `%LOCALAPPDATA%\jx\bin` (Windows):

```sh
# macOS / Linux: inspect first, then run
curl -fsSLO https://raw.githubusercontent.com/kael-tombo/Jx/main/install.sh
less install.sh
sh install.sh
```

```powershell
# Windows PowerShell: inspect first, then run
Invoke-WebRequest https://raw.githubusercontent.com/kael-tombo/Jx/main/install.ps1 -OutFile install.ps1
notepad install.ps1
powershell -ExecutionPolicy Bypass -File .\install.ps1
```

Use `--version v1.0.0` (or `JX_VERSION`) to pin a release. If no release exists
for your platform yet, build from source; the installers refuse an archive
whose checksum does not match. Release artifacts are not signed by a
maintainer key yet, so the checksum and attestation are the available signals.

## Building from source

```sh
cargo build --release        # jx lands in target/release/jx
cargo fmt -- --check && cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
# optional live Maven Central check; also runs weekly on Linux and Windows in CI
cargo test -p jx-cli --test live_maven -- --ignored --nocapture
```

Rust stable (edition 2024) on Linux (x64, arm64), macOS (x64, arm64) and
Windows (x64).

## Right now

```sh
jx new hello         # scaffold a project (no JDK needed anywhere)
cd hello             # pin: temurin 25, the current LTS
jx run               # first run downloads & installs nothing but the JDK,
                     # then javac-compiles and runs: "Hello, jx!"
jx run --offline     # afterwards, cached runs need no network (~1 s)

# Dependencies and the lockfile
jx add com.google.guava:guava:33
jx tree              # resolved graph, with conflicts named
jx why com.google.guava:guava
jx lock              # resolve and record exact versions + sha256
jx update            # refresh prefix dependencies and print version changes
jx update guava      # update one direct dependency, keeping the others locked
jx fetch             # download/verify exactly what jx.lock pins, including its JDK
jx run               # javac and java get the locked jars on their classpath
jx package           # target/dist: runnable jar + lib/ dependency tree
java -jar target/dist/hello-0.1.0.jar
jx package --fat     # one merged jar: target/dist/hello-0.1.0-all.jar
jx image             # target/dist/runtime: trimmed jlink runtime
jx image --full      # all linkable JDK modules when reflection needs them
jx export maven --group-id com.example  # write a POM from the lockfile
jx export gradle --group-id com.example # write build.gradle.kts + settings

# Coming from Maven, Gradle or jot
jx import            # convert ./pom.xml, build.gradle(.kts) or jot.toml
jx import service/pom.xml --dry-run  # print the manifest and notes only
jx import app/build.gradle.kts --dry-run
jx import jot.toml --dry-run

# Scripts stay with JBang
jx script hello.java Max   # delegate to JBang; its exit code is jx's

# Or keep the native build and delegate build/test/clean to it
#   [build]
#   engine = "maven"    # or "gradle"
jx build              # runs ./mvnw -B compile with the pinned JDK
jx test               # runs the build tool's own test goal
jx --engine maven test  # one-off delegation without editing project.toml

# Tests
jx add --dev org.junit.platform:junit-platform-console-standalone
jx test              # compile src/test/java and run JUnit 5

jx jdk list          # jx-managed JDKs live in ~/.jx/jdks, never near system Java
jx doctor            # diagnose jx's home, cache, network, system Java, disk, and the optional JBang and package-manager tools
```

`jx update` re-resolves wanted prefixes and prints every old → new version.
With a direct dependency (`alias` or `group:artifact`), every other direct
root is pinned to its locked version, so the other direct roots stay put;
transitives can still change with the target. Exact pins and platform
prefixes are not updated selectively. Without a target, every wanted prefix
is re-expanded.

Builds skip javac when sources and the classpath are unchanged. Test sources
are compiled against the main classes plus dev dependencies, and the JUnit
launcher runs only that test classpath. `jx package` writes a thin jar with a
`Main-Class` manifest and a `Class-Path` that points into `dist/lib/`, so the
whole `target/dist` tree can be moved and still run with `java -jar`.
`jx package --fat` instead merges main dependencies into one
`<name>-<version>-all.jar`; duplicate classes are first-wins, `META-INF/services`
files are merged, and signatures/multi-release entries are intentionally
omitted. `jx export maven` writes a POM using the concrete versions from
`jx.lock`, test scope for dev dependencies and managed entries for overrides;
platform BOMs are inlined and an existing `pom.xml` is not overwritten unless
`--force` is passed (ADR 0007). `jx image` asks `jdeps` which modules the app
needs, builds a verified `jlink` runtime under `target/dist/runtime`, and
falls back to a full runtime when analysis cannot trim; use `--full` when
reflection loads modules at runtime (ADR 0009). A full image omits
`jdk.jlink` and `jdk.jpackage`: Temurin 24+ ships without `jmods/` (JEP 493),
and a JMOD-less JDK cannot link those two tool modules into another image. `jx import` reads the
project's effective POM (parents, properties and imported BOMs included),
writes `project.toml` with exact direct-dependency pins and prints every
approximation it made; it refuses to overwrite an existing manifest without
`--force` (ADR 0010). `jx import` also accepts `build.gradle` and
`build.gradle.kts`: it reads the declarative subset — string and map
coordinates, `platform(...)` BOMs, `constraints`, literal `ext`/`val`
variables, the Java toolchain and the application main class — and stops
with a location on anything that needs script evaluation. Versionless
dependencies rely on a platform BOM, including the Spring Boot plugin's own
BOM (ADR 0011). `jx export gradle` writes a Kotlin-DSL build with the locked
versions, dev dependencies as `testImplementation` and `[overrides]` as
constraints.

`[build] engine = "maven"` (or `"gradle"`) keeps the native build
authoritative while jx owns the JDK: `jx build`, `jx test` and `jx clean` run
the project's wrapper (or the installed tool) in the project directory with
`JAVA_HOME` set to the pinned JDK, forward `--offline`, and exit with the
tool's own code. Commands that would edit or describe a graph the build does
not use — the dependency commands, `run`, `package`, `image` and exports —
are refused with a `--engine jx` escape (ADR 0012); `--engine` selects an
engine for one invocation.

## Composition

jx owns the path, not the tools. The JDK owns compilation, tests, linking and
its own runtime tools; Maven Central owns artifact bytes; Maven and Gradle own
the build when `[build] engine` delegates; JBang owns scripts and single-file
programs through `jx script`; Chocolatey, Homebrew, Scoop, winget and the
platform installer own OS installs. The boundary and the rules for adding an
adapter are in
[docs/decisions/0013-compose-the-ecosystem.md](docs/decisions/0013-compose-the-ecosystem.md).
The first three post-V1 integrations are
[0015](docs/decisions/0015-jbang-script-adapter.md) (delegated JBang scripts),
[0016](docs/decisions/0016-jot-import.md) (`jot.toml` import) and
[0017](docs/decisions/0017-os-package-manifests.md) (generated Chocolatey,
Homebrew, Scoop and winget manifests).

`jx script hello.java` requires JBang and hands it the script, its arguments
and `--offline`; when the script sits above a jx manifest, jx also exports that
project's pinned JDK. `jx import jot.toml` reads the Java-only, single-module
subset of jot's manifest — including version catalogs and annotation
processors — and prints every approximation; workspaces, path dependencies,
Kotlin and non-Temurin pins are refused, and jot's default flat layout is
imported with a note about moving sources. The five-minute path
never requires any of those external tools.

Not yet usable: manifest-less delegation (a `project.toml` supplies the JDK
pin), delegated `run` / `package` (use the build tool, or `--engine jx`), and
published OS package-manager entries. The release workflow generates and
attaches Chocolatey, Homebrew, Scoop and winget manifests, but submitting them
to those repositories needs the maintainer's review and credentials. Annotation
processors run from `[processors]`; entries need explicit versions and cannot
be platform BOMs.

## Design preview website

The public design preview lives in `public/`: hand-written HTML, CSS and ES
modules, no framework and no build step. It describes the plan; it never claims
a release. Live preview: [https://kael-tombo.github.io/Jx/](https://kael-tombo.github.io/Jx/).
Open `public/index.html` directly or serve the folder:

```sh
npm install                # dev tooling only; the site has no runtime deps
npm start                  # http://127.0.0.1:4173
npm run check              # claims, TODOs, contrast, shared markup, links, HTML
npm test                   # Playwright and axe, both themes
npm run spellcheck
npm run lighthouse         # mobile, port 4174
```

`npm run check:production` fails until every `{{TODO}}` is resolved. The owner
inputs and launch gates are listed in [docs/LAUNCH.md](docs/LAUNCH.md); design
and truth decisions are in [docs/decisions/0100-site-design-system.md](docs/decisions/0100-site-design-system.md),
[0101](docs/decisions/0101-site-truth-and-status.md) and
[0102](docs/decisions/0102-site-fonts-and-assets.md). Current measured results
are in [docs/QUALITY.md](docs/QUALITY.md).

## Repository

* [docs/README.md](docs/README.md) — documentation index, layout, roadmap
* [docs/decisions/](docs/decisions/) — architectural decision records
* [docs/exit-codes.md](docs/exit-codes.md) — the exit-code contract
* [docs/decisions/0007-maven-export.md](docs/decisions/0007-maven-export.md) — what `jx export maven` reproduces
* [docs/decisions/0009-runtime-images.md](docs/decisions/0009-runtime-images.md) — jdeps/jlink image decisions
* [docs/decisions/0010-maven-import.md](docs/decisions/0010-maven-import.md) — what `jx import` converts from Maven
* [docs/decisions/0011-gradle-interop.md](docs/decisions/0011-gradle-interop.md) — the Gradle import subset and export shape
* [docs/decisions/0012-delegated-builds.md](docs/decisions/0012-delegated-builds.md) — delegated Maven/Gradle builds and their command boundary
* [docs/decisions/0013-compose-the-ecosystem.md](docs/decisions/0013-compose-the-ecosystem.md) — what jx owns, what stays with the ecosystem, and the adapter rules
* [SECURITY.md](SECURITY.md) — reporting, supported versions and the security model
* [docs/security-checklist.md](docs/security-checklist.md) — the V1 review record and open owner actions
* [docs/RELEASING.md](docs/RELEASING.md) — version, tag and release verification
* [docs/quickstart.md](docs/quickstart.md) — install to first run, with verification
* [docs/KNOWN_LIMITATIONS.md](docs/KNOWN_LIMITATIONS.md) — what does not work yet
* [docs/LAUNCH.md](docs/LAUNCH.md) — website launch gates and owner inputs
* [docs/QUALITY.md](docs/QUALITY.md) — measured site quality record

## Licence

Dual-licensed under [Apache-2.0](LICENSE-APACHE) OR
[MIT](LICENSE-MIT).
