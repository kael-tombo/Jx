# OS package manifests

These files are generated from a published release's `SHA256SUMS`; do not edit
the copies under `dist/`. The source of truth is
[`scripts/render-packages.mjs`](../scripts/render-packages.mjs).

## Render

After a release exists and its assets are downloadable:

```sh
# dist/ contains the release archives and SHA256SUMS
node scripts/render-packages.mjs v1.0.0 dist
```

The command writes `dist/packaging/`:

| Directory | What it is |
| --- | --- |
| `chocolatey/` | A Chocolatey package source tree (`jx.nuspec` + `tools/`) |
| `homebrew/` | A Homebrew formula for a tap |
| `scoop/` | A Scoop bucket manifest |
| `winget/` | Three winget manifests under `KaelTombo.Jx/` |

Every manifest points at the GitHub release archives and carries the SHA-256
from `SHA256SUMS`. The release workflow runs the same command and attaches the
rendered files to the release, so this directory is for local verification and
for submitting updates.

Run both checks before a release:

```sh
npm run test:packages
npm run validate:packages
```

The second command renders a fixture release and validates it with winget,
PowerShell, Ruby and Chocolatey when they are installed. An unavailable tool is
reported as `SKIP`, never as a pass.

## Chocolatey

```sh
choco pack dist/packaging/chocolatey/jx.nuspec --out dist/packaging/chocolatey
choco install jx --source dist/packaging/chocolatey
```

Publishing to the community feed is a maintainer action:

```sh
choco push dist/packaging/chocolatey/jx.<version>.nupkg \
  --source https://push.chocolatey.org/ --api-key <key>
```

The package downloads the Windows zip from the GitHub release, verifies its
SHA-256, extracts it and creates the `jx` shim. It does not bundle a JRE or a
JDK; `jx` installs its own Temurin JDK on first use.

## Homebrew

The formula installs the prebuilt archive for the current macOS or Linux
architecture. Homebrew 7 only loads formulae from a tap, so install it from
one (for example `kael-tombo/tap`) rather than from a file:

```sh
brew install kael-tombo/tap/jx
brew test kael-tombo/tap/jx
brew audit --new-formula kael-tombo/tap/jx
```

To verify a rendered formula before it reaches a tap, create a throwaway local
tap and copy the file into it:

```sh
brew tap-new local/jx --no-git
cp dist/packaging/homebrew/jx.rb "$(brew --repository local/jx)/Formula/jx.rb"
brew install local/jx/jx
brew test local/jx/jx
```

The formula does not depend on `openjdk`; `jx` manages its own JDK store.

## Scoop

Add `dist/packaging/scoop/jx.json` to a bucket, or install it directly from the
file:

```sh
scoop install ./dist/packaging/scoop/jx.json
```

The manifest extracts the Windows zip and exposes `jx.exe`.

## winget

Validate and submit from a clone of `microsoft/winget-pkgs`:

```sh
winget validate --manifest dist/packaging/winget/KaelTombo.Jx
winget install --manifest dist/packaging/winget/KaelTombo.Jx
```

The manifests use the portable zip installer and alias `jx` to `jx.exe`.

Installing a local manifest needs the administrator-only `LocalManifestFiles`
setting enabled first. It also registers the package under an ARP-style id
(`ARP\User\X64\KaelTombo.Jx__DefaultSource`), which is the id to pass to
`winget uninstall`; a package installed from the published winget source uses
`KaelTombo.Jx` normally. A locally built, unsigned archive can additionally
trip the archive malware scan, which applies only to local manifest installs.

## What is not automated

The generated files are release artifacts, not publications. Submitting to
Chocolatey, homebrew-core, a Scoop bucket or `winget-pkgs` requires the
maintainer's account, review, and in Chocolatey's case an API key. None of
those credentials belong in this repository.
