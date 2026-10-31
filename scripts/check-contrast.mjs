import { readFileSync } from "node:fs";

const css = readFileSync("public/css/tokens.css", "utf8");
const required = [
  "paper",
  "ink",
  "muted",
  "line",
  "control-line",
  "code-bg",
  "accent",
  "on-accent",
  "ok",
  "terminal-bg",
  "terminal-ink",
  "terminal-muted",
  "terminal-accent",
];

function blockAfter(selector, from = 0) {
  const start = css.indexOf(selector, from);
  if (start < 0) return null;
  const open = css.indexOf("{", start + selector.length);
  if (open < 0) return null;
  let depth = 0;
  for (let index = open; index < css.length; index += 1) {
    if (css[index] === "{") depth += 1;
    if (css[index] === "}") {
      depth -= 1;
      if (depth === 0) return css.slice(open + 1, index);
    }
  }
  return null;
}

function vars(body) {
  const values = {};
  if (!body) return values;
  for (const match of body.matchAll(/(--[\w-]+)\s*:\s*(#[0-9a-fA-F]{3,8})\s*;/g)) {
    values[match[1]] = match[2];
  }
  return values;
}

const rootMatch = css.match(/^:root\s*\{/m);
const light = vars(rootMatch ? blockAfter(rootMatch[0].replace(/\s*\{$/, "")) : null);
const mediaStart = css.indexOf("@media (prefers-color-scheme: dark)");
const darkMedia = { ...light, ...vars(blockAfter(':root:not([data-theme="light"])', mediaStart)) };
const darkAttr = { ...light, ...vars(blockAfter(':root[data-theme="dark"]')) };

const themes = [
  { name: "light", values: light },
  { name: "dark (system)", values: darkMedia },
  { name: "dark (manual)", values: darkAttr },
];

function hex(value) {
  const clean = value.trim().replace("#", "");
  const full = clean.length === 3 ? clean.split("").map((part) => part + part).join("") : clean;
  return [0, 2, 4].map((offset) => parseInt(full.slice(offset, offset + 2), 16));
}

function luminance(value) {
  const [r, g, b] = hex(value).map((channel) => {
    const c = channel / 255;
    return c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

function ratio(a, b) {
  const first = luminance(a);
  const second = luminance(b);
  return (Math.max(first, second) + 0.05) / (Math.min(first, second) + 0.05);
}

const pairs = [
  ["ink", "paper", 4.5, "text", "body text"],
  ["muted", "paper", 4.5, "text", "muted text"],
  ["accent", "paper", 4.5, "text", "links"],
  ["on-accent", "accent", 4.5, "text", "primary button"],
  ["paper", "ink", 4.5, "text", "skip link and inverse button"],
  ["ink", "code-bg", 4.5, "text", "code text"],
  ["muted", "code-bg", 4.5, "text", "code header"],
  ["ok", "paper", 3, "ui", "status dot"],
  ["control-line", "paper", 3, "ui", "control border"],
  ["control-line", "code-bg", 3, "ui", "copy button border"],
  ["terminal-ink", "terminal-bg", 4.5, "text", "terminal text"],
  ["terminal-muted", "terminal-bg", 4.5, "text", "terminal label and caption"],
  ["terminal-accent", "terminal-bg", 4.5, "text", "terminal prompt"],
];

const errors = [];

for (const theme of themes) {
  for (const name of required) {
    if (!theme.values[`--${name}`]) errors.push(`${theme.name}: token --${name} is missing or not a hex colour.`);
  }

  for (const [foreground, background, minimum, kind, label] of pairs) {
    const fg = theme.values[`--${foreground}`];
    const bg = theme.values[`--${background}`];
    if (!fg || !bg) continue;
    const value = ratio(fg, bg);
    const ok = value >= minimum;
    const line = `${theme.name.padEnd(13)} ${label.padEnd(28)} ${fg} on ${bg}  ${value.toFixed(2)}:1  needs ${minimum}:1  ${ok ? "pass" : "FAIL"}`;
    console.log(line);
    if (!ok) {
      errors.push(`${theme.name}: ${label} is ${value.toFixed(2)}:1 (needs ${minimum}:1). Adjust --${foreground} or --${background}.`);
    }
  }
}

if (darkMedia.paper !== darkAttr.paper || darkMedia.accent !== darkAttr.accent || darkMedia.ink !== darkAttr.ink) {
  errors.push("dark (system) and dark (manual) diverge. Keep both override blocks in sync.");
}

if (errors.length) {
  console.log(`\ncheck-contrast: ${errors.length} problem(s)`);
  for (const error of errors) console.log(`FAIL ${error}`);
  process.exit(1);
}

console.log("\ncheck-contrast: all text pairs meet 4.5:1 and UI pairs meet 3:1.");
console.log("Note: --line is a decorative hairline, not a UI boundary; --control-line is checked at 3:1.");
