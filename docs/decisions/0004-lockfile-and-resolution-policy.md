# 0004. Lockfile format, version selection and conflict policy (M3)

Date: 2026-10-31
Status: Accepted

## Context and problem

M3 turns a `project.toml` into a resolved dependency graph. The brief demands
reproducibility ("one manifest, one lockfile"), offline reuse
(`jx fetch`), and honest conflict reporting, while staying Maven-compatible
enough that `export maven` round-trips. M2's pieces are in place: a
checksum-verified repository client with a content-addressed cache,
an effective-POM loader (parent chains, interpolation, BOM imports) and
Maven-exact version ordering.

What had to be decided: the lockfile's shape and its relationship to the
cache, the rule for expanding a *wanted* version (a manifest pin like `33`)
to a concrete one, and the rule for two transitive paths disagreeing about
one artifact's version.

## Decision

### Version selection (wanted → concrete)

* A wanted version `N`, `N.M` or `N.M.P` selects the **highest stable
  release with that version prefix** among the offered versions, ordered by
  `ComparableVersion` (ADR 0003's port). "Stable" excludes pre-releases:
  versions whose ordering recognises an `alpha`/`beta`/`milestone`/`rc`/
  `snapshot` qualifier are not candidates for a prefix pick.
* A leading `=` pins exactly (`=2.0.16`); it matches only that version string.
* SNAPSHOTs are never resolved from a prefix (rejected at manifest parse
  time, ADR-consistent with `docs/manifest.md`).
* Platforms: a `platform` entry supplies versions to same-table dependencies
  that name none, exactly like imported BOM management (the loader already
  produces a consolidated `dependencyManagement`).

### Conflict policy

With a manifest-limited reach: neither Maven's *nearest-wins* nor Cargo's
*diamond-transparent* default. jx resolves **like Maven (nearest wins) with
the aligned subtree trimmed**, but any version-fork is reported:

* The nearest declaration wins, and the loser's subtree is resolved against
  the winning version (Maven's rule — the "nearest wins, losers' subtrees
  align to the winner" result `mvn dependency:tree` shows; VERIFY against a
  real tree when the resolver lands).
* A conflict is never silent: the resolution report prints both
  coordinates and winners (`jx tree` shows it persistently; the resolver's
  report shows it on every resolve).
* `[overrides]` is the explicit hammer: an override changes the version of
  the artifact *everywhere* in the graph and is always visible in the
  report.

### The lockfile (`jx.lock`)

```toml
version = 1

[jdk]
provider = "temurin"
feature = 25
release = "25.0.1+9"                          # exact build
sha256 = "<archive checksum>"

[[artifact]]
group = "com.google.guava"
artifact = "guava"
version = "33.4.8-jre"                        # concrete, wanted pin expanded
sha256 = "<the fetched jar's sha256>"

[[artifact]]
group = "org.slf4j"
artifact = "slf4j-api"
version = "2.0.16"
classifier = "test"                           # optional
sha256 = "…"
```

* **Artifacts, not packages**: each listed entry is one fetchable file (the
  classifier/extension is part of the identity). POMs are not listed — they
  are inputs to resolution, not build inputs; their versions are implied.
* **`sha256` is the cache key** (ADR 0003: content-addressed). Offline mode
  requires a matching, present cache entry; a missing/mismatching entry is a
  hard error pointing at `jx fetch`. Locks then *pin* Maven-checksum
  verification to a griefing-resistant value from the first fetch, not to
  what later fetches claim (the sha1-or-sha256 race the resolver client
  absorbs).
* **Ordering is canonical**: artifacts sorted by `(group, artifact,
  classifier, version)`. Writes are whole-file and atomic (temp + rename),
  so concurrent jx processes cannot interleave.
* **The `[jdk]` entry** records the exact toolchain actually used to build;
  `jx run` refuses a mismatched *provider* silently? No — a mismatch is a
  hard error; a different *build* of the same feature release is allowed
  with a warning line in verbose mode only.
* Unknown keys are diagnosed with the did-you-mean machinery (Kind::Lock);
  `version` above 1 is refused with "the lockfile was written by a newer
  jx".

## Options considered

* Cargo-style "minimal version selection" — friendlier upgrades but
  diverges from Maven; users come from Maven and `export maven` would
  produce different graphs. Not chosen.
* Salted/HMAC'd lockfile entries against substituted sidecars — overkill
  for V1: the first fetch verifies a repository checksum, and the stored
  sha256 pins the bytes; sidecar substitution later fails cache verification
  too. Deferred to the M8 threat-model pass.
* Locking POMs too — no build input; resolution inputs change only when
  dependencies change, in which case re-resolve is the correct response.

## Consequences

* `jx add`/`remove`/`fetch` operate on the manifest + lockfile pair: add
  writes the wanted pin, resolve computes artifacts, fetch materializes the
  cache, build/run/test use the lock's sha256s for their classpath without
  the network (the M3 "lock fast path").
* Version changes upstream (a repo publishes a higher `33.x` release) do
  NOT disturb an existing lockfile: concrete versions update only on an
  explicit re-resolve (`jx lock`, or the lock that `jx add`/`jx remove`
  performs) or a changed manifest entry. V1 has no separate `jx update`
  verb; `jx lock` is the explicit refresh.
* `[overrides]` are honoured at every occurrence and every replacement is
  reported (implemented in `jx-resolve`).
* **Implementation note (M3):** the resolver keeps parent edges alongside
  the solved nodes for `jx tree`/`jx why`; the lockfile itself remains the
  artifact list below and does not store those edges. The conflict policy
  is exercised against the fake repository in tests; a live
  `mvn dependency:tree` differential is still an M8 verification item.
