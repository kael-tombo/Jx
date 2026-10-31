import { readFileSync, readdirSync } from "node:fs";
import { extname, join, relative, resolve } from "node:path";

const root = resolve("public");
const production = process.argv.includes("--production");
const binary = new Set([".png", ".jpg", ".jpeg", ".gif", ".webp", ".avif", ".ico", ".woff", ".woff2", ".ttf", ".otf"]);
const found = [];

function walk(dir, files = []) {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    if (entry.name.startsWith(".")) continue;
    const path = join(dir, entry.name);
    if (entry.isDirectory()) walk(path, files);
    else files.push(path);
  }
  return files;
}

for (const file of walk(root)) {
  if (binary.has(extname(file).toLowerCase())) continue;
  const source = readFileSync(file, "utf8");
  const lines = source.split(/\r?\n/);
  lines.forEach((line, index) => {
    if (!line.includes("{{TODO")) return;
    found.push({
      file: relative(process.cwd(), file).split("\\").join("/"),
      line: index + 1,
      text: line.trim(),
    });
  });
}

if (found.length === 0) {
  console.log("check-todos: no placeholders in public/.");
  process.exit(0);
}

console.log(`check-todos: ${found.length} placeholder(s) in public/`);
for (const item of found) console.log(`${item.file}:${item.line} ${item.text}`);

if (production) {
  console.log("\nProduction is blocked until every placeholder has an owner-supplied value.");
  process.exit(1);
}

console.log("\nPreview mode: listed only. Use --production to block a deployment.");
