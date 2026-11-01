# Releasing jx

The release path is one version tag. The workflow
[`.github/workflows/release.yml`](../.github/workflows/release.yml) verifies the
tag against `Cargo.toml` and the committed lockfile, runs fmt, clippy and the
workspace tests, builds five native targets, packages the archives, writes
`SHA256SUMS`, attests provenance and creates the GitHub release.

## Preconditions

1. The tree is clean and `Cargo.lock` is committed.
2. CI is green on `main`: fmt, clippy, workspace tests and `cargo-deny`.
3. [security-checklist.md](security-checklist.md) has no unresolved finding
   that blocks release. Open owner actions may remain, but the approver must
   accept them explicitly.
4. [CHANGELOG.md](../CHANGELOG.md) describes the release.
5. The workspace version in `Cargo.toml` is the version being released. The
   workflow refuses a tag that does not match it.
6. `v*` tag protection and private vulnerability reporting are configured (see
   the checklist).

## Cut the release

```sh
# from a clean main
cargo test --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings

git tag -a v1.0.0 -m "jx 1.0.0"
git push origin main
git push origin v1.0.0
```

Pushing the tag starts the workflow. The `preflight` job fails before building
anything if the tag and `Cargo.toml` disagree or `Cargo.lock` is stale, and the
`verify` job must pass before any platform build starts.

## What the workflow publishes

| Target | Runner | Archive |
| --- | --- | --- |
| `x86_64-unknown-linux-gnu` | `ubuntu-22.04` | `jx-<tag>-x86_64-unknown-linux-gnu.tar.gz` |
| `aarch64-unknown-linux-gnu` | `ubuntu-24.04-arm` | `jx-<tag>-aarch64-unknown-linux-gnu.tar.gz` |
| `x86_64-apple-darwin` | `macos-15-intel` | `jx-<tag>-x86_64-apple-darwin.tar.gz` |
| `aarch64-apple-darwin` | `macos-15` | `jx-<tag>-aarch64-apple-darwin.tar.gz` |
| `x86_64-pc-windows-msvc` | `windows-latest` | `jx-<tag>-x86_64-pc-windows-msvc.zip` |

Each archive contains the binary, `README.md`, `SECURITY.md` and both licence
texts under one top-level directory. `SHA256SUMS` covers every archive, and
`actions/attest-build-provenance` records the workflow provenance. The release
is marked pre-release when the tag contains a hyphen (`v1.0.0-rc.1`).

The Linux x64 binary builds on Ubuntu 22.04 (glibc 2.35). The arm64 binary
builds on Ubuntu 24.04 (glibc 2.39); a static/musl build is not part of V1.
macOS and Windows binaries are not code-signed or notarized; Gatekeeper and
SmartScreen warnings are expected until certificates are supplied.

## Verify the release

Do this on a machine that did not build it, before announcing:

```sh
gh release download v1.0.0 --repo kael-tombo/Jx
sha256sum --check --ignore-missing SHA256SUMS
gh attestation verify jx-v1.0.0-x86_64-unknown-linux-gnu.tar.gz --repo kael-tombo/Jx

tar -xzf jx-v1.0.0-x86_64-unknown-linux-gnu.tar.gz
./jx-v1.0.0-x86_64-unknown-linux-gnu/jx --version
```

Then run [quickstart.md](quickstart.md) through `jx run` and `jx test` with the
downloaded binary. On Windows, run `install.ps1` against the published tag and
confirm the checksum check catches a corrupted download.

## If a build fails

* The workflow is safe to rerun: artifacts are uploaded by name, and an
  existing release is updated with `--clobber`.
* Fix the cause on `main`, then use the Actions "Re-run failed jobs" control.
  Do not move a published tag; publish a new patch tag instead.
* If a release is published with a bad binary, mark it as a pre-release and
  publish a fixed patch with an advisory. Do not silently replace an archive
  under the same tag.

## Owner actions still open

* A maintainer minisign/GPG key over `SHA256SUMS`; no key material belongs in
  this repository or workspace.
* macOS Developer ID signing/notarization and a Windows code-signing
  certificate.
* Reproducible-build verification (`SOURCE_DATE_EPOCH` and an independent
  rebuild comparison).
