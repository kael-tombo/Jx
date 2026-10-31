import { readFileSync, readdirSync } from "node:fs";
import { extname, join, relative, resolve } from "node:path";

const root = resolve("public");
const allowed = JSON.parse(readFileSync("scripts/allowed-numbers.json", "utf8")).allowed.map(normalise);
const errors = [];

const banned = [
  "blazing",
  "lightning",
  "revolutionary",
  "game-changing",
  "seamless",
  "effortless",
  "production-ready",
  "stable",
  "battle-tested",
  "enterprise-grade",
  "10x",
  "the future of",
];

const numberPatterns = [
  /\b\d[\d,]*(?:\.\d+)?\s*(?:%|percent\b)/gi,
  /\b\d[\d,]*(?:\.\d+)?x\b/gi,
  /\b\d[\d,]*(?:\.\d+)?\s*(?:ms|s|secs?|seconds?|mins?|minutes?|hours?|days?|weeks?|months?|years?|bytes?|kb|mb|gb|stars?|users?|downloads?|developers?)\b/gi,
];

function normalise(value) {
  return value.toLowerCase().replace(/\s+/g, " ").trim();
}

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

function fail(file, line, message) {
  errors.push(`${file}:${line} ${message}`);
}

const pages = walk(root).filter((file) => extname(file) === ".html");

for (const file of pages) {
  const path = relative(process.cwd(), file).split("\\").join("/");
  const source = readFileSync(file, "utf8");

  if (!/<main\b[^>]*\bdata-status="[^"]+"/i.test(source)) {
    fail(path, 1, "the <main> element has no data-status. Add the page status.");
  }

  const h1Count = (source.match(/<h1\b/gi) ?? []).length;
  if (h1Count !== 1) {
    fail(path, 1, `expected one <h1>, found ${h1Count}. Keep one page heading.`);
  }

  if (/<style\b/i.test(source)) {
    fail(path, lineOf(source, source.search(/<style\b/i)), "inline <style> is not allowed. Move the rule to public/css.");
  }

  for (const match of source.matchAll(/\sstyle\s*=/gi)) {
    fail(path, lineOf(source, match.index), "a style attribute is not allowed. Use a class in public/css.");
  }

  for (const match of source.matchAll(/<script\b([^>]*)>([\s\S]*?)<\/script>/gi)) {
    const attributes = match[1];
    if (/\bsrc\s*=/.test(attributes)) continue;
    if (/type\s*=\s*["']application\/ld\+json["']/i.test(attributes)) continue;
    fail(path, lineOf(source, match.index), "inline JavaScript is not allowed. Use a module in public/js.");
  }

  for (const match of source.matchAll(/data-status\s*=\s*["'](implemented|shipped)["'][^>]*>/gi)) {
    const tag = match[0];
    if (!/\bdata-evidence\s*=/i.test(tag) || !/\bdata-since\s*=/i.test(tag)) {
      fail(path, lineOf(source, match.index), `status "${match[1]}" needs data-evidence and data-since. Remove the status until a release exists.`);
    }
  }

  const withoutCode = source
    .replace(/<script\b[\s\S]*?<\/script>/gi, " ")
    .replace(/<style\b[\s\S]*?<\/style>/gi, " ");
  const attributeText = [...source.matchAll(/\b(?:alt|title|content)\s*=\s*"([^"]*)"/gi)].map((match) => match[1]).join(" ");
  const text = `${withoutCode.replace(/<[^>]+>/g, " ")} ${attributeText}`
    .replace(/&[a-z]+;/gi, " ")
    .replace(/\s+/g, " ");

  for (const word of banned) {
    const pattern = new RegExp(`\\b${word.replace(/[.*+?^${}()|[\]\\]/g, "\\$&").replace(/\s+/g, "\\s+")}\\b`, "i");
    if (pattern.test(text)) {
      const rawIndex = source.toLowerCase().search(/\b(?:blazing|lightning|revolutionary|game-changing|seamless|effortless|production-ready|stable|battle-tested|enterprise-grade|10x|the future of)\b/i);
      fail(path, rawIndex >= 0 ? lineOf(source, rawIndex) : 1, `banned word or phrase "${word}". Replace it with a concrete, verifiable statement.`);
    }
  }

  for (const pattern of numberPatterns) {
    for (const match of text.matchAll(pattern)) {
      const value = normalise(match[0]);
      if (allowed.includes(value)) continue;
      fail(path, 1, `unregistered number "${match[0].trim()}". Add it to scripts/allowed-numbers.json with its source, or remove it.`);
    }
  }

  for (const match of source.matchAll(/<figure\b[^>]*class="[^"]*\bterminal\b[^"]*"[^>]*>([\s\S]*?)<\/figure>/gi)) {
    const caption = match[1].match(/<figcaption\b[^>]*>([\s\S]*?)<\/figcaption>/i);
    if (!caption || !/illustrative/i.test(caption[1]) || !/not a recording/i.test(caption[1])) {
      fail(path, lineOf(source, match.index), "a terminal figure needs an illustrative caption containing \"Illustrative\" and \"not a recording\".");
    }
  }

  const footer = source.match(/<footer\b[^>]*class="[^"]*site-footer[^"]*"[^>]*>([\s\S]*?)<\/footer>/i);
  if (!footer) {
    fail(path, lineOf(source, source.indexOf("<footer")), "the shared site footer is missing.");
  } else {
    if (!/Java is a trademark of Oracle/.test(footer[1])) {
      fail(path, lineOf(source, footer.index), "the footer lacks the Java trademark notice.");
    }
    if (!/not affiliated with or endorsed by Oracle/.test(footer[1])) {
      fail(path, lineOf(source, footer.index), "the footer lacks the independent-project notice.");
    }
  }
}

if (errors.length) {
  console.log(`check-claims: ${errors.length} problem(s)\n`);
  for (const error of errors) console.log(`FAIL ${error}`);
  process.exit(1);
}

console.log(`check-claims: ${pages.length} pages passed.`);
