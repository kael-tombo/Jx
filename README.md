# jx — a Cargo/uv-style front door for Java

> **Status: under construction (M0).** This README describes the intent of
> the project, not yet its shipped abilities; see
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
jx --version     # show the version
jx doctor        # diagnose jx's home, cache, network, system Java, disk
```

## Repository

* [docs/README.md](docs/README.md) — documentation index, layout, roadmap
* [docs/decisions/](docs/decisions/) — architectural decision records
* [docs/exit-codes.md](docs/exit-codes.md) — the exit-code contract
* [docs/KNOWN_LIMITATIONS.md](docs/KNOWN_LIMITATIONS.md) — what does not work yet

## Licence

Dual-licensed under [Apache-2.0](LICENSE-APACHE) OR
[MIT](LICENSE-MIT).
