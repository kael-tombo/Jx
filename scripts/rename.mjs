import { execFileSync } from "node:child_process";
import { readFileSync, readdirSync, writeFileSync } from "node:fs";
import { extname, join, relative, resolve } from "node:path";

const textExtensions = new Set([".html", ".css", ".js", ".mjs", ".json", ".svg", ".txt", ".xml", ".md"]);
const targets = ["public", "design"];

function walk(dir, files = []) {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    if (entry.name.startsWith(".")) continue;
    const path = join(dir, entry.name);
    if (entry.isDirectory()) walk(path, files);
    else files.push(path);
  }
  return files;
}

function stop(message) {
  console.log(`rename: ${message}`);
  process.exit(1);
}

const newName = process.argv[2];
if (!newName) stop("usage: node scripts/rename.mjs <new-name>");
if (!/^[a-z][a-z0-9-]{1,23}$/.test(newName)) {
  stop("the new name must start with a lowercase letter and contain only a-z, 0-9 and hyphens (2–24 characters).");
}

let status = "";
try {
  status = execFileSync("git", ["status", "--porcelain"], { encoding: "utf8" });
} catch {
  stop("this is not a git repository. Rename refuses to run without a clean-tree check.");
}
if (status.trim()) stop("the git tree is dirty. Commit or stash first, then run the rename.");

const configPath = resolve("site.config.json");
const config = JSON.parse(readFileSync(configPath, "utf8"));
const oldName = config.name;
if (newName === oldName) stop("the new name is the same as the current name.");
if (!/^[a-z][a-z0-9-]{0,23}$/.test(oldName)) stop("the current name in site.config.json is not a valid lowercase word.");

const escape = (value) => value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
const pattern = new RegExp(`\\b${escape(oldName)}\\b`, "gi");
const detect = new RegExp(`\\b${escape(oldName)}\\b`, "i");

function preserveCase(match) {
  if (match === match.toUpperCase()) return newName.toUpperCase();
  if (match[0] === match[0].toUpperCase()) return newName[0].toUpperCase() + newName.slice(1);
  return newName;
}

const files = [];
for (const target of targets) files.push(...walk(resolve(target)));
files.push(configPath);

const textFiles = files.filter((file) => textExtensions.has(extname(file)) || file.endsWith("_headers"));
let changedFiles = 0;
let replacements = 0;

for (const file of textFiles) {
  const before = readFileSync(file, "utf8");
  if (!detect.test(before)) continue;
  const after = before.replace(pattern, (match) => {
    replacements += 1;
    return preserveCase(match);
  });
  if (after !== before) {
    writeFileSync(file, after);
    changedFiles += 1;
  }
}

const leftovers = [];
for (const file of textFiles) {
  const source = readFileSync(file, "utf8");
  source.split(/\r?\n/).forEach((line, index) => {
    if (detect.test(line)) leftovers.push(`${relative(process.cwd(), file)}:${index + 1} ${line.trim()}`);
  });
}

console.log(`rename: ${oldName} → ${newName}`);
console.log(`changed ${changedFiles} file(s), ${replacements} occurrence(s).`);

if (leftovers.length) {
  console.log("\nFAIL the old name still appears:");
  for (const item of leftovers) console.log(`  ${item}`);
  process.exit(1);
}

console.log("No trace of the old name remains in public/, design/ or site.config.json.");
console.log("Review the diff, especially the repository URL. Paths outside those directories (the Rust workspace, package.json, docs) were not changed.");
