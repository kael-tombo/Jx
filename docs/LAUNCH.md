# Launch gates

The public folder is a **V1 preview**, not a tagged release. Production is
blocked until every item below is closed. `node scripts/check-todos.mjs --production`
returns non-zero while any `{{TODO}}` remains. The 21 current placeholders are
grouped under the gates below; `node scripts/check-todos.mjs` prints the exact
file and line for each.

## Preview deployment

* Live preview: https://kael-tombo.github.io/Jx/
* Workflow: `.github/workflows/pages.yml`, uploading `public/` as the Pages
  artifact on pushes to `main`.
* Verified 2026-11-01: all 13 pages load with one H1 and no console errors or
  failed assets; the shared CSS, JavaScript and fonts return 200; the custom
  404 returns 404 and its CSS loads.
* GitHub Pages ignores `_headers`. The live preview therefore has no CSP
  header, no `X-Frame-Options` and no `Permissions-Policy`. This is an open
  security gate, not a completed one.

## Owner decisions

| # | Gate | Where | State |
| --- | --- | --- | --- |
| 1 | Project name and final domain | `site.config.json`, every canonical URL, OG metadata | Open: `{{TODO: final domain}}` |
| 2 | Name collision review for `jx` (Jenkins X and any other CLI) | Search results, GitHub organisation, package names | Open. The name is a placeholder. |
| 3 | Public repository | Footer GitHub link and JSON-LD `codeRepository` | URL is set from the `origin` remote. Open: confirm the repository is public before launch. |
| 4 | Final licence | Footer, JSON-LD `license` | Closed: the repository and both licence files state Apache-2.0 OR MIT; the footer matches. |
| 5 | Owner and governance | FAQ, contributing docs | Open: `{{TODO: owner and governance}}` |
| 6 | Public milestone visibility | Roadmap and landing roadmap strip | Closed for implementation status: V1 and M0–M8 are marked implemented; gates remain open. ADR 0103. |
| 7 | Signup endpoint and wording | Landing follow section | Open: `{{TODO: signup endpoint}}`. Add the form only when the endpoint exists. |
| 8 | Analytics decision | Privacy page, CSP | Open: `{{TODO: analytics decision}}`. Default is none. |
| 9 | Hosting choice | `public/_headers` | Chosen: GitHub Pages. `_headers` is not applied there, so the live preview has no CSP header. Use a host that supports `_headers` if the strict header is required. |
| 10 | Accessibility review owner and date | `/accessibility` | Open: `{{TODO}}` |
| 11 | Decision gates for the roadmap | `/roadmap` | Open: `{{TODO: date}}` and `{{TODO: owner}}` |
| 12 | Trade-off limit in the vision | `/vision` | Open: `{{TODO: trade-off limit}}` |

## Before flipping indexing

1. Fill every placeholder above and run `npm run check:production`.
2. Set `indexable` to `true` in `site.config.json`.
3. Remove `<meta name="robots" content="noindex">` from all 13 pages. The pages
   currently share that line, so the change is mechanical.
4. Replace the `Disallow: /` line in `public/robots.txt` with `Allow: /` and add
   the final sitemap URL.
5. Replace every canonical and Open Graph URL with the final domain.
6. Re-run Lighthouse and confirm SEO reaches 95. The preview score is 58 only
   because crawling is intentionally off and the canonical URL is a placeholder.

## Before publishing

1. Serve the exact `public/` folder on the chosen host. Do not add a build step.
2. GitHub Pages does not apply `_headers`; the preview has no CSP. If the strict
   header is required, choose a host that supports `_headers` or put a proxy in
   front. Do not treat the local `_headers` file as proof of a live header.
3. Test `/missing-page` returns the custom 404.
4. Confirm the signup form, if added, posts only to the configured endpoint and
   has the consent sentence.
5. Run `npm test`, `npm run check:production`, `npm run spellcheck` and
   `npm run lighthouse` on the deployed artifact.
6. Re-check the ten-second test with a person who has not seen the project.

## Sign-off

| Role | Name | Date |
| --- | --- | --- |
| Owner (name, domain, licence, repository) | `{{TODO}}` | `{{TODO}}` |
| Accessibility reviewer | `{{TODO}}` | `{{TODO}}` |
| Release approver | `{{TODO}}` | `{{TODO}}` |
