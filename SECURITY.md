# Security policy

`jx` downloads JDKs and Maven artifacts, verifies checksums, extracts archives,
and runs `javac`/`java` plus opt-in Maven or Gradle builds. Security reports
about any of that are welcome.

## Supported versions

| Version | Supported |
| --- | --- |
| 1.x | Yes, with fixes on the latest patch release |
| Earlier than 1.0 | No |

Security fixes land on `main` and are published as a new patch release. Older
minor versions are not maintained separately during V1.

## Reporting a vulnerability

**Do not open a public issue with vulnerability details.**

Use GitHub's private vulnerability reporting for this repository:

<https://github.com/kael-tombo/Jx/security/advisories/new>

If that form is unavailable, open a minimal public issue asking the maintainer
to enable a private channel, and do not include the vulnerability itself.

Please include:

* the `jx --version` output and the operating system,
* the smallest reproduction you can share (a manifest, command and observed
  result),
* the impact you believe it has, and
* whether a fix or mitigation is already known.

We aim to:

* acknowledge a report within 3 business days,
* confirm or dispute the impact within 10 business days,
* ship a fix for a confirmed critical issue within 30 days, and for other
  confirmed issues within 90 days.

These are best-effort targets for a small project, not a contractual service
level. There is no bug bounty. We will credit reporters who ask to be credited
once a fix is published.

## Release integrity

Release archives are accompanied by:

* `SHA256SUMS` with one SHA-256 digest per archive, and
* a GitHub build provenance attestation created by the release workflow.

Verify a download before running it:

```sh
sha256sum --check --ignore-missing SHA256SUMS
gh attestation verify jx-v1.0.0-x86_64-unknown-linux-gnu.tar.gz \
  --repo kael-tombo/Jx
```

Release artifacts are **not GPG- or minisign-signed yet**. That is recorded as
an owner action in [docs/security-checklist.md](docs/security-checklist.md);
until it is closed, checksums and attestations are the available release
integrity signals. The install scripts verify the checksum before installing.

## Scope

In scope:

* the `jx` binary and the crates in this workspace,
* the JDK download, checksum and extraction path,
* the Maven repository client, resolver and lockfile,
* `install.sh`, `install.ps1` and the release workflow,
* a dependency vulnerability that is reachable through `jx`.

Out of scope:

* the contents of Maven Central or any third-party repository,
* Eclipse Temurin JDK binaries and the Adoptium API (report those upstream),
* defects in Maven, Gradle, `javac`, `java`, `jdeps` or `jlink` themselves,
* arbitrary code in a project built with `jx`,
* the design-preview website's content.

## Security model

* **TLS is verified.** The repository and toolchain clients use `ureq` with
  rustls and the bundled WebPKI roots. An HTTPS request redirected to plain
  HTTP is refused.
* **Checksums are mandatory.** Maven artifact bytes are checked against a
  repository checksum on every fetch, and the lockfile pins a SHA-256 digest
  that `jx fetch` and builds re-check. A JDK archive without a published
  SHA-256 is refused.
* **`config.toml` is the only repository trust anchor.** POM-declared
  repositories are parsed and diagnosed but never fetched from in V1.
* **Credentials are references, never values.** `auth = "env:VAR"` or
  `auth = "bearer:env:VAR"` reads the secret from the environment at request
  time. Inline credentials in `config.toml` and userinfo in repository URLs
  are rejected. Error output never prints the secret; an invalid proxy URL is
  redacted.
* **No shell interpolation.** Delegated Maven/Gradle commands and JDK tools
  are launched as argument vectors, not through a shell string.
* **Archives are bounded.** Extraction rejects absolute paths, `..`
  traversal, backslashes, drive letters, control characters and special
  files. It accepts at most 200,000 entries and 4 GiB expanded; a JDK download
  is bounded by the provider's stated size or a 512 MiB fallback. The archive
  is hashed while it streams, so verification does not require loading it into
  memory.
* **Delegation is opt-in.** `[build] engine = "maven"` or `"gradle"` runs the
  project's own wrapper; the default engine is jx's own build path.

## Accepted risks and limitations

These are deliberate V1 trade-offs, recorded in
[docs/security-checklist.md](docs/security-checklist.md):

* A repository credential may be sent over plain `http://` when a user
  configures such a repository. `jx` prints a warning but does not refuse it,
  because internal HTTP mirrors are still common.
* The lockfile is not signed. An attacker who can write to a project can change
  both the manifest and the lockfile.
* A JDK archive is trusted when its published SHA-256 matches Adoptium's
  listing. A compromise of that listing is outside `jx`'s control.
* Release archives are not GPG/minisign-signed yet.
* No fuzzing campaign or reproducible-build verification has been completed.
