import { createServer } from "node:http";
import { readFile, stat } from "node:fs/promises";
import { extname, join, resolve, sep } from "node:path";
import { gzipSync } from "node:zlib";

const root = resolve("public");
const port = Number(process.env.JX_PORT ?? 4173);
const host = process.env.JX_HOST ?? "127.0.0.1";

const types = {
  ".css": "text/css; charset=utf-8",
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".json": "application/json; charset=utf-8",
  ".mjs": "text/javascript; charset=utf-8",
  ".png": "image/png",
  ".svg": "image/svg+xml",
  ".txt": "text/plain; charset=utf-8",
  ".woff2": "font/woff2",
  ".xml": "application/xml; charset=utf-8",
};

const compressible = new Set([".css", ".html", ".js", ".json", ".mjs", ".svg", ".txt", ".xml"]);

function matches(pattern, pathname) {
  if (pattern === "/*") return true;
  if (pattern.endsWith("/*")) return pathname.startsWith(pattern.slice(0, -1));
  if (pattern.startsWith("*.")) return pathname.endsWith(pattern.slice(1));
  return pattern === pathname;
}

async function headerRules() {
  try {
    const text = await readFile(join(root, "_headers"), "utf8");
    return text
      .trim()
      .split(/\n\s*\n/)
      .map((block) => {
        const [pattern, ...lines] = block.split("\n");
        return {
          pattern: pattern.trim(),
          headers: lines
            .filter((line) => line.includes(":"))
            .map((line) => {
              const index = line.indexOf(":");
              return [line.slice(0, index).trim(), line.slice(index + 1).trim()];
            }),
        };
      });
  } catch {
    return [];
  }
}

const rules = await headerRules();

function headersFor(pathname) {
  const headers = {};
  for (const rule of rules) {
    if (!matches(rule.pattern, pathname)) continue;
    for (const [name, value] of rule.headers) headers[name] = value;
  }
  return headers;
}

const server = createServer(async (request, response) => {
  const url = new URL(request.url ?? "/", `http://${request.headers.host ?? `${host}:${port}`}`);
  let pathname = decodeURIComponent(url.pathname);
  if (pathname.endsWith("/")) pathname += "index.html";

  const file = resolve(root, `.${pathname}`);
  if (!file.startsWith(`${root}${sep}`)) {
    response.writeHead(403, { "Content-Type": "text/plain; charset=utf-8" });
    response.end("Forbidden");
    return;
  }

  let body;
  let status = 200;
  let filePath = file;

  try {
    const info = await stat(file);
    if (info.isDirectory()) filePath = join(file, "index.html");
    body = await readFile(filePath);
  } catch {
    status = 404;
    filePath = join(root, "404.html");
    try {
      body = await readFile(filePath);
    } catch {
      body = Buffer.from("Not found");
    }
  }

  const extension = extname(filePath);
  const headers = {
    "Content-Type": types[extension] ?? "application/octet-stream",
    "X-Content-Type-Options": "nosniff",
    Vary: "Accept-Encoding",
    ...headersFor(pathname),
  };

  const wantsGzip = /\bgzip\b/.test(request.headers["accept-encoding"] ?? "");
  if (wantsGzip && compressible.has(extension)) {
    body = gzipSync(body);
    headers["Content-Encoding"] = "gzip";
  }

  headers["Content-Length"] = String(body.length);
  response.writeHead(status, headers);
  if (request.method === "HEAD") response.end();
  else response.end(body);
});

server.listen(port, host, () => {
  const address = server.address();
  const actualPort = typeof address === "object" && address ? address.port : port;
  console.log(`jx site: http://${host}:${actualPort}/`);
});
