# Agent state

Last updated: 2026-11-01
Brief: `jx website: final master build prompt`, sections 3 and 11 in particular.

## Current state

The design-preview site is complete through W4. W5 is defined but blocked on owner
facts. Nothing on the site claims a release.

| Area | State |
| --- | --- |
| Pages | 13: landing, vision, roadmap, privacy, accessibility, 404, seven docs pages |
| Design system | Tokens, type scale, components, patterns page, print stylesheet |
| Framing | Cargo, uv and npm for Java; one run command across stacks is the destination, plain Java in V1 |
| JavaScript | Theme init, theme toggle, terminal, copy. All optional. |
| Checks | claims, todos, contrast, shared markup, links, HTML validator, spellcheck |
| Tests | 41 Playwright tests, axe on 13 pages in both themes |
| Assets | Self-hosted subset fonts, SVG favicon, generated 1200x630 OG image |
| CI | cargo jobs unchanged; a `site` job runs checks, tests, Lighthouse |
| Deployment | GitHub Pages preview is live at https://kael-tombo.github.io/Jx/ |
| Production | Blocked by 21 `{{TODO}}` placeholders |
| Final verification | Site CI job and Pages deploy pass; 41 browser tests, `check` and Lighthouse pass; SEO 58 is the intentional preview gate |

## Decisions

* The owner's framing (2026-11-01): Cargo, uv and npm for Java; one `jx run`
  across stacks is the destination, with plain Java in V1. The landing, vision,
  quickstart, Maven/Gradle guide, glossary and social image carry it.
* 0100 — reference-manual design system, token palette, typography.
* 0101 — design preview status, "Not released" instead of "Not started",
  placeholders and the claims checks.
* 0102 — self-hosted subsets, licences, two preloads, generated assets.

Earlier ADRs are the Rust project's; they are linked from `/docs/decisions`.

## Deliberate deviations from the brief

1. Milestones say **Not released**, not **Not started**, because this repository
   contains work through M4. Public progress is not published.
2. The docs command reference says commands are part of the **V1 plan**, not that
   they are unimplemented.
3. Two fonts are preloaded; the brief says one. Measured LCP required both.
4. The terminal shows output immediately when motion is reduced, and has Pause
   and Show all controls only when animation is available.

## Open questions for the owner

* Final name and domain. The repository URL is the `origin` remote; confirm
  that repository should be public.
* Whether to publish milestone progress.
* Final licence.
* GitHub Pages is enabled for the preview; confirm that its lack of
  `_headers`-based CSP is acceptable for launch, or choose a host that supports
  the strict header. The 404 still needs a live check.
* Signup endpoint and consent wording.
* Analytics: the default is none.
* Accessibility reviewer and launch approver.

## Next step

The preview is deployed. The owner fills `docs/LAUNCH.md`. Then flip indexing as
described there, choose whether the `_headers` CSP gap is acceptable, run the
production gate and re-measure. Do not add a signup form before an endpoint
exists.

## Commands

```sh
npm install
npm start                 # http://127.0.0.1:4173
npm run check             # includes the preview placeholder report
npm run check:production  # fails while any TODO remains
npm test                  # Playwright and axe
npm run spellcheck
npm run lighthouse        # port 4174, mobile
npm run og                # regenerate public/assets/og.png
```
