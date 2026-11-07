# 0017. Generate OS package-manager manifests at release time

Date: 2026-11-01
Status: Accepted

## Context and problem

The release workflow publishes tar.gz and zip archives plus `SHA256SUMS`.
Users of Chocolatey, Homebrew, Scoop and winget expect their own package
manager to install and upgrade a tool. ADR 0013 says those tools own OS-level
installation; jx should not become a package manager, but shipping manifests
*for* them is not the same as wrapping them.

Hand-written manifests would drift from the release: a version, URL or hash
would be updated in one place and forgotten in another. The release already
computes the canonical hashes.

## Decision

`scripts/render-packages.mjs` reads the published `SHA256SUMS` and generates:

* a Chocolatey package source tree (`jx.nuspec`, install/uninstall scripts);
* a Homebrew formula for a tap;
* a Scoop bucket manifest;
* three winget manifests under `KaelTombo.Jx/`.

The renderer validates the tag, repository name and every expected archive
before writing anything. URLs always point at the release tag, and every
manifest carries the archive's SHA-256. The release workflow runs the renderer
after the GitHub release exists, attaches the rendered files, and packs the
Chocolatey source into a `.nupkg` on a Windows runner. It does not push to
Chocolatey, Homebrew, Scoop or winget; those submissions need the maintainer's
account, review and, for Chocolatey, an API key. No credentials belong in the
repository.

The package manifests install the native binary only. They do not install a
JDK, a JRE, Maven or Gradle; `jx` still installs its own Eclipse Temurin JDK
and resolves from Maven Central at use time. Chocolatey verifies the zip
checksum before extracting it.

## Options considered

* **Hand-maintain four sets of manifests** — simple to start, but version,
  URL and hash drift is guaranteed. Rejected.
* **Publish automatically from the release workflow** — requires external
  accounts and review; a release workflow cannot submit to homebrew-core or
  winget-pkgs without credentials and human review. Rejected.
* **Install the JDK through Chocolatey/Homebrew/Scoop** — would make jx a
  package manager and give up the checksum-verified, reproducible JDK store.
  Rejected by ADR 0013.
* **Bundle the binary inside the Chocolatey package** — distribution rights
  are simplest from the official release; downloading from GitHub keeps one
  artifact and one checksum. The install script downloads from the same
  release as every other package manager.

## Consequences

* A release produces one canonical set of manifests whose hashes match the
  archives; local rendering and the release workflow use the same code.
* The Chocolatey package is ready for `choco pack`; the `.nupkg` is attached
  to the release for testing. It is not pushed to the community feed.
* The Homebrew formula targets a project tap first. A homebrew-core submission
  is a separate review with its own naming and notability rules.
* Winget uses the package identifier `KaelTombo.Jx`. The owner must confirm it
  before submitting; identifiers are immutable once accepted.
* The renderer has fixture tests in CI, plus `npm run validate:packages`,
  which uses installed real tools and reports absent ones as skipped. A
  Linux site job runs the renderer tests and the validation command (so Ruby
  checks the Homebrew formula syntax), and a Windows job runs both again with
  the runner's Chocolatey so `choco pack` is exercised on every push. The
  rendered winget manifests were also validated locally with `winget validate`
  on 2026-11-01; the three files
  carry their 1.9.0 schema headers and the validator reports no warnings. The
  Chocolatey nuspec is well-formed XML and both install scripts parse under
  PowerShell, but `choco pack`, Homebrew's audit and a Scoop install are not
  available in the development environment. The release job itself is only
  exercised by a version tag, so those remain release-time checks.
