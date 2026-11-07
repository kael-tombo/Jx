#!/usr/bin/env node
// Validate the generated OS package manifests with the real tools installed
// on this machine. A missing tool is reported as SKIP, never as a pass.
//
//   node scripts/validate-packages.mjs
//
// The release workflow still owns publication; this catches template and
// schema regressions before a tag is cut.

import { spawnSync } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { renderPackages } from "./render-packages.mjs";

const TAG = "v1.2.3";
const VERSION = "1.2.3";
const ASSETS = [
  "jx-v1.2.3-x86_64-unknown-linux-gnu.tar.gz",
  "jx-v1.2.3-aarch64-unknown-linux-gnu.tar.gz",
  "jx-v1.2.3-x86_64-apple-darwin.tar.gz",
  "jx-v1.2.3-aarch64-apple-darwin.tar.gz",
  "jx-v1.2.3-x86_64-pc-windows-msvc.zip",
];
const HASH = "a".repeat(64);

function run(command, args, options = {}) {
  return spawnSync(command, args, {
    encoding: "utf8",
    timeout: 120_000,
    ...options,
  });
}

function available(command, args = ["--version"]) {
  const result = run(command, args);
  return !result.error;
}

function detail(result) {
  return [result.stdout, result.stderr]
    .filter(Boolean)
    .join("\n")
    .trim()
    .slice(0, 2000);
}

function report(status, name, message = "") {
  const suffix = message ? `: ${message}` : "";
  console.log(`  ${status.padEnd(4)} ${name}${suffix}`);
}

function powershellCheck(scripts, nuspec) {
  const env = {
    ...process.env,
    JX_PACKAGE_SCRIPTS: scripts.join(";"),
    JX_PACKAGE_NUSPEC: nuspec,
  };
  const command = [
    "$errors = $null",
    "foreach ($file in ($env:JX_PACKAGE_SCRIPTS -split ';')) {",
    "  [void][System.Management.Automation.Language.Parser]::ParseFile($file, [ref]$null, [ref]$errors)",
    "}",
    "$null = [xml](Get-Content -Raw $env:JX_PACKAGE_NUSPEC)",
    "if ($errors.Count) { $errors | Out-String | Write-Error; exit 1 }",
    "exit 0",
  ].join("; ");
  return run("powershell", ["-NoProfile", "-NonInteractive", "-Command", command], {
    env,
  });
}

function main() {
  const dir = mkdtempSync(join(tmpdir(), "jx-package-validation-"));
  let failed = false;
  try {
    const sums = new Map(ASSETS.map((name) => [name, HASH]));
    const files = renderPackages({ tag: TAG, sums, outDir: dir });
    console.log(`package validation (${TAG}, ${files.length} files)`);

    const wingetDir = join(dir, "winget", "KaelTombo.Jx");
    if (process.platform === "win32" && available("winget")) {
      const result = run("winget", ["validate", "--manifest", wingetDir]);
      if (result.status === 0) {
        report("PASS", "winget validate");
      } else {
        failed = true;
        report("FAIL", "winget validate", detail(result));
      }
    } else {
      report("SKIP", "winget validate", "winget is unavailable");
    }

    if (process.platform === "win32" && available("powershell")) {
      const result = powershellCheck(
        [
          join(dir, "chocolatey", "tools", "chocolateyinstall.ps1"),
          join(dir, "chocolatey", "tools", "chocolateyuninstall.ps1"),
        ],
        join(dir, "chocolatey", "jx.nuspec"),
      );
      if (result.status === 0) {
        report("PASS", "Chocolatey XML and PowerShell syntax");
      } else {
        failed = true;
        report("FAIL", "Chocolatey XML and PowerShell syntax", detail(result));
      }
    } else {
      report("SKIP", "Chocolatey XML and PowerShell syntax", "PowerShell is unavailable");
    }

    if (available("ruby")) {
      const result = run("ruby", ["-c", join(dir, "homebrew", "jx.rb")]);
      if (result.status === 0) {
        report("PASS", "Homebrew formula syntax (ruby -c)");
      } else {
        failed = true;
        report("FAIL", "Homebrew formula syntax (ruby -c)", detail(result));
      }
    } else {
      report("SKIP", "Homebrew formula syntax (ruby -c)", "ruby is unavailable");
    }

    if (available("choco")) {
      const result = run(
        "choco",
        ["pack", join(dir, "chocolatey", "jx.nuspec"), "--out", join(dir, "chocolatey")],
        { cwd: dir },
      );
      const expected = join(dir, "chocolatey", `jx.${VERSION}.nupkg`);
      if (result.status === 0 && existsSync(expected) && readFileSync(expected).length > 0) {
        report("PASS", "Chocolatey pack");
      } else {
        failed = true;
        report(
          "FAIL",
          "Chocolatey pack",
          existsSync(expected) ? detail(result) : `no ${expected}`,
        );
      }
    } else {
      report("SKIP", "Chocolatey pack", "choco is unavailable");
    }

    // Scoop has no manifest validator; parsing is what the bucket consumes.
    const scoop = JSON.parse(readFileSync(join(dir, "scoop", "jx.json"), "utf8"));
    if (scoop.version !== VERSION || !scoop.architecture?.["64bit"]?.hash) {
      failed = true;
      report("FAIL", "Scoop manifest JSON");
    } else {
      report("PASS", "Scoop manifest JSON");
    }
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
  if (failed) process.exit(1);
}

main();
