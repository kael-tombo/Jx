# V1 security checklist

Review date: 2026-11-01
Scope: the `jx` 1.0.0 source tree, its installers and its release workflow
Reviewer: implementation review by the coding agent; **owner sign-off still
required** (see the end of this document)

This is a working security record, not a certification. It lists what was
checked, what was fixed, what is verified by tests, and what remains an owner
action or a deliberate V1 limitation. The public policy is
[SECURITY.md](../SECURITY.md).

## Threat model

| Asset | Threat | Control |
| --- | --- | --- |
| JDK archive and installed JDK | Malicious or corrupted download, downgrade, decompression bomb | HTTPS only; publish-time SHA-256; HTTPS→HTTP redirect refused; size budget; 4 GiB/200k-entry extraction budget; path and special-file rejection |
| Dependency jars and POMs | Tampered artifact, malicious metadata, credential leak | Mandatory repository checksum; SHA-256-pinned lockfile re-check; POM repositories not fetched; auth only through env references; no inline URL credentials |
| Repository credentials | Disclosure in files, logs or redirects | Env reference only; redacted proxy errors; ureq `RedirectAuthHeaders::Never`; plaintext warning |
| Project manifest and lockfile | Command injection, silent resolution change, graph confusion | Manifest grammar has no command field; engines are a closed enum; nearest-wins conflicts are reported; `--locked` fails on drift |
| Build and run | Running attacker-controlled shell text | `std::process::Command` with argument vectors; JDK tool paths come from the installed store |
| Release artifacts | Substituted binary, unclear provenance | Per-platform builds from a version tag; `SHA256SUMS`; GitHub build provenance attestation; installers verify checksums |

## Findings and fixes

| # | Finding | Severity | Resolution |
| --- | --- | --- | --- |
| 1 | The JDK downloader read the whole archive into memory for verification and had no download cap. | Medium (resource exhaustion) | **Fixed.** SHA-256 is computed while streaming; the download is capped at the provider's stated size or 512 MiB. A one-byte-over read is refused. `sha256_file` also verifies cached archives by streaming. |
| 2 | Extraction had no expansion budget; a decompression bomb could fill the disk. | Medium | **Fixed.** Tarballs and zips are capped at 4 GiB expanded and 200,000 entries. Tarball entries other than regular files, directories and links are refused. |
| 3 | An HTTPS repository or JDK URL could redirect to plain HTTP. | Low (downgrade) | **Fixed.** Both HTTP clients reject a final URL that is `http://` after an `https://` request. ureq's default `RedirectAuthHeaders::Never` was verified, so `Authorization` is not forwarded across redirects. |
| 4 | A repository URL containing `user:password@` would place a secret in `config.toml` and in diagnostics. | Medium | **Fixed.** `config.toml` rejects userinfo in repository and mirror URLs. Proxy userinfo remains supported but is redacted before an error prints the URL. |
| 5 | Credentials configured for a plain-HTTP repository were sent without any warning. | Low | **Mitigated.** A one-time warning is printed for non-loopback plaintext auth. Refusing outright would break internal mirrors; the residual risk is accepted and documented. |
| 6 | Archive entry paths could escape the staging directory. | High if present | **Verified safe.** `entry_path_is_safe` rejects absolute paths, `..`, backslashes, drive letters, `//` and control characters for both file names and tar link targets; tests cover the rejects. |
| 7 | Archive entries could create devices or FIFOs. | Medium | **Fixed.** Tar entry types are restricted to file, directory, symlink and hard link. Zip symlink payloads are written as regular bytes, so they cannot redirect a later write. |
| 8 | XML parser could resolve external entities or a DTD. | Medium if present | **Verified safe.** `quick-xml` is used with `Reader`; `DOCTYPE` is ignored and only predefined/numeric entities are unescaped. No external entity resolver is configured. |
| 9 | Auth headers could leak on redirect. | Medium | **Verified safe.** ureq 2.12's default `RedirectAuthHeaders::Never` strips `authorization` and `cookie`; jx does not override it. A downgrade is also refused (finding 3). |
| 10 | Command injection through delegated builds or tool arguments. | High if present | **Verified safe.** Commands are constructed as argument vectors (`Command::arg`), never through a shell. The engine is a closed enum. |
| 11 | Repository and artifact fetches could be redirected to an unrelated host with credentials. | Medium | **Verified safe.** ureq does not preserve authorization across hosts; TLS is verified with rustls/WebPKI; final URLs are not allowed to downgrade. |
| 12 | Lockfile or cache tampering could silently change artifacts. | Medium | **Verified.** Locked artifacts are re-hashed before use; a mismatch is a hard error. The lockfile itself is not signed (accepted risk below). |
| 13 | Release downloads could be substituted. | High if unmitigated | **Mitigated for V1.** Archives carry `SHA256SUMS` and a GitHub build provenance attestation; installers verify the archive against the checksum file before installing. No maintainer signature yet (open action 3). |

## Verified controls

These are exercised by the workspace suite, not only read:

* `jx-toolchain`: unsafe path rejection (`extract::tests::safe_paths`), staging
  naming, and the version-ordering/store tests.
* `jx-maven`: checksum enforcement (headers, `.sha256`, `.sha1`, missing
  checksum), auth (Basic and Bearer), mirror routing, offline cache serving,
  snapshot metadata, and per-coordinate parallel failures in
  `tests/repository_fetch.rs`.
* `jx-common`: inline auth rejection, URL-credential rejection and auth
  reference parsing.
* `jx-maven::client`: redirect downgrade helper assumptions, plaintext-auth
  loopback exemption, and proxy credential redaction.
* `jx-cli`: end-to-end dependency, build, import, export and delegation tests
  against fake repositories and fake build tools.
* CI runs `cargo-deny` for licence and advisory checks on every push and pull
  request.
* The release workflow builds with `--locked`, so the dependency set is the
  one committed in `Cargo.lock`.

## Open owner actions

1. **Enable GitHub private vulnerability reporting** in repository settings so
   the link in [SECURITY.md](../SECURITY.md) works. Until then, reporters are
   asked to request a private channel without disclosing details.
2. **Protect `v*` tags** (ruleset or tag protection) so a release tag cannot be
   moved or created by an untrusted actor. The release workflow trusts the tag.
3. **Decide on artifact signing.** Recommended: a maintainer minisign or GPG
   key over `SHA256SUMS`, published in the repository. Not configured because
   no key material belongs in this workspace.
4. **Turn on secret scanning and Dependabot alerts** for the public repository.
5. **Review the release workflow** once on a non-production tag (for example
   `v1.0.0-rc.1`) and confirm the five platform jobs, attestation and release
   assets all succeed before tagging `v1.0.0`.
6. **Set a review cadence** for `cargo-deny` advisories and for the Adoptium
   provider (ADR 0002 asks for re-verification before release work).

## Deliberate V1 limitations

* No fuzzing of tar/zip extraction, POM parsing or the Gradle declaration
  parser. They are covered by targeted tests only.
* No reproducible-build verification. The release workflow does not set
  `SOURCE_DATE_EPOCH` or compare independent builds.
* No runtime sandbox for `javac`, `java`, Maven or Gradle. Building a project
  executes its code by design; jx does not claim a sandbox.
* The installer checks a checksum fetched from the same origin as the archive.
  A compromise of the GitHub release is not detected by this alone; the
  attestation is the second signal.
* The macOS and Windows binaries are built but not code-signed or notarized.
  Gatekeeper and SmartScreen warnings are expected until an owner supplies
  certificates.

## Sign-off

| Role | Name | Date | State |
| --- | --- | --- | --- |
| Implementation review | Coding agent | 2026-11-01 | Complete for the source tree in this commit |
| Owner / release approver | _Owner action_ | _Owner action_ | **Open** |
| Independent security review | _Owner action_ | _Owner action_ | **Open** — recommended before a 1.x release that changes the trust model |
