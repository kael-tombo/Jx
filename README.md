# jx — a Cargo/uv-style front door for Java

> **Status: under construction (M6 done: build, run, test, packaging, Maven
> export and runtime images).** This README describes the intent of the
> project, not yet its shipped abilities; see
> [docs/KNOWN_LIMITATIONS.md](docs/KNOWN_LIMITATIONS.md) for the honest state.

`jx` is a single-binary command line tool, written in Rust, that manages JDKs,
resolves dependencies from Maven Central into a reproducible lockfile, and
builds, tests, runs and packages Java projects from one small manifest.
North star: on a machine with nothing installed,
`jx new hello && cd hello && jx run` prints a greeting in under five minutes.

## Principles

1. One tool, one manifest, one lockfile.
2. Smooth beats complete — protect the five-minute happy path first.
3. Every error says what happened, where, and what to try next.
4. Reuse the ecosystem (Maven Central, the JDK's own tools) and always offer
   an exit (`jx export maven`).
5. Fast: no JVM starts unless needed.
6. Never silent about risk: conflicts, changed checksums and unsupported POM
   features are loud.

## Building from source

```sh
cargo build --release        # jx lands in target/release/jx
cargo fmt -- --check && cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
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
jx fetch             # download/verify exactly what jx.lock pins
jx run               # javac and java get the locked jars on their classpath
jx package           # target/dist: runnable jar + lib/ dependency tree
java -jar target/dist/hello-0.1.0.jar
jx package --fat     # one merged jar: target/dist/hello-0.1.0-all.jar
jx image             # target/dist/runtime: trimmed jlink runtime
jx image --full      # all JDK modules when reflection needs them
jx export maven --group-id com.example  # write a POM from the lockfile

# Tests
jx add --dev org.junit.platform:junit-platform-console-standalone
jx test              # compile src/test/java and run JUnit 5

jx jdk list          # jx-managed JDKs live in ~/.jx/jdks, never near system Java
jx doctor            # diagnose jx's home, cache, network, system Java, disk
```

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
reflection loads modules at runtime (ADR 0009).

Not yet usable (they report their milestone): import (M7), annotation
processors (deferred from M4), and a separate `update` command (`jx lock`
re-resolves wanted prefixes today).

## Repository

* [docs/README.md](docs/README.md) — documentation index, layout, roadmap
* [docs/decisions/](docs/decisions/) — architectural decision records
* [docs/exit-codes.md](docs/exit-codes.md) — the exit-code contract
* [docs/decisions/0007-maven-export.md](docs/decisions/0007-maven-export.md) — what `jx export maven` reproduces
* [docs/decisions/0009-runtime-images.md](docs/decisions/0009-runtime-images.md) — jdeps/jlink image decisions
* [docs/KNOWN_LIMITATIONS.md](docs/KNOWN_LIMITATIONS.md) — what does not work yet

## Licence

Dual-licensed under [Apache-2.0](LICENSE-APACHE) OR
[MIT](LICENSE-MIT).
