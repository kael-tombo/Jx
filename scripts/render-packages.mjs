#!/usr/bin/env node
// Render OS package-manager manifests from a published release's SHA256SUMS.
//
//   node scripts/render-packages.mjs v1.0.0 dist
//
// The output is a set of source trees for Chocolatey, Homebrew, Scoop and
// winget. Nothing is published: each ecosystem still needs its own review and
// the maintainer's credentials. Keeping the renderer in one place means the
// URLs and hashes cannot drift between package managers.

import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

export const DEFAULT_REPO = "kael-tombo/Jx";

const ASSETS = [
  {
    target: "x86_64-unknown-linux-gnu",
    suffix: "x86_64-unknown-linux-gnu.tar.gz",
  },
  {
    target: "aarch64-unknown-linux-gnu",
    suffix: "aarch64-unknown-linux-gnu.tar.gz",
  },
  { target: "x86_64-apple-darwin", suffix: "x86_64-apple-darwin.tar.gz" },
  { target: "aarch64-apple-darwin", suffix: "aarch64-apple-darwin.tar.gz" },
  { target: "x86_64-pc-windows-msvc", suffix: "x86_64-pc-windows-msvc.zip" },
];

const TAG_PATTERN = /^v\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?$/;
const REPO_PATTERN = /^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/;

export function parseChecksums(text) {
  const sums = new Map();
  for (const line of text.split(/\r?\n/)) {
    const trimmed = line.trim();
    if (!trimmed) continue;
    const match = trimmed.match(/^([0-9a-fA-F]{64})\s+\*?(.+)$/);
    if (!match) {
      throw new Error(`SHA256SUMS has an unreadable line: ${trimmed}`);
    }
    sums.set(match[2], match[1].toLowerCase());
  }
  return sums;
}

function xml(value) {
  return String(value)
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;");
}

function formulaBlock(os, arm, intel) {
  return `${os} do\n    on_arm do\n      url "${arm.url}"\n      sha256 "${arm.hash}"\n    end\n    on_intel do\n      url "${intel.url}"\n      sha256 "${intel.hash}"\n    end\n  end`;
}

/** Render every package file and return the workspace-relative paths written. */
export function renderPackages({
  tag,
  sums,
  outDir,
  repo = DEFAULT_REPO,
  sourceDir,
}) {
  if (!TAG_PATTERN.test(tag)) {
    throw new Error(`tag must look like v1.0.0, got ${JSON.stringify(tag)}`);
  }
  if (!REPO_PATTERN.test(repo)) {
    throw new Error(`repository must look like owner/name, got ${JSON.stringify(repo)}`);
  }
  if (!(sums instanceof Map)) {
    throw new Error("sums must be a Map of asset name to SHA-256");
  }

  const version = tag.slice(1);
  const base = `https://github.com/${repo}/releases/download/${tag}`;
  const assets = new Map();
  for (const asset of ASSETS) {
    const name = `jx-${tag}-${asset.suffix}`;
    const hash = sums.get(name);
    if (!hash) {
      throw new Error(`SHA256SUMS has no entry for ${name}`);
    }
    assets.set(asset.target, { name, hash, url: `${base}/${name}` });
  }

  const linuxX64 = assets.get("x86_64-unknown-linux-gnu");
  const linuxArm = assets.get("aarch64-unknown-linux-gnu");
  const macX64 = assets.get("x86_64-apple-darwin");
  const macArm = assets.get("aarch64-apple-darwin");
  const windows = assets.get("x86_64-pc-windows-msvc");

  const files = new Map();

  files.set(
    "chocolatey/jx.nuspec",
    `<?xml version="1.0" encoding="utf-8"?>
<package xmlns="http://schemas.microsoft.com/packaging/2015/06/nuspec.xsd">
  <metadata>
    <id>jx</id>
    <version>${xml(version)}</version>
    <title>jx</title>
    <authors>kael-tombo</authors>
    <owners>kael-tombo</owners>
    <projectUrl>https://github.com/${xml(repo)}</projectUrl>
    <projectSourceUrl>https://github.com/${xml(repo)}</projectSourceUrl>
    <packageSourceUrl>https://github.com/${xml(repo)}/tree/main/packaging</packageSourceUrl>
    <docsUrl>https://github.com/${xml(repo)}#readme</docsUrl>
    <licenseUrl>https://github.com/${xml(repo)}/blob/main/LICENSE-APACHE</licenseUrl>
    <requireLicenseAcceptance>false</requireLicenseAcceptance>
    <description>jx is a single-binary workflow tool for Java: one manifest, one lockfile, and one command set for the JDK, Maven Central, javac, JUnit and packaging. It installs its own Eclipse Temurin JDK and does not require Maven, Gradle or a system Java.</description>
    <summary>A Cargo/uv-style front door for Java</summary>
    <tags>java jdk build-tool maven gradle jx</tags>
    <releaseNotes>See https://github.com/${xml(repo)}/blob/main/CHANGELOG.md</releaseNotes>
    <copyright>Copyright (c) the jx contributors</copyright>
  </metadata>
  <files>
    <file src="tools\\**" target="tools" />
  </files>
</package>
`,
  );

  files.set(
    "chocolatey/tools/chocolateyinstall.ps1",
    `$ErrorActionPreference = 'Stop'

$toolsDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$url64 = '${windows.url}'
$checksum64 = '${windows.hash}'

Install-ChocolateyZipPackage \`
  -PackageName 'jx' \`
  -Url64bit $url64 \`
  -UnzipLocation $toolsDir \`
  -Checksum64 $checksum64 \`
  -ChecksumType64 'sha256'

$exe = Get-ChildItem -Path $toolsDir -Filter 'jx.exe' -Recurse | Select-Object -First 1
if (-not $exe) {
    throw 'jx.exe was not found after extracting the release archive'
}
Install-BinFile -Name 'jx' -Path $exe.FullName
`,
  );

  files.set(
    "chocolatey/tools/chocolateyuninstall.ps1",
    `$ErrorActionPreference = 'Stop'
Uninstall-BinFile -Name 'jx'
`,
  );

  files.set(
    "homebrew/jx.rb",
    `class Jx < Formula
  desc "Cargo/uv-style front door for Java"
  homepage "https://github.com/${repo}"
  version "${version}"
  license any_of: ["Apache-2.0", "MIT"]

  ${formulaBlock("on_macos", macArm, macX64)}

  ${formulaBlock("on_linux", linuxArm, linuxX64)}

  def install
    bin.install "jx"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/jx --version")
  end
end
`,
  );

  files.set(
    "scoop/jx.json",
    `${JSON.stringify(
      {
        version,
        description:
          "A Cargo/uv-style front door for Java: one manifest, one lockfile, one command set.",
        homepage: `https://github.com/${repo}`,
        license: "Apache-2.0 OR MIT",
        architecture: {
          "64bit": {
            url: windows.url,
            hash: windows.hash,
          },
        },
        extract_dir: `jx-${tag}-x86_64-pc-windows-msvc`,
        bin: "jx.exe",
        checkver: { github: `https://github.com/${repo}` },
      },
      null,
      2,
    )}\n`,
  );

  files.set(
    "winget/KaelTombo.Jx/KaelTombo.Jx.yaml",
    `# yaml-language-server: $schema=https://aka.ms/winget-manifest.version.1.9.0.schema.json
PackageIdentifier: KaelTombo.Jx
PackageVersion: ${version}
DefaultLocale: en-US
ManifestType: version
ManifestVersion: 1.9.0
`,
  );

  files.set(
    "winget/KaelTombo.Jx/KaelTombo.Jx.installer.yaml",
    `# yaml-language-server: $schema=https://aka.ms/winget-manifest.installer.1.9.0.schema.json
PackageIdentifier: KaelTombo.Jx
PackageVersion: ${version}
InstallerType: zip
NestedInstallerType: portable
NestedInstallerFiles:
  - RelativeFilePath: jx-${tag}-x86_64-pc-windows-msvc\\jx.exe
    PortableCommandAlias: jx
Installers:
  - Architecture: x64
    InstallerUrl: ${windows.url}
    InstallerSha256: ${windows.hash.toUpperCase()}
ManifestType: installer
ManifestVersion: 1.9.0
`,
  );

  files.set(
    "winget/KaelTombo.Jx/KaelTombo.Jx.locale.en-US.yaml",
    `# yaml-language-server: $schema=https://aka.ms/winget-manifest.defaultLocale.1.9.0.schema.json
PackageIdentifier: KaelTombo.Jx
PackageVersion: ${version}
PackageLocale: en-US
Publisher: kael-tombo
PublisherUrl: https://github.com/${repo}
PublisherSupportUrl: https://github.com/${repo}/issues
PackageName: jx
PackageUrl: https://github.com/${repo}
License: Apache-2.0 OR MIT
LicenseUrl: https://github.com/${repo}/blob/main/LICENSE-APACHE
ShortDescription: A Cargo/uv-style front door for Java
Description: jx is a single-binary workflow tool for Java. It manages JDKs, resolves dependencies from Maven Central into a reproducible lockfile, and builds, tests, runs and packages Java projects from one small manifest.
Moniker: jx
Tags:
  - java
  - jdk
  - build-tool
  - maven
  - gradle
ManifestType: defaultLocale
ManifestVersion: 1.9.0
`,
  );

  if (sourceDir) {
    const readme = join(sourceDir, "packaging", "README.md");
    if (existsSync(readme)) {
      files.set("README.md", readFileSync(readme, "utf8"));
    }
  }

  const written = [];
  for (const [relative, content] of files) {
    const path = join(outDir, relative);
    mkdirSync(dirname(path), { recursive: true });
    writeFileSync(path, content);
    written.push(path);
  }
  return written;
}

function main() {
  const [tag, dist = "dist"] = process.argv.slice(2);
  if (!tag) {
    console.error("usage: node scripts/render-packages.mjs <tag> [dist]");
    process.exit(2);
  }
  const distDir = resolve(dist);
  const sumsPath = join(distDir, "SHA256SUMS");
  if (!existsSync(sumsPath)) {
    console.error(`render-packages: missing ${sumsPath}; run this after a release is published`);
    process.exit(1);
  }
  const sums = parseChecksums(readFileSync(sumsPath, "utf8"));
  const written = renderPackages({
    tag,
    sums,
    outDir: join(distDir, "packaging"),
    sourceDir: resolve(dirname(fileURLToPath(import.meta.url)), ".."),
  });
  for (const path of written) {
    console.log(path);
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  main();
}
