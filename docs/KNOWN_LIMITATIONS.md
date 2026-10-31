# Known limitations

jx must never claim support for something untested. This file records honest
gaps, milestone by milestone.

## Current (M4 done; M5 next)

Live commands: `jx --version`, `jx doctor`, `jx jdk install/list/use/remove`,
`jx new`, `jx init`, `jx add`, `jx remove`, `jx lock`, `jx fetch`, `jx tree`,
`jx why`, `jx build`, `jx test`, `jx run`, `jx clean`. M2 delivered the
repository client (fetch with always-on checksum verification, mirrors,
proxies, auth references, content cache), POM/`maven-metadata.xml` models,
Maven version ordering and the effective-POM loader. M3 added the resolver,
the lockfile, the six dependency commands, and the lock fast path that puts
cached jars on javac's and java's classpath. M4 added fingerprint-skipped
javac builds, separate main/test dependency classpaths, test compilation and
JUnit 5 execution.

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
  manifest.
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
  (`scope=import`) consolidation live in `jx_maven::PomLoader`. POM
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
- **`jx build` does not produce a jar**; packaging is M5.

### Toolchain (from M1)

- **The JDK provider is Eclipse Temurin over the Adoptium API only.** Other
  providers are behind `JdkProvider` but not implemented.
- **JDK archives download with only the API-provided sha256 as
  verification**; GPG signing is part of the M8 security checklist.
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
- **The cache layer serves three consumers**: JDK archives (M1), Maven
  artifacts via the repository client (M2/M3) and the build classpath
  (M3).

### CI / release

- **CI runs fmt/clippy/tests on Linux, macOS and Windows, plus cargo-deny**,
  but milestone end-to-end scenarios are not exercised in CI yet (M8).
- **No release binaries, no install script** (M8).
