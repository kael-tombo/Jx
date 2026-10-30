# ADR 0003 — Repository client and POM model (M2)

Date: 2026-10-30
Status: Accepted

## Context

M2 builds the pieces M3's resolver consumes: a Maven repository client and a
POM model. The brief says "verify, do not assume"; everything below marked
**verified** was checked against Apache sources on 2026-10-30, not recalled
from memory.

## Verified Maven behaviour we build against

### Repository URL layout

- A `groupId` becomes a path by replacing each `.` with `/`; artifact id,
  version, classifier and extension are used verbatim:
  `{group}/{artifact}/{version}/{artifact}[-{classifier}].{ext}`.
  Source: <https://maven.apache.org/repositories/layout.html>.

### Checksums

- Repositories publish `.sha1`/`.md5` sidecar files next to each artifact.
- Central additionally sends `x-checksum-sha1` / `x-checksum-md5` **response
  headers**; the resolver prefers provided header checksums, then sidecars.
  Source: <https://maven.apache.org/resolver/expected-checksums.html>.
- The policy on mismatch is configurable in Maven; the **default changed from
  `warn` (Maven 3) to `fail` (Maven 4)**.

**Decision:** jx always uses `fail`. A checksum mismatch or an unchanged-but-
corrupt file is a hard error, never a warning. (Also brief requirement 8.)

### Metadata

- Artifact-level `maven-metadata.xml` lives at `{group}/{artifact}/` and lists
  `versioning/{latest,release,versions,lastUpdated}`.
- Snapshot resolution metadata lives at `{group}/{artifact}/{baseVersion}/`
  with `versioning/snapshot{timestamp,buildNumber}` plus optional
  `snapshotVersions` entries; the timestamped snapshot filename replaces
  `SNAPSHOT` with `YYYYMMDD.HHMMSS-N`.
  Source: <https://maven.apache.org/ref/current/maven-repository-metadata/repository-metadata.html>.

### Mirrors (settings.xml semantics, as implemented by the resolver)

- `mirrorOf` grammar: repository **ids** comma-separated, `*`,
  `external:*` (non-`file:`, host not `localhost`/`127.0.0.1`),
  `external:http:*`, and `!id` exclusions. **It is not a glob** — `repo*`
  does not mean "ids starting with repo".
- Selection: an exact whole-value match of one id wins first; otherwise the
  first declared mirror whose pattern matches wins. No trimming of whitespace
  in the list. The mirror's own **id** selects its credentials.
  Source: `DefaultMirrorSelector`, maven-resolver 1.9.x.

**Decision:** jx mirrors use this exact grammar and order, driven from
`$JX_HOME/config.toml` instead of `settings.xml`.

### Proxies

- Resolver picks the first proxy whose protocol matches (HTTPS falls back to
  the configured HTTP proxy when no HTTPS entry exists). `nonProxyHosts`
  splits on `|`, treats `.` literally and `*` as a wildcard, and is matched
  case-insensitively against the repository **host**. Proxy selection happens
  against the **mirror-selected** repository URL, not the original one.

**Decision:** jx honours `HTTPS_PROXY`/`HTTP_PROXY`/`NO_PROXY` (standard env
grammar, comma-separated) and a `proxy` table in config.toml; `no_proxy`
patterns are suffix matched with leading `*` supported. Priority: config >
environment. Nothing here emulates `active=true` (a Maven settings concept).

### Authentication

- Maven selects `<server>` credentials **by repository/mirror id**; its
  password "encryption" is obfuscation-grade (single-hash key derivation, no
  authenticated encryption), which Maven's own docs admit.

**Decision:** jx never stores credentials in files that the user typed them
into casually. Credentials are only `auth = "env:VAR"` references; the value
is `user:password` for Basic auth (a leading `bearer:` prefix sends it as a
bearer token instead). Document this in `docs/repository.md`.

### Version ordering

- Ported **from source**, not from docs: `ComparableVersion.java` and
  `ComparableVersionTest.java` (maven-artifact, master). Notable verified
  points: the underscore is *not* a separator in the current implementation;
  a hyphen followed by a digit in a nondigit run folds into a combination
  item (`X-1` ≈ `X1`); `ga`/`final`/`release` alias the empty qualifier but
  remain structurally distinct (ordering equality, not object equality);
  numeric items rank above string items (`1.1 > 1-sp`, `1.1 > 1-1`); unknown
  qualifiers sort after `sp`, lexicographically; a `-` followed by a nonzero
  number nests a sub-list; **trailing separate zero items are removed but the
  nested structure matters** (`MNG-7644`: `1.0.0.X1 < 1.0.0-X2`).
- The port is verified against the ordering and equality tables from
  Maven's own test suite (`tests/` in this crate carries them verbatim).
- Deviation: jx bounds version strings at 256 **characters** (Maven: 256
  UTF-16 units) and returns a polished error instead of an exception.

## POM model scope for M2

- Parsed with `quick-xml`: parent, coordinates, packaging, properties,
  dependencies, `dependencyManagement` (incl. BOM imports, recursively, with
  cycle detection), `repositories`/`pluginRepositories` and
  `distributionManagement` URLs.
- Inheritance: parent chain is fetched through the same client and merged
  parent-before-child; child values win; `${...}` property interpolation
  supports declared properties plus `project.*`, `project.parent.*` and
  `pom.*` spellings. No active profiles in M2 (recorded in limitations).

## Consequences

- M3's resolver consumes: `Coordinate`, `Version`, the effective-POM model,
  and the client's content-cache-backed fetch.
- The jx content cache remains sha256-keyed; sha1 is a transport-only
  verification step against the repository's own sidecar/header, exactly as
  Maven treats it. Integrity guarantee comes from the fixture: lockfile pins
  a sha256 per artifact in M3.
- Network behaviour (retries, `--offline` refusals) mirrors M1's toolchain
  patterns so error text stays consistent.
