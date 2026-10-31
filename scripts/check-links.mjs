import { existsSync, readFileSync, readdirSync, statSync } from "node:fs";
import { dirname, extname, join, relative, resolve } from "node:path";

const root = resolve("public");
const errors = [];

function walk(dir, files = []) {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    if (entry.name.startsWith(".")) continue;
    const path = join(dir, entry.name);
    if (entry.isDirectory()) walk(path, files);
    else files.push(path);
  }
  return files;
}

function lineOf(source, index) {
  return source.slice(0, index).split(/\r?\n/).length;
}

function checkTarget(source, sourcePath, raw, label, index) {
  const value = raw.trim();
  if (!value) return;
  if (value.startsWith("{{TODO")) return;
  if (/^(?:https?:|mailto:|tel:|javascript:|data:)/i.test(value)) return;

  const [pathPart, hash = ""] = value.split("#");
  const targetPath = pathPart ? resolve(dirname(sourcePath), decodeURIComponent(pathPart)) : sourcePath;
  const display = relative(process.cwd(), sourcePath).split("\\").join("/");

  if (!existsSync(targetPath)) {
    errors.push(`${display}:${lineOf(source, index)} ${label}="${value}" points to a missing file. Fix the path or remove the link.`);
    return;
  }

  if (hash) {
    if (extname(targetPath) !== ".html") return;
    const targetSource = readFileSync(targetPath, "utf8");
    const id = decodeURIComponent(hash);
    const found = new RegExp(`\\bid\\s*=\\s*["']${id.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}["']`).test(targetSource);
    if (!found) {
      errors.push(`${display}:${lineOf(source, index)} ${label}="${value}" targets an id that does not exist in ${relative(process.cwd(), targetPath)}.`);
    }
  }
}

const htmlFiles = walk(root).filter((file) => extname(file) === ".html");

for (const file of htmlFiles) {
  const source = readFileSync(file, "utf8");
  for (const match of source.matchAll(/\b(href|src)\s*=\s*"([^"]*)"/gi)) {
    checkTarget(source, file, match[2], match[1], match.index);
  }
}

const cssFiles = walk(root).filter((file) => extname(file) === ".css");
for (const file of cssFiles) {
  const source = readFileSync(file, "utf8");
  for (const match of source.matchAll(/url\(\s*["']?([^"')]+)["']?\s*\)/gi)) {
    const value = match[1].trim();
    if (/^(?:data:|https?:|#)/i.test(value)) continue;
    const target = resolve(dirname(file), value.split(/[?#]/)[0]);
    if (!existsSync(target) || !statSync(target).isFile()) {
      const display = relative(process.cwd(), file).split("\\").join("/");
      errors.push(`${display}:${lineOf(source, match.index)} url("${value}") points to a missing asset.`);
    }
  }
}

if (errors.length) {
  console.log(`check-links: ${errors.length} problem(s)\n`);
  for (const error of errors) console.log(`FAIL ${error}`);
  process.exit(1);
}

console.log(`check-links: ${htmlFiles.length} pages and ${cssFiles.length} stylesheets have no broken local links.`);
