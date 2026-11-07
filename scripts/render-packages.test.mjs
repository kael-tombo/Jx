import assert from "node:assert/strict";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import { parseChecksums, renderPackages } from "./render-packages.mjs";

const TAG = "v1.2.3";
const ASSETS = [
  "jx-v1.2.3-x86_64-unknown-linux-gnu.tar.gz",
  "jx-v1.2.3-aarch64-unknown-linux-gnu.tar.gz",
  "jx-v1.2.3-x86_64-apple-darwin.tar.gz",
  "jx-v1.2.3-aarch64-apple-darwin.tar.gz",
  "jx-v1.2.3-x86_64-pc-windows-msvc.zip",
];
const HASH = "a".repeat(64);
const SUMS = new Map(ASSETS.map((name) => [name, HASH]));

function render() {
  const dir = mkdtempSync(join(tmpdir(), "jx-packages-"));
  try {
    const files = renderPackages({
      tag: TAG,
      sums: SUMS,
      outDir: dir,
      repo: "kael-tombo/Jx",
    });
    return { dir, files };
  } catch (error) {
    rmSync(dir, { recursive: true, force: true });
    throw error;
  }
}

test("parses sha256sum output", () => {
  const sums = parseChecksums(
    `${HASH}  jx-v1.2.3-x86_64-unknown-linux-gnu.tar.gz\n${"b".repeat(64)} *jx-v1.2.3-x86_64-pc-windows-msvc.zip\n`,
  );
  assert.equal(sums.get("jx-v1.2.3-x86_64-unknown-linux-gnu.tar.gz"), HASH);
  assert.equal(
    sums.get("jx-v1.2.3-x86_64-pc-windows-msvc.zip"),
    "b".repeat(64),
  );
});

test("renders all four package-manager trees", () => {
  const { dir, files } = render();
  try {
    assert.equal(files.length, 8);
    const nuspec = readFileSync(join(dir, "chocolatey/jx.nuspec"), "utf8");
    assert.match(nuspec, /<version>1\.2\.3<\/version>/);
    assert.match(nuspec, /<id>jx<\/id>/);

    const install = readFileSync(
      join(dir, "chocolatey/tools/chocolateyinstall.ps1"),
      "utf8",
    );
    assert.match(install, /Install-ChocolateyZipPackage/);
    assert.match(install, /Install-BinFile/);
    assert.match(
      install,
      /https:\/\/github\.com\/kael-tombo\/Jx\/releases\/download\/v1\.2\.3\/jx-v1\.2\.3-x86_64-pc-windows-msvc\.zip/,
    );
    assert.match(install, new RegExp(`\\$checksum64 = '${HASH}'`));

    const formula = readFileSync(join(dir, "homebrew/jx.rb"), "utf8");
    assert.match(formula, /version "1\.2\.3"/);
    assert.match(formula, /on_macos do/);
    assert.match(formula, /on_linux do/);
    assert.match(formula, /bin\.install "jx"/);
    assert.match(formula, /assert_match version\.to_s/);

    const scoop = JSON.parse(readFileSync(join(dir, "scoop/jx.json"), "utf8"));
    assert.equal(scoop.version, "1.2.3");
    assert.equal(scoop.hash, undefined);
    assert.equal(scoop.architecture["64bit"].hash, HASH);
    assert.equal(
      scoop.extract_dir,
      "jx-v1.2.3-x86_64-pc-windows-msvc",
    );

    const versionManifest = readFileSync(
      join(dir, "winget/KaelTombo.Jx/KaelTombo.Jx.yaml"),
      "utf8",
    );
    const installer = readFileSync(
      join(dir, "winget/KaelTombo.Jx/KaelTombo.Jx.installer.yaml"),
      "utf8",
    );
    const locale = readFileSync(
      join(dir, "winget/KaelTombo.Jx/KaelTombo.Jx.locale.en-US.yaml"),
      "utf8",
    );
    assert.match(
      versionManifest,
      /^# yaml-language-server: \$schema=https:\/\/aka\.ms\/winget-manifest\.version\.1\.9\.0\.schema\.json/,
    );
    assert.match(
      installer,
      /^# yaml-language-server: \$schema=https:\/\/aka\.ms\/winget-manifest\.installer\.1\.9\.0\.schema\.json/,
    );
    assert.match(
      locale,
      /^# yaml-language-server: \$schema=https:\/\/aka\.ms\/winget-manifest\.defaultLocale\.1\.9\.0\.schema\.json/,
    );
    assert.match(installer, /InstallerSha256: A{64}/);
    assert.match(
      installer,
      /RelativeFilePath: jx-v1\.2\.3-x86_64-pc-windows-msvc\\jx\.exe/,
    );
    assert.match(installer, /PortableCommandAlias: jx/);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test("a missing archive is refused", () => {
  const incomplete = new Map(SUMS);
  incomplete.delete("jx-v1.2.3-aarch64-apple-darwin.tar.gz");
  assert.throws(
    () =>
      renderPackages({
        tag: TAG,
        sums: incomplete,
        outDir: mkdtempSync(join(tmpdir(), "jx-packages-")),
      }),
    /SHA256SUMS has no entry for jx-v1\.2\.3-aarch64-apple-darwin\.tar\.gz/,
  );
});

test("a malformed tag is refused before writing", () => {
  assert.throws(
    () =>
      renderPackages({
        tag: "1.2.3",
        sums: SUMS,
        outDir: mkdtempSync(join(tmpdir(), "jx-packages-")),
      }),
    /tag must look like v1\.0\.0/,
  );
});
