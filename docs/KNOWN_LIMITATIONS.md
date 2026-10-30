# Known limitations

jx must never claim support for something untested. This file records honest
gaps, milestone by milestone.

## Current (M1: toolchain + hello)

Live now: `jx --version`, `jx doctor`, `jx jdk install/list/use/remove`,
`jx new`, `jx init`, `jx build`, `jx run`, `jx clean`. The M0 caveats below
that still apply are marked.

### Toolchain

- **The JDK provider is Eclipse Temurin over the Adoptium API only.** Other
  providers are behind `JdkProvider` but not implemented.
- **`jx new`/`jx jdk install` default to Temurin 25**, the current LTS
  verified on 2026-10-30 (ADR 0002). The pin is a checked-in constant, not a
  network query, so scaffolding works offline.
- **JDK archives download with only the API-provided sha256 as verification**
  (streamed and re-verified on cache reuse). GPG signature verification is
  part of the M8 security checklist.
- **`--offline` refuses a missing pinned JDK** with a clear error, and
  succeeds when that feature release is already installed. There is no
  `jx fetch` yet (M3), so "offline after one download" means "offline after
  the JDK for your pin is installed".
- **Root-level symlink validation in tar.gz** covers link *targets* escaping
  the tree; hard-link checks are thinner and belong to the M8 hardening.

### Scaffold, build, run

- **`jx new`/`init` write a fixed package `app`** regardless of project name;
  renaming a package means editing sources.
- **`--lib` scaffolds a JUnit 5 test that `jx test` cannot run yet** (M4 adds
  the JUnit Platform launcher).
- **Builds are not incremental and have no dependency classpath** (M3
  resolver, then M4 fingerprints). `jx build` recompiles everything each
  time; diagnostics stream straight from javac.
- **`jx run` discovery matches only static `public void main(String[])`**;
  instance-main / compact-source styles (a newer JDK feature) are not
  discovered — pin `[run].main` for those.
- **`jx run --debug` starts a suspend-on-launch JDWP server** with a fixed
  default port 5005 (`--port`); no client-side tooling is provided.
- **Long-classpath handling** via javac argument files is implemented, but
  the `target/dist` layout, launchers, fat jar, `export` and runtime
  trimming are all M5–M6 work not yet present.
- **`jx test` is still a stub** that names M4. Everything else listed as a
  planned milestone reports itself as unimplemented with its milestone.

### Doctor, config, cache (still from M0)

- **Doctor's network probe is DNS+TCP only.** TLS URLs (real transfers,
  proxy environments, custom CAs, `NO_PROXY`) are M2 repository-client work;
  corporate proxies with custom CAs may currently fail the *probe*, not jx
  itself.
- **config.toml parsing is permissive** about stray unknown keys; strict
  diagnostics arrive with M2/M3.
- **The cache layer is exercised for JDK archives**; no Maven artifact
  download path exists yet (M2).

### CI / release

- **CI runs fmt/clippy/tests on Linux, macOS and Windows, plus cargo-deny**,
  but the milestone's end-to-end scenarios are not yet exercised in CI (M8).
- **No release binaries, no install script** (M8). Locally, this Windows dev
  box builds jx with the windows-gnu toolchain (rustup's self-contained
  linker + a portable MinGW in `~/mingw64`); CI stays MSVC.
