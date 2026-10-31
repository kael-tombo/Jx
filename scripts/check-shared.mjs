import { readFileSync, readdirSync } from "node:fs";
import { extname, join, relative, resolve } from "node:path";

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

function normalise(markup) {
  return markup
    .replace(/\s+aria-current="[^"]*"/gi, "")
    .replace(/\b(href|src)="(?:\.\.\/)+/gi, '$1="')
    .replace(/\b(href|src)="docs\//gi, '$1="')
    .replace(/\s+/g, " ")
    .trim();
}

function extract(source, name) {
  const pattern = new RegExp(`<${name}\\b[^>]*class="[^"]*site-${name === "header" ? "header" : "footer"}[^"]*"[^>]*>[\\s\\S]*?<\\/${name}>`, "i");
  const matches = [...source.matchAll(new RegExp(pattern.source, "gi"))];
  return matches.map((match) => normalise(match[0]));
}

function firstDifference(a, b) {
  let index = 0;
  while (index < a.length && index < b.length && a[index] === b[index]) index += 1;
  const start = Math.max(0, index - 30);
  return `at character ${index + 1}\n  expected: ...${a.slice(start, start + 90)}\n  found:    ...${b.slice(start, start + 90)}`;
}

const pages = walk(root).filter((file) => extname(file) === ".html").sort();
const referencePath = join(root, "index.html");
const reference = readFileSync(referencePath, "utf8");
const expectedHeader = extract(reference, "header")[0];
const expectedFooter = extract(reference, "footer")[0];

if (!expectedHeader || !expectedFooter) {
  console.log("check-shared: index.html is missing its header or footer.");
  process.exit(1);
}

for (const file of pages) {
  const path = relative(process.cwd(), file).split("\\").join("/");
  const source = readFileSync(file, "utf8");
  const headers = extract(source, "header");
  const footers = extract(source, "footer");

  if (headers.length !== 1) {
    errors.push(`${path}: expected one site header, found ${headers.length}.`);
  } else if (headers[0] !== expectedHeader) {
    errors.push(`${path}: header differs from index.html. ${firstDifference(expectedHeader, headers[0])}`);
  }

  if (footers.length !== 1) {
    errors.push(`${path}: expected one site footer, found ${footers.length}.`);
  } else if (footers[0] !== expectedFooter) {
    errors.push(`${path}: footer differs from index.html. ${firstDifference(expectedFooter, footers[0])}`);
  }
}

if (errors.length) {
  console.log(`check-shared: ${errors.length} problem(s)\n`);
  for (const error of errors) console.log(`FAIL ${error}`);
  process.exit(1);
}

console.log(`check-shared: ${pages.length} pages share the same header and footer.`);
