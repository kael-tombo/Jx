# jx documentation index

`jx` is a single-binary command line tool that makes developing Java feel like
`cargo` (Rust) or `uv` (Python): one tool, one manifest (`project.toml`), one
lockfile (`jx.lock`). It manages JDKs, resolves dependencies from Maven
Central, and builds, tests, runs and packages a project — while always
offering the exit back to Maven or Gradle.

## Where things live

| Path | Purpose |
| --- | --- |
| [decisions/](decisions/) | ADRs (MADR). Template: [decisions/TEMPLATE.md](decisions/TEMPLATE.md) |
| [exit-codes.md](exit-codes.md) | The stable exit-code contract |
| [KNOWN_LIMITATIONS.md](KNOWN_LIMITATIONS.md) | What jx honestly does not do yet |

Written so far: [manifest.md](manifest.md) — the project.toml specification;
[repository.md](repository.md) — repositories, mirrors, proxies, auth
references and integrity (repositories live in `config.toml`, per ADR
0003); [decisions/0004-lockfile-and-resolution-policy.md](decisions/0004-lockfile-and-resolution-policy.md)
— the lockfile, version selection and conflict rules M3 implements;
[decisions/0005-packaging-layout.md](decisions/0005-packaging-layout.md) —
the thin jar and `target/dist` layout;
[decisions/0006-fat-jar-merge-rules.md](decisions/0006-fat-jar-merge-rules.md)
— opt-in fat-jar merging;
[decisions/0007-maven-export.md](decisions/0007-maven-export.md) — what the
generated POM does and does not reproduce;
[decisions/0009-runtime-images.md](decisions/0009-runtime-images.md) — the
jdeps/jlink image and its full-runtime fallback. Still planned for later
milestones:

* `docs/quickstart.md` — the executed-in-CI quickstart
* `docs/security-checklist.md` — extraction, checksum, TLS, injection review (M8)

Build/test (M4), packaging (M5) and runtime-image (M6) behaviour is described
in the README quickstart and [KNOWN_LIMITATIONS.md](KNOWN_LIMITATIONS.md); no
separate build/test guide is written yet.

## Workspace layout

```text
jx/
  crates/
    jx-cli/         # clap commands, output, error rendering
    jx-common/      # shared infrastructure: errors, color, JX_HOME, cache (ADR 0001)
    jx-manifest/    # project.toml model, validation, comment-preserving edits
    jx-lock/        # lockfile model, deterministic TOML serialization
    jx-toolchain/   # JDK discovery, download, verify, extract, pin
    jx-maven/       # POM model, version ordering, repository client, metadata
    jx-resolve/     # graph building, conflict policy, tree and why
    jx-build/       # javac, fingerprints, test running
    jx-package/     # jar, dist layout, jdeps/jlink, export maven and gradle
    jx-run/         # Runner trait, plain-main runner
    jx-interop/     # Maven and Gradle import, compatibility mode, delegate engines
    jx-testsupport/ # fake repository server, fixtures, differential harness
  docs/  corpus/  xtask/
```

Crates marked "placeholder" in their doc comment are reserved for upcoming
milestones; nothing in this repository claims to support them yet.

## Roadmap (mirrors the brief's milestone table)

| Milestone | Content | Status |
| --- | --- | --- |
| M0 | Workspace, CI, ADRs, docs skeleton, config/cache, error framework, `jx --version`, `jx doctor` | done |
| M1 | `jx jdk`, `jx new`, minimal `build`/`run`, offline hello | **done** |
| M2 | Repository client, cache, checksums, proxy/mirror, POM model, version ordering | done |
| M3 | Resolver: add/remove/tree/why/lock/fetch, conflict policy, lock fast path | **done** — commands, lockfile, classpath fast path |
| M4 | javac integration, fingerprint skip, resources, JUnit launcher | **done** — annotation processors deferred |
| M5 | Run and package: main discovery, debug, dist, fat jar, `export maven` | **done** — POM export is an approximation (ADR 0007) |
| M6 | Runtime trimming: jdeps + jlink with fallback | **done** — `jx image`, verified and atomic; reflection risk documented |
| M7 | Maven and Gradle interop: import, compatibility, delegate, exports | not started |
| M8 | Release hardening: docs, install script, security checklist | not started |

Items requiring verification against live documentation before implementation
are collected in the brief (section 15); the relevant ADR records the verified
answers when each lands.
