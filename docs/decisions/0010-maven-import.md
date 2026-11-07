# 0010. Maven import into project.toml

Date: 2026-11-01
Status: Accepted

## Context and problem

M7 starts with the opposite direction of `jx export maven`: a team has an
existing Maven build and wants a jx manifest without hand-copying coordinates.
The two models do not line up. Maven has parents, property interpolation,
`dependencyManagement`, imported BOMs, profiles, scopes (compile, provided,
runtime, test, system, import), exclusions, optional dependencies, artifact
types and a plugin lifecycle. jx has one `[project]` table, a JDK feature pin,
flat `[dependencies]` and `[dev-dependencies]` tables, exact/prefix versions,
platform BOM entries and overrides.

Import must not invent a project that quietly builds differently. It also
must not require Maven to be installed: jx already has an effective-POM loader
that understands parent chains, inheritance order, interpolation and BOM
imports.

## Decision

`jx import [pom]` reads an effective POM and writes a new `project.toml`. The
default source is `./pom.xml`; a directory source means its `pom.xml`. The
default destination is `./project.toml` (or the global `--manifest-path`).
`--output` chooses another path, `--dry-run` prints the manifest and notes
without writing, and `--force` is required to replace an existing manifest.

**Reading the POM**

* The root POM is read from disk through `PomLoader::load_file`, not through a
  repository.
* A parent POM found at the child's `<relativePath>` (default `../pom.xml`) is
  read locally. Missing local parents, and all further ancestors, are fetched
  through the checksum-verified repository client.
* Imported BOMs are expanded by the same effective-POM loader used for
  resolution. No Maven process is started, so `mvn` does not need to exist.
* Profiles are ignored, exactly as they are during resolution (ADR 0003).

**Generating the manifest**

* `[project] name` is the effective `artifactId`; `version` is the effective
  `version`. Parent-inherited values therefore arrive correct.
* `jdk` comes from `maven.compiler.release`, then `maven.compiler.target`, then
  `java.version`, then `maven.compiler.source`. Values such as `1.8` and
  `21.0.2` are reduced to the feature release. With no compiler setting, the
  jx default is used and the fallback is a printed note.
* Compile, runtime and provided dependencies go to `[dependencies]`; test
  dependencies go to `[dev-dependencies]`. Runtime and provided dependencies
  are over-inclusive compared with Maven — jx has neither scope — so the note
  says they will also be packaged. System dependencies (local `systemPath`)
  are skipped with a note. Non-jar artifact types are skipped with a note.
  `optional` is imported as a normal dependency (with a note) because jx has
  no optionality.
* Every imported version is an **exact** pin (`=version`), using the version
  the effective POM resolved. Prefix versions would let a later resolve choose
  a different patch.
* Test dependencies are imported, but `jx test` needs the JUnit Platform
  console standalone, which Maven POMs normally do not declare. The note names
  the artifact to add; import does not silently add a second test launcher.
* A version still absent after `dependencyManagement`, an unresolved
  `${property}`, a version range or a SNAPSHOT is a hard error: no manifest is
  written. Silently guessing would make import look more faithful than it is.
* Two direct dependencies with the same `group:artifact` (for example two
  classifiers) are refused; jx's manifest cannot name both yet.
* BOM imports are **expanded at import time**, not written as platform
  entries. Direct versions are therefore exact even for projects whose POMs
  depend on a large Spring Boot-style BOM chain. The note reports how many
  BOMs were expanded.
* POM `<repositories>` are not migrated: jx only trusts `config.toml`
  (ADR 0003). The note lists their ids.
* Exclusions, optionality, Maven plugin configuration, resource directories
  and `module-info` handling are not migrated. Notes say which of these
  applied.
* `maven-compiler-plugin`'s `<annotationProcessorPaths>` is the one plugin
  setting that is read: each `<path>` with an explicit version becomes a
  `[processors]` entry, inherited through the parent chain until a POM
  declares its own list. jx runs every processor on the path, so
  `<annotationProcessors>` narrowing, processor `<arguments>` and
  `<executions>` are reported rather than reproduced.
* The generated text is parsed with the same `jx-manifest` validator used by
  every other command before it is written. A validation failure is a jx bug
  and is reported as such; nothing is written.

**Packaging**

Only `<packaging>jar</packaging>` is accepted. A `pom` aggregator should
point at its application module; `war`, `ear` and other packagings are
refused with an explanation. Import does not copy sources or change the
project layout.

## Options considered

* **Invoke `mvn help:effective-pom`** — exact for Maven's own model, but
  requires Maven, starts a JVM, and makes the result depend on the local
  Maven/plugin cache. jx already has a tested effective-POM loader; using it
  keeps import deterministic and works offline for single-module projects.
* **Read only the raw POM** — easy, but useless for the common case: Spring
  Boot and most modern POMs omit direct dependency versions and rely on
  parents/BOMs. It would either fail constantly or copy incomplete data.
* **Write imported BOMs as platform entries** — preserves management for
  future `jx add`, but the effective model may contain dozens of BOMs from
  ancestors, and jx's resolver would have to re-expand them. Exact direct
  pins keep the first import simple and reproduce the versions Maven already
  chose. Platform migration can be added later without breaking the format.
* **Import Maven's test-scope dependencies into the main table** — wrong for
  jx's separate test classpath. The two tables are kept distinct.
* **Overwrite project.toml silently** — destroys work in a directory that may
  already be a jx project. `--force` is explicit.

## Consequences

* A normal single-module Maven application imports into a manifest that jx's
  own parser accepts, with the same direct release versions and JDK feature.
* Import is a conversion, not a round trip. Users must review the notes and
  run `jx lock`/`jx build`; plugin behaviour, exclusions and non-jar types are
  not reproduced.
* Multi-module repositories should import each application module separately
  until jx has a workspace model.
* Gradle import was added in ADR 0011, and delegated Maven/Gradle lifecycle
  commands in ADR 0012; M7 is complete.
