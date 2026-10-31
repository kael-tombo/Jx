# Website quality record

Measured 2026-11-01 against the files in this workspace. Every figure below comes
from a command in the reproduction section; none is estimated. The site is a
design preview, so the SEO result is deliberately below target until launch.

## Acceptance

| Requirement | Result | Evidence |
| --- | --- | --- |
| Ten-second test: what, for whom, status | Pass in the browser suite | Hero title, subhead, badge and status strip asserted by Playwright |
| Every claim traceable; no banned hype | Pass | `check-claims`: 13 pages, zero banned words, zero unregistered numbers |
| No release claimed | Pass | Every page `data-status="draft"`; no `implemented` or `shipped` status |
| Rename in one command, no trace | Pass | Dry run in a throwaway clone: 20 files, 179 occurrences renamed, no old-name match |
| Lighthouse mobile, all categories 95+ | Performance, accessibility and best practices pass at 100; SEO is 58 | SEO is blocked by intentional `noindex` and placeholder canonicals; see below |
| axe: no serious or critical issues | Pass | 26 scans: 13 pages, light and dark |
| Shared header and footer | Pass | `check-shared`: 13 pages identical after path and current-page normalisation |
| HTML, links and spelling | Pass | Validator 13 pages; link check 13 pages and 4 stylesheets; cspell 13 files |
| Production gate | Blocked, as intended | 21 `{{TODO}}` placeholders remain in `public/` |
| GitHub Pages preview | Deployed and checked | https://kael-tombo.github.io/Jx/ |

## Performance and size

Lighthouse 12, mobile emulation, single run per URL:

| URL | Performance | Accessibility | Best practices | SEO | FCP | LCP | CLS | TBT |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `/` | 100 | 100 | 100 | 58 | 1.1 s | 1.4 s | 0 | 0 ms |
| `/vision.html` | 100 | 100 | 100 | 58 | 1.2 s | 1.4 s | 0 | 0 ms |
| `/docs/commands.html` | 100 | 100 | 100 | 58 | 1.2 s | 1.4 s | 0 | 0 ms |

* LCP target: 2.0 s or less. Measured 1.4 s on all three URLs.
* CLS target: 0.05 or less. Measured 0 to 0.001 across repeated runs.
* Landing JavaScript target: 10 KB gzipped or less. Measured **2,188 bytes**
  (`theme-init` 244, `theme` 576, `terminal` 1,368). All four modules are 2,723
  bytes.
* Fonts are 41,220 bytes total. The landing page transfers 65 KiB; the docs page
  59 KiB. No raster images load on any page.
* No render-blocking font request: fonts are self-hosted, two are preloaded, and
  `font-display: swap` is set.

The SEO score is 58, not 100, because two weighted audits fail by design:
`is-crawlable` (the meta robots tag and `robots.txt` both say no crawl) and
`canonical` (the final domain is unknown). The other SEO audits pass. Filling
`docs/LAUNCH.md` and flipping indexing is the launch gate; the expected result is
95 or higher.

## Deployment

The GitHub Pages preview is at https://kael-tombo.github.io/Jx/, deployed by
`.github/workflows/pages.yml` from the committed `public/` folder. The Pages run
succeeded on 2026-11-01. A live browser pass loaded all 13 pages with one H1,
loaded fonts, no console errors and no failed assets; the custom 404 returned
404 and loaded its stylesheet. The root, CSS, JavaScript and font responses were
200 with the expected content types.

GitHub Pages does not apply `_headers`. The live preview therefore has no CSP
header, no frame denial and no permissions policy. That remains a launch gap;
the local `_headers` file is not evidence of a live header.

## Accessibility

* axe ran with the WCAG 2.0, 2.1 and 2.2 A and AA tags on 13 pages in light and
  dark. Zero serious or critical violations. The suite asserts the filtered
  result is empty.
* 39 contrast pairs pass: 13 pairs in each of light, dark-by-system and
  dark-by-toggle. Text needs 4.5:1; UI borders need 3:1. Lowest text pair is
  4.88:1 (links on paper); lowest UI pair is 3.40:1.
* Playwright covers the skip link moving focus, keyboard activation of native
  FAQ controls, the theme toggle's `aria-pressed` and label, copy announcement,
  reduced motion, JavaScript off, the 404 page and the 375px layout.
* Reflow: all 13 pages at 320, 375, 640, 768 and 1280 pixels have no horizontal
  page overflow; all 13 also pass with the root font size at 200 percent at
  1280 pixels. Wide tables scroll inside their own labelled region.
* The terminal's static transcript is the accessible copy. The animation is
  `aria-hidden`, plays once, is disabled under `prefers-reduced-motion`, and has
  Pause and Show all controls.
* Print styles were rendered for `/vision` and `/roadmap` and inspected; both
  hide navigation and buttons and keep the footer notice.

Not verified: a screen-reader session, a real mobile device, Firefox or WebKit,
or an independent audit by a disabled user. The accessibility page says so.

## Content and truth

* 13 pages, each with `data-status` on `<main>` and a visible badge.
* `check-claims` enforces banned words, numbers with units, inline styles,
  inline scripts, terminal captions and the two footer notices.
* 21 placeholders remain: 13 canonical URLs, the OG URL and image URL, the owner,
  governance, accessibility and privacy contacts, hosting and retention, the
  contribution guide, the analytics decision, and the roadmap and vision facts.
  They are grouped in `docs/LAUNCH.md`.
* No testimonials, logos, star counts, user numbers, benchmarks, release dates,
  versions or download links are present. The only performance number is the
  five-minute target, listed in `scripts/allowed-numbers.json`.
* The landing, vision, FAQ, roadmap and glossary separate the workflow layer
  from runtime packaging: jx does not bundle a JRE and does not claim to fix
  the JRE's size; `jlink` is optional deployment packaging.
* The terminal caption stays visible in every state and says the output is
  illustrative and not a recording.
* Milestones say "Not released" rather than "Not started"; the reason is ADR
  0101.

## Metadata, privacy and security

* 13 unique titles and descriptions; every description is 155 characters or
  fewer; one H1 per page.
* The landing page has Open Graph and Twitter metadata, a 1200 by 630 generated
  image, and JSON-LD `SoftwareApplication` with no ratings and no offers.
* `robots.txt` disallows crawling, every page has `noindex`, and the sitemap is
  intentionally empty until the domain exists.
* `_headers` defines a self-only CSP, nosniff, frame denial, a referrer policy
  and a closed permissions policy. There are no third-party scripts, no cookies,
  no analytics and no signup form. `form-action 'none'` is deliberate until an
  endpoint exists. GitHub Pages ignores `_headers`, so these headers are not
  active on the deployed preview.
* GitHub Pages is the chosen host. It does not apply `_headers`, so the live
  preview has no CSP header. `_headers` remains useful only on a host that
  supports it. This is a deployment limitation, not a local check.

## Reproduce

```sh
npm install
npm run check                 # claims, todos, contrast, shared, links, HTML
npm run spellcheck
npm test                      # 41 Playwright tests, including 26 axe scans
CHROME_PATH="$(node tests/chrome-path.mjs)" npm run lighthouse
node scripts/check-todos.mjs --production   # expected to fail today
```

The rename drill copied `public/`, `design/`, `site.config.json` and
`scripts/rename.mjs` into a temporary Git repository, ran
`node scripts/rename.mjs demo`, then searched for the old word boundary in the
result. The copy was discarded.

## Deviations

1. Milestones say "Not released" and the docs command reference says "part of
   the V1 plan", not "not implemented". ADR 0101.
2. Two fonts are preloaded rather than one, because LCP is body text. ADR 0102.
3. The terminal has Pause and Show all, and reduced motion removes the animation
   entirely. The brief permits a pause control; this is a stricter fallback.
4. The roadmap strip and page are intentionally conservative and publish no
   milestone progress. The owner must decide whether to change that.

## Remaining limitations

* Chromium only. No cross-browser run.
* One Lighthouse run per URL, simulated mobile, not field data.
* No screen-reader or disabled-user audit.
* GitHub Pages cannot apply `_headers`, so the deployed preview has no CSP
  header, frame denial or permissions policy.
* SEO is a launch gate, not a pass.
* The design has been reviewed from screenshots at 375, 768 and 1280 in both
  themes, but not by an independent designer.

## Sign-off

| Item | State |
| --- | --- |
| Automated checks | Pass, except the intentional preview SEO gate |
| Manual visual review | Done from screenshots |
| Independent accessibility audit | Not done |
| Owner launch approval | Pending `docs/LAUNCH.md` |
