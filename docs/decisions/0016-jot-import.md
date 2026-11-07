# 0016. Import `jot.toml` projects

Date: 2026-11-01
Status: Accepted

## Context and problem

jot is a Java build tool with a similar shape: one binary, one TOML manifest,
a lockfile and a JDK manager. A developer evaluating jx may already have a
`jot.toml`; asking them to retype dependencies or run a second tool would make
the exit back to jx worse than the exit to Maven or Gradle.

Importing a competitor's manifest is not the same as supporting their build.
jx can read declarations, but it cannot evaluate jot plugins, workspaces,
Kotlin, path dependencies or its publish pipeline. Dropping those silently
would produce a project that builds differently.

## Decision

`jx import` accepts a `jot.toml` path, and detects one in a directory after
`pom.xml` and `build.gradle(.kts)`:

| jot.toml | jx result |
| --- | --- |
| `[project] name`, `version` | `[project] name`, `version`; missing version uses `0.1.0` with a note |
| `[project] main-class` | `[run] main` |
| `[project] group` | not imported; the note points at `jx export maven --group-id` for publishing |
| `[toolchains] java = "21"` | `jdk = "21"` |
| `temurin-21` | `jdk = "21"`; another vendor is refused |
| `[dependencies]` strings | exact-pinned `[dependencies]` |
| `[test-dependencies]` strings | exact-pinned `[dev-dependencies]` |
| `[processors]` entries | exact-pinned `[processors]`; each needs an explicit version |
| `{ catalog = "name" }` | resolved through the nearest `libs.versions.toml` (project directory, or the workspace root above a member) |
| `{ path = "../module" }` | refused: jx has no workspace path dependencies |
| `[workspace]` | refused: import one member directory at a time |
| a Kotlin toolchain | refused: jx compiles Java only |
| `layout` omitted or `"flat"` (jot's default) | imported with a note that `src/`, `test/` and `res/` must move to the Maven layout |
| `[publish]` | not imported; `jx export maven` is the publishing exit |

Every approximation is printed before the manifest is written. The generated
text is then parsed by the same validator `jx build` uses; a converter bug
cannot leave a broken `project.toml` behind. An existing manifest is still
protected by `--force`, and `--dry-run` prints without writing.

## Options considered

* **Ignore jot** — leaves users to retype coordinates and gives jx no answer
  when jot is named beside Maven, Gradle and JBang. Rejected.
* **Read jot plugins or evaluate its build** — would turn import into a second
  jot runtime and violate the rule against script evaluators. Rejected.
* **Support jot workspaces** — jx has no workspace model; pretending one
  `project.toml` covers several members would be wrong. Refused with a clear
  next step.
* **Accept any JDK vendor in the pin** — jx installs Eclipse Temurin only;
  silently changing the vendor would make the import look reproducible when
  it is not. Refused.
* **Export a `jot.toml` instead** — a one-way migration path into jx is useful
  first; export to jot can be added later if the format stabilises.

## Consequences

* A Java-only, single-module jot project with a Temurin or plain feature pin
  imports in one command, including version catalogs and annotation
  processors.
* Workspaces, Kotlin, path dependencies and publish metadata stay with jot.
  The limitations file says so.
* Catalog resolution is read-only and local; no network request is needed for
  `jx import`.
* The converter is covered by unit tests for the supported subset, the flat
  default and the refusals, and by CLI tests that build a `jot.toml` plus
  catalog — including a workspace member reading the root catalog — and parse
  the generated manifest with jx's own model.
