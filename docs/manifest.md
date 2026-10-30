# The jx manifest: project.toml — target specification

This is the target format for V1. Status per table: **parsed & validated now**,
**accepted (opaque)** = parsed but its values carry no semantics until the
named milestone, and **planned** = not yet read at all.

```toml
[project]                                  # parsed & validated (M1)
name = "myapp"                             # required; folder legal name
version = "0.1.0"                          # required
jdk = "21"                                 # required; feature release pin;
                                           # exact build recorded in jx.lock (M3)

[dependencies]                             # accepted (opaque), semantics M3
guava = "com.google.guava:guava:33"        # alias = groupId:artifactId[:version]
slf4j = "org.slf4j:slf4j-api:=2.0.16"      # leading `=` pins exactly
jackson-bom = { platform = "com.fasterxml.jackson:jackson-bom:2.18.0" }
jackson-databind = "com.fasterxml.jackson.core:jackson-databind" # version from platform

[dev-dependencies]                         # accepted (opaque), semantics M3/M4
junit = "org.junit.jupiter:junit-jupiter:5"

[processors]                               # planned (M4): annotation processors
mapstruct = "org.mapstruct:mapstruct-processor:1"

[overrides]                                # planned (M3): explicit, always visible
"org.slf4j:slf4j-api" = "2.0.16"

[run]                                      # parsed & validated (M1)
main = "com.acme.App"                      # optional; else discovered post-build

[repositories]                             # planned (M2)
internal = { url = "https://repo.example.com/maven", auth = "env:JX_INTERNAL_TOKEN" }

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
* Unknown keys are errors with a did-you-mean suggestion (invalid on
  `[project]` and `[run]` today; dependency tables get the same in M3).
* Unknown tables are errors with a did-you-mean suggestion over the table
  names above.

## Rules (implemented in M1, exactly as above)

* `[project]`: `name` (non-empty string), `version` (string), `jdk`
  (`"25"`-style feature release, digits only).
* `[run]`: only key `main` (non-empty fully-qualified class name).
* Unknown `[project]`/`[run]` keys and unknown tables are rejected with a
  did-you-mean suggestion; known-but-future tables parse opaquely.
* `jx jdk use <feature>` rewrites only the `jdk` line in place — byte-identical
  elsewhere, comments preserved — and is the model for comment-preserving
  dependency edits in M3.
