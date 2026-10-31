# The jx manifest: project.toml — target specification

This is the target format for V1. Status per table: **parsed & validated now**,
**accepted (opaque)** = parsed but its values carry no semantics until the
named milestone, and **planned** = not yet read at all.

```toml
[project]                                  # parsed & validated (M1)
name = "myapp"                             # required; folder legal name
version = "0.1.0"                          # required
jdk = "21"                                 # required; feature release pin;
                                           # exact-build locking is not written yet

[dependencies]                             # parsed, validated and resolved
guava = "com.google.guava:guava:33"        # alias = groupId:artifactId[:version]
slf4j = "org.slf4j:slf4j-api:=2.0.16"      # leading `=` pins exactly
jackson-bom = { platform = "com.fasterxml.jackson:jackson-bom:2.18.0" }
jackson-databind = "com.fasterxml.jackson.core:jackson-databind" # version from platform

[dev-dependencies]                         # parsed, validated and resolved
junit = "org.junit.jupiter:junit-jupiter:5"

[processors]                               # planned (M4): annotation processors
mapstruct = "org.mapstruct:mapstruct-processor:1"

[overrides]                                # parsed & validated; applied by the resolver
"org.slf4j:slf4j-api" = "2.0.16"           # keys are group:artifact; values pin exactly

[run]                                      # parsed & validated (M1)
main = "com.acme.App"                      # optional; else discovered post-build

[repositories]                             # not a manifest table: repositories live in
                                           # config.toml (ADR 0003); known here only so
                                           # the error can say so

[build]                                    # planned (M7): engine delegation
engine = "maven"
```

## Rules (target)

* Keys in dependency tables are local aliases; coordinates are
  `group:artifact[:version][:classifier]`.
* A version `N`, `N.M` or `N.M.P` is a compatibility prefix: jx selects the
  highest stable release with that prefix. A leading `=` is exact.
  Pre-releases (alpha, beta, rc, milestone) are excluded unless pinned
  exactly. SNAPSHOT versions are rejected with a clear message.
* A `platform` entry is a BOM supplying versions to entries without one.
* `[overrides]` entries override one artifact everywhere in the graph;
  values are exact pins (a leading `=` is accepted but has the same
  meaning); a table may not override the same artifact twice. Overrides
  never act silently — every applied replacement is part of jx's visible
  resolution report.
* Unknown keys are errors with a did-you-mean suggestion (on
  `[project]`, `[run]` and every dependency table entry).
* Unknown tables are errors with a did-you-mean suggestion over the table
  names above.

## Rules (implemented so far)

* `[project]`: `name` (non-empty string), `version` (string), `jdk`
  (`"25"`-style feature release, digits only).
* `[run]`: only key `main` (non-empty fully-qualified class name).
* Unknown `[project]`/`[run]` keys and unknown tables are rejected with a
  did-you-mean suggestion.
* `[dependencies]`/`[dev-dependencies]` entries are parsed and validated at
  manifest parse time: coordinate shape (`group:artifact[:version]
  [:classifier]`), version charset, SNAPSHOT rejection, `=` exact pins and
  `{ platform = … }` BOM entries; a table may not name the same
  `group:artifact` twice. The resolver then expands each wanted version and
  `jx build`/`run` put the locked jars on javac's and java's classpath.
* `[overrides]` entries are parsed and validated the same way (quoted
  `group:artifact` keys, version values, no duplicates, no SNAPSHOTs;
  a leading `=` is allowed but optional — override values pin either way).
* `jx jdk use <feature>` rewrites only the `jdk` line in place —
  byte-identical elsewhere, comments preserved.
* `jx add` and `jx remove` apply the same line-oriented discipline to the
  dependency tables: only the added/removed entry line changes, every other
  byte (comments, blank lines, CRLF endings) survives. A failed resolve
  restores the manifest unchanged.
