# 0002. Eclipse Temurin as the M1 JDK provider — verified API facts

Date: 2026-10-30
Status: accepted (facts verified live on this date by direct `curl` against
`api.adoptium.net` and `/q/openapi.json`; the exact-release route was
re-verified on 2026-11-06; re-verify before release work)

## Context and problem

The brief's section 15 requires verifying, not assuming, the Adoptium API
endpoints, response format, checksum availability, and which LTS release to
pin by default. This ADR records the verified answers used by
`crates/jx-toolchain`.

## Verified facts (checked 2026-10-30)

* `GET https://api.adoptium.net/v3/info/available_releases` →
  `{ "available_lts_releases": [8, 11, 17, 21, 25], "most_recent_lts": 25, ... }`.
  **The current LTS is 25**; V1 pins feature release `25` by default.
* The OpenAPI description lives at `https://api.adoptium.net/q/openapi.json`.
  Notable paths:
  * `GET /v3/assets/latest/{feature_version}/{jvm_impl}?release_type=…&os=…&architecture=…&image_type=jdk&vendor=eclipse`
    — note `release_type` is a **query parameter** (calling it as a path
    segment, e.g. `assets/latest/25/ga`, returns 404).
  * `GET /v3/binary/latest/{feature_version}/{release_type}/{os}/{arch}/{image_type}/{jvm_impl}/{heap_size}/{vendor}`
    (e.g. `/v3/binary/latest/25/ga/windows/x64/jdk/hotspot/normal/eclipse`)
    redirects (307) to the release artifact on GitHub.
  * `GET /v3/checksum/version/{release_name}/{os}/{arch}/{image_type}/{jvm_impl}/{heap_size}/{vendor}`
    exists for checksum-only queries.
* `…/assets/latest/25/hotspot?release_type=ga&os=windows&architecture=x64&image_type=jdk&vendor=eclipse`
  returns one JSON asset with the fields jx relies on:
  `release_name` (e.g. `jdk-25.0.5+8`), `version.semver`, and
  `binary.package.{name, link, checksum}` where `checksum` is the **sha256
  hex digest of the package** (verified present for GA packages; a known
  2026-10 mis-match case exists for one early-access build, so jx treats any
  checksum mismatch as a hard error rather than trying to be clever).
* `GET /v3/assets/release_name/{vendor}/{release_name}` (e.g.
  `/v3/assets/release_name/eclipse/jdk-25.0.5%2B8?os=windows&architecture=x64&image_type=jdk&vendor=eclipse`)
  returns **one release object**, not an array, with `release_name` and a
  `binaries` array; each binary carries `os`, `architecture`, `image_type`,
  `jvm_impl` and `package.{link, checksum, size}`. The `+` in the path must be
  `%2B`; `jx fetch` uses this route to restore the exact build named by
  `[jdk]` in `jx.lock`.

## Decision

1. `jx new` (and the default for `jx jdk install`) pins feature release **25**
   as a checked-in constant (no network needed to scaffold). Bumping it is a
   one-line, ADR-noted change.
2. `JdkProvider` resolves via the `assets/latest/{N}/hotspot` endpoint above;
   jx uses `binary.package.checksum` when present and treats a missing
   checksum as an error (M8 hardening may add the checksum-endpoint fallback).
3. Downloads use the `binary/latest/...` redirect URL, are streamed to a temp
   file with an atomic rename, and are sha256-verified against the API-provided
   checksum both at download time (streamed) and again when a cached archive
   is reused.
4. Archive layout: Windows `.zip` (verified above), other platforms
   `.tar.gz`, as offered by Temurin. Extraction rejects path traversal
   (`..`, absolute paths, drive letters, backslashes, unsafe symlink targets)
   before writing, exactly one top directory is expected and is stripped if
   present, and the install layout is
   `<JX_HOME>/jdks/temurin-<release_name-without-jdk->>` with `bin/javac`
   verified after extraction.

## Consequences

Other providers (Foojay, Corretto) can be added behind `JdkProvider` without
touching storage or build code. `most_recent_lts` is available from the same
API if jx later wants `new` to pick the newest LTS dynamically; V1 keeps the
static constant for offline determinism.
