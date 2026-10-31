# Launch gates

The public folder is a **design preview**, not a release. Production is blocked
until every item below is closed. `node scripts/check-todos.mjs --production`
returns non-zero while any `{{TODO}}` remains. The 21 current placeholders are
grouped under the gates below; `node scripts/check-todos.mjs` prints the exact
file and line for each.

## Owner decisions

| # | Gate | Where | State |
| --- | --- | --- | --- |
| 1 | Project name and final domain | `site.config.json`, every canonical URL, OG metadata | Open: `{{TODO: final domain}}` |
| 2 | Name collision review for `jx` (Jenkins X and any other CLI) | Search results, GitHub organisation, package names | Open. The name is a placeholder. |
| 3 | Public repository | Footer GitHub link and JSON-LD `codeRepository` | URL is set from the `origin` remote. Open: confirm the repository is public before launch. |
| 4 | Final licence | Footer, JSON-LD `license` | Open: Apache-2.0 OR MIT is proposed |
| 5 | Owner and governance | FAQ, contributing docs | Open: `{{TODO: owner and governance}}` |
| 6 | Public milestone visibility | Roadmap and landing roadmap strip | Open. Currently "Not released" for every milestone; no progress is published. |
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
2. Confirm the CSP header is actually returned. Opening `tests/server.mjs` does
   not prove the host applies `_headers`.
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
