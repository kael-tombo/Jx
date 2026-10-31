# 0007. Maven POM export

Date: 2026-11-01
Status: Accepted

## Context and problem

The project promises an exit from jx: a team can start with `jx` and move to
Maven without recreating its dependency graph by hand. `jx export maven` was
planned for M5. Maven and jx do not share a project model: jx has one name and
version, a single JDK pin, prefix-based wanted versions and an override table;
Maven needs groupId/artifactId/version, concrete dependency versions and
plugin configuration.

A generated POM that silently differs from the jx build is worse than no
export. The command therefore has to state what it can reproduce and refuse to
overwrite an existing build.

## Decision

`jx export maven` writes `pom.xml` in the project directory after compiling
once (so a discovered main class is included).

* **Coordinates:** `groupId` defaults to `local.jx` and can be changed with
  `--group-id`; `artifactId` is `[project] name` and `version` is
  `[project] version`. Nothing is inferred from an internet domain.
* **Dependencies:** direct `[dependencies]` entries become compile-scope
  dependencies with the **concrete version recorded in `jx.lock`**, not the
  manifest prefix. `[dev-dependencies]` become `test` scope. Classifiers are
  preserved. A declaration with no locked artifact is an error, not a guessed
  version.
* **Overrides:** every `[overrides]` artifact that occurs in the locked graph
  becomes a `dependencyManagement` entry at its locked version. This also
  influences Maven's transitive resolution.
* **Platform BOMs are not exported.** Their effect is already present in the
  concrete versions of direct dependencies and the managed overrides. Writing
  a BOM with a prefix version (for example `2.18`) would pin the wrong thing.
* **Java release:** `<maven.compiler.release>` is the manifest's JDK feature;
  the compiler plugin is pinned to 3.13.0.
* **Main class and runnable layout:** `<mainClass>` is written into the jar
  plugin when jx can determine one. The same plugin adds a relative
  `Class-Path` with `lib/` entries, and `maven-dependency-plugin` copies
  runtime dependencies into `target/lib` during `package`, so
  `java -jar target/<name>-<version>.jar` works like jx's thin dist. A
  library gets neither plugin.
* **Tests:** if any test-scope dependency exists, surefire 3.5.2 is configured
  with `useModulePath=false` so the JUnit Platform runs on the classpath.
* **No clobbering:** an existing `pom.xml` is an error unless `--force` is
  passed.

The command is deliberately an **export**, not a round-trip conversion. The
POM does not include annotation-processor paths, jx's own fingerprint caching,
native trimming, or a copy of the `.jx` manifest state. Maven's nearest-wins
resolution can still produce a different transitive graph from jx's
lockfile-driven one; jx's direct versions and overrides are the parts it can
make exact.

## Options considered

* **Emit a POM from the lockfile's full transitive closure** — every artifact
  would become a direct dependency, freezing Maven's graph but destroying
  normal Maven dependency management and creating a very large POM. Not
  chosen.
* **Keep manifest prefixes as Maven version ranges** — ranges have different
  semantics and would resolve differently over time. Concrete locked versions
  keep the export reproducible.
* **Ask for a groupId interactively** — blocking on input breaks scripts and
  CI. A documented `--group-id` flag with a neutral default is cheaper.
* **Overwrite `pom.xml` unconditionally** — a user may already have a Maven
  build in the same directory. Refusing by default protects work; `--force`
  is the explicit opt-in.

## Consequences

* `jx export maven` gives a working starting point for a Maven build with
  the same direct versions, scopes and JDK release.
* Users must review the generated POM before relying on it: plugin versions
  are pinned to known-good releases, BOM behaviour is inlined, and the
  groupId is a neutral default unless supplied.
* Gradle export remains M7.
