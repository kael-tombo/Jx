# Changelog

All notable changes to jx are recorded here. The project follows
[Semantic Versioning](https://semver.org/). V1's exit-code contract is in
[docs/exit-codes.md](docs/exit-codes.md).

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
  `--debug`/`--port`.
* `jx package` (thin jar and `--fat`), `jx image` (jdeps-trimmed jlink runtime
  with a full-runtime fallback), `jx clean`.
* `jx import` for Maven POMs and the declarative Gradle Kotlin/Groovy subset,
  `jx export maven/gradle`, and opt-in delegated `build`/`test`/`clean` via
  `[build] engine` or `--engine`.
* `jx doctor`: home, cache, network, system Java and disk diagnostics.
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

* Annotation processors are deferred.
* There is no `jx update`; `jx lock` is the refresh command.
* Delegated `run`/`package` are not supported; use the build tool or
  `--engine jx`.
* JBang and OS-installer adapters are post-V1 (ADR 0013).
* Release artifacts are not maintainer-signed, and macOS/Windows binaries are
  not code-signed or notarized yet.
* Fuzzing and reproducible-build verification are not complete.

The detailed, milestone-by-milestone list is in
[docs/KNOWN_LIMITATIONS.md](docs/KNOWN_LIMITATIONS.md).
