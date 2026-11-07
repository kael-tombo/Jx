# Quickstart

This is the path a new user takes in V1: install one binary, create a project,
and run it without installing a JDK first. Every command below is part of the
shipped command surface; the limits are in
[KNOWN_LIMITATIONS.md](KNOWN_LIMITATIONS.md).

## 1. Install

Download the archive for your platform from
<https://github.com/kael-tombo/Jx/releases>, or use the installer:

```sh
# macOS / Linux — inspect before running
curl -fsSLO https://raw.githubusercontent.com/kael-tombo/Jx/main/install.sh
less install.sh
sh install.sh
```

```powershell
# Windows x64 — inspect before running
Invoke-WebRequest https://raw.githubusercontent.com/kael-tombo/Jx/main/install.ps1 -OutFile install.ps1
notepad install.ps1
powershell -ExecutionPolicy Bypass -File .\install.ps1
```

The installer verifies the archive against `SHA256SUMS` before moving the
binary into place. It does not use `sudo` and does not edit your shell profile;
it prints the `PATH` line to add.

If you prefer the archive directly:

```sh
sha256sum --check --ignore-missing SHA256SUMS
gh attestation verify jx-v1.0.0-x86_64-unknown-linux-gnu.tar.gz \
  --repo kael-tombo/Jx
tar -xzf jx-v1.0.0-x86_64-unknown-linux-gnu.tar.gz
install -m 755 jx-v1.0.0-x86_64-unknown-linux-gnu/jx ~/.local/bin/jx
```

## 2. Check the environment

```sh
jx --version        # jx 1.0.0
jx doctor           # home, cache, network, system Java, disk, optional tools
```

`jx doctor` is diagnostic only. A missing system Java is fine: jx manages its
own Temurin JDK under `~/.jx/jdks` and never touches the system installation.
It also reports the optional tools jx delegates to: JBang (for `jx script`,
honouring `JX_JBANG`) and the platform package managers the release renders
manifests for. Those findings are informational; jx works without any of them.

## 3. First project

```sh
jx new hello
cd hello
cat project.toml
```

The manifest pins Temurin 25 (the current LTS verified in ADR 0002) and names
`app.App` as the main class. No JDK is needed to scaffold.

```sh
jx run
```

The first run downloads and verifies the JDK, caches it, compiles with `javac`,
and prints:

```text
Hello, jx!
```

The second run skips the download and the compile when nothing changed:

```sh
jx run
jx run --offline    # after the first run, cached work needs no network
```

## 4. Add a dependency

```sh
jx add com.google.guava:guava:33
jx tree
jx why com.google.guava:guava
jx lock             # re-resolve and record exact versions plus sha256
jx fetch            # download and re-verify exactly what jx.lock pins
jx run
```

`jx add` writes the manifest as text (comments stay) and updates `jx.lock`.
The lock records a SHA-256 for every artifact; builds and `jx fetch` re-check
it. A versionless `jx add` selects the latest stable release, never a
pre-release.

## 5. Tests

```sh
jx new --lib calc
cd calc
jx test
```

The library scaffold includes JUnit 5 and one test that passes. `jx test`
compiles main and test sources against separate classpaths and runs the JUnit
Platform console launcher.

## 6. Package and export

```sh
cd ../hello
jx package          # target/dist: thin jar + lib/
java -jar target/dist/hello-0.1.0.jar

jx package --fat    # one merged jar: target/dist/hello-0.1.0-all.jar
jx image            # target/dist/runtime: jdeps-trimmed jlink runtime
jx export maven --group-id com.example
```

`jx export maven` writes a POM using the locked versions; `jx export gradle`
writes Kotlin DSL. Both are starting points, not full reproductions of jx's
build (ADRs 0007 and 0011).

## 7. Coming from Maven or Gradle

```sh
jx import                              # ./pom.xml or build.gradle(.kts)
jx import app/pom.xml --dry-run        # print the manifest and notes
```

Keep the native build authoritative and let jx own the JDK by adding this to
`project.toml`:

```toml
[build]
engine = "maven"    # or "gradle"
```

Then `jx build`, `jx test` and `jx clean` run the project's wrapper with the
pinned JDK. Pass the tool's own flags after `--`, for example
`jx build -- --parallel` or `jx test -- --tests app.LibTest`; jx's
`--filter` and `--reports-dir` belong to its own test runner and stay
refused here. Dependency commands, `run`, `package`, `image` and exports are
refused in that mode; use the build tool or `--engine jx` for one command
(ADR 0012).

## Troubleshooting

* **No network / proxy:** set the proxy in `~/.jx/config.toml` or the standard
  `HTTPS_PROXY` / `NO_PROXY` variables, then run `jx doctor`.
* **Checksum failure:** jx never accepts a changed checksum silently. Re-run
  the download; if it repeats, treat it as a supply-chain event and report it
  through [SECURITY.md](../SECURITY.md).
* **Wrong JDK:** `jx jdk list` shows the jx-managed store; `jx jdk use 21`
  changes the manifest pin. `jx jdk remove 25.0.5+8` deletes only jx's copy.
* **Windows PATH:** the installer prints the exact user-PATH expression; open
  a new terminal after setting it.
* **A command is refused under a delegated build:** that is intentional. Use
  the Maven/Gradle command, or pass `--engine jx` for jx's own build.

For everything jx deliberately does not do, read
[KNOWN_LIMITATIONS.md](KNOWN_LIMITATIONS.md) before filing an issue.
