# jx documentation index

`jx` is a single-binary command line tool that makes developing Java feel like
`cargo` (Rust) or `uv` (Python): one tool, one manifest (`project.toml`), one
lockfile (`jx.lock`). It manages JDKs, resolves dependencies from Maven
Central, and builds, tests, runs and packages a project — while sitting on
top of the tools that already own each job and always offering the exit back
to Maven or Gradle.

## Where things live

| Path | Purpose |
| --- | --- |
| [decisions/](decisions/) | ADRs (MADR). Template: [decisions/TEMPLATE.md](decisions/TEMPLATE.md) |
| [exit-codes.md](exit-codes.md) | The stable exit-code contract |
| [KNOWN_LIMITATIONS.md](KNOWN_LIMITATIONS.md) | What jx honestly does not do yet |
| [security-checklist.md](security-checklist.md) | V1 security review, fixes and open owner actions |
| [RELEASING.md](RELEASING.md) | Version, tag, release verification and rollback |
| [quickstart.md](quickstart.md) | Install, verify and run the first project |
| [QUALITY.md](QUALITY.md) | Measured website quality record |
| [LAUNCH.md](LAUNCH.md) | Website launch gates and owner inputs |
| [AGENT_STATE.md](AGENT_STATE.md) | Current site work, decisions and next step |

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
jdeps/jlink image and its full-runtime fallback;
[decisions/0010-maven-import.md](decisions/0010-maven-import.md) — what
`jx import` converts from an effective POM and what it deliberately does
not;
[decisions/0011-gradle-interop.md](decisions/0011-gradle-interop.md) — the
Gradle declaration subset, the Spring Boot BOM rule and the generated
Kotlin-DSL build;
[decisions/0012-delegated-builds.md](decisions/0012-delegated-builds.md) —
`[build] engine` and `--engine`: what a delegated Maven/Gradle build owns,
which commands are refused, and why;
[decisions/0013-compose-the-ecosystem.md](decisions/0013-compose-the-ecosystem.md)
— the ownership table and adapter rules that keep jx a workflow layer rather
than a package manager, runtime or build tool. [quickstart.md](quickstart.md)
is the executable user path; [security-checklist.md](security-checklist.md)
records the extraction, checksum, TLS and injection review;
[RELEASING.md](RELEASING.md) is the version/tag/verification procedure.

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
    jx-package/     # jar, dist layout, jdeps/jlink and Maven export
    jx-run/         # Runner trait, plain-main runner
    jx-interop/     # Maven and Gradle import and delegated builds
    jx-testsupport/ # fake repository server, fixtures, differential harness
  docs/  public/  scripts/  tests/
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
| M7 | Maven and Gradle interop: import, compatibility, delegate, exports | **done** — `jx import` (Maven + Gradle), `jx export maven/gradle` and delegated `build`/`test`/`clean` via `[build] engine` or `--engine`; ADRs 0010–0012 |
| M8 | Release hardening: docs, install script, security checklist | **done** — release workflow, checksummed installers, security review; owner sign-off and signing remain |

After V1, optional adapters may hand scripts to JBang and OS installs to
Chocolatey, Homebrew or Scoop; both are deliberately absent today (ADR 0013).

Items requiring verification against live documentation before implementation
are collected in the brief (section 15); the relevant ADR records the verified
answers when each lands.
