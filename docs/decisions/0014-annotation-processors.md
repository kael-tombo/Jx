# 0014. Annotation processors from `[processors]`

Date: 2026-11-01
Status: Accepted

## Context and problem

M4 shipped javac builds, test compilation and fingerprint skipping, but left
annotation processors open: `[processors]` was a recognized table with no
semantics, and `javac` ran without `-processorpath`. Java projects commonly
need processors (MapStruct, Lombok, Dagger, Micronaut, …), and javac's rules
for them differ from ordinary dependencies:

* a processor is a **jar javac executes**, not a class the application links;
* javac discovers processors on `-processorpath` (or an explicit
  `-processor` list), not by scanning the application classpath;
* since JDK 23, annotation processing is disabled unless the invocation
  explicitly asks for it — passing `-processorpath` is such a request;
* processors may have transitive dependencies of their own, and those must
  not leak onto the application classpath.

A processor also has to be reproducible: the same lockfile must pin its bytes
and version, like every other build input.

## Decision

### Manifest

`[processors]` uses the dependency grammar (alias → coordinate, optional
classifier) but is stricter:

```toml
[processors]
mapstruct = "org.mapstruct:mapstruct-processor:1.6.3"
lombok = "org.projectlombok:lombok:=1.18.36"
```

Every entry must name a version. `{ platform = … }` entries are rejected:
javac needs jars on its processor path, and a BOM in another table is not a
source of processor versions. `jx add --processor` appends an entry (picking
the latest stable release when the user gives none, exactly like a normal
add), and `jx remove --processor` removes one. Both flags are mutually
exclusive with `--dev` and `--platform`.

### Resolution

Processors resolve as a **separate graph** with the same nearest-wins,
conflict-reporting and `[overrides]` rules as the app graph (ADR 0004).
Resolution is rooted only in `[processors]`; processor roots are never
dev-scoped. The two graphs never share classpath state.

### Lockfile schema 3

`jx.lock` gains two per-artifact booleans:

| field | default | serialized | meaning |
| --- | --- | --- | --- |
| `app` | `true` | only when `false` | belongs to the application graph |
| `processor` | `false` | only when `true` | belongs on javac's processor path |

The existing `dev` flag is unchanged. Classpaths derive as:

* main compile/run: `app && !dev`
* test compile/run: `app`
* processor path: `processor`
* packaging and export: `app`

Version 1 and 2 locks are still read (all entries are app artifacts, none are
processors) and are rewritten as version 3 by the next lock.

A GA that appears in both graphs is merged when the versions agree: one lock
entry with `app = true` and `processor = true`, so a jar that is both a
library and a processor stays on both paths. When the versions disagree,
`jx lock` **fails**: one lockfile records one version per group:artifact, and
silently picking either would change one of the graphs. The error names both
versions and points at `[overrides]`, which applies to both graphs and is the
supported way to align them. `jx add` still refuses a GA already declared in
another table; overlap is possible through hand edits or a transitive
processor dependency.

### Build

`jx-build` receives the processor path separately from the classpath. A
non-empty path adds exactly one `-processorpath` line to the javac argument
file for **both** main and test compilation; an empty path adds no flag, so
javac's own default (no processing on JDK 23+) applies. The path is part of
the command fingerprint (under a distinct marker from classpath entries), so
adding, removing or changing a processor jar recompiles.

### Commands and reports

* `jx tree` prints the app graph, then an `annotation processors:` section
  with the processor graph.
* `jx why` answers from both graphs. An app-graph match prints the app path;
  processor-only artifacts print a labeled `annotation processors:` path,
  and an artifact that serves both roles prints both paths. An explicit
  version selects only the graph(s) resolving to that version. The tree
  remains the compact report.
* `jx import` migrates the processor declarations it can read statically:
  Gradle `annotationProcessor`/`testAnnotationProcessor` entries with a
  literal version, and Maven's `maven-compiler-plugin`
  `<annotationProcessorPaths>`. Anything else (Kotlin `kapt`/`ksp`, a
  versionless path, the buildscript classpath) is counted in a note that
  points at `[processors]`.
* `jx export maven` and `jx export gradle` do not emit processor
  configuration; the generated build is an app-only starting point.

## Options considered

* **One merged graph** — processor jars would land on the application
  classpath, and javac could discover processors the manifest did not
  declare. Rejected: the two roles have different classpath and packaging
  rules.
* **Treat processors as `[dev-dependencies]`** — simple, but dev dependencies
  are application code for tests, not javac plugins, and javac would scan the
  test classpath for processors. Rejected.
* **Require `-processor` class names** — would need to read
  `META-INF/services/javax.annotation.processing.Processor` from each jar and
  would break processors that register differently. Letting javac discover
  from an explicit processor path is the standard behavior. Rejected.
* **Store two versions of one GA in the lockfile** — the canonical identity
  is group/artifact/classifier, and the resolver's version policy is per GA;
  two versions would make `jx fetch`, export and cache lookups ambiguous.
  Rejected in favor of a hard error plus `[overrides]`.
* **Resolve `{ platform = … }` inside `[processors]`** — would let a BOM
  supply versions, but a processor path cannot use a POM and the extra
  indirection hides which jars javac executes. Deferred; explicit versions
  are the V1 rule.

## Consequences

* A project can use MapStruct/Lombok-style processors with one manifest table
  and one lockfile; processor transitive dependencies stay off the app
  classpath and out of packaged jars.
* Same-version overlap is represented faithfully; version disagreement is
  loud and fixable with an override that is visible in every resolution
  report.
* `jx why` explains both the app graph and the processor graph; the two
  graphs still resolve independently, and a shared artifact shows its two
  roles separately.
* Import is one-way and partial, and export emits no processor configuration,
  so a round trip drops it. Both importers write `[processors]` where a
  declaration is statically readable, and their notes name what was skipped.
* Annotation processing is all-or-nothing per compile: jx does not expose
  `-proc:none`, `-proc:only`, processor options (`-A…`) or a separate
  generated-sources directory yet.
