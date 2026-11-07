# Agent state

Last updated: 2026-11-07
Brief: `jx website: final master build prompt`, sections 3 and 11 in particular.

## Current state

The site is a V1 preview: it says V1 is implemented and release-pending, and
claims no tagged release. W5 (launch) is still blocked on owner facts.

| Area | State |
| --- | --- |
| Pages | 13: landing, vision, roadmap, privacy, accessibility, 404, seven docs pages |
| Landing | Inverted front-door hero with a toolchain rail; numbered chapters; a full-width five-property band; a combined stack/interop split; full architecture map with layer notes and a reused/adapter split; a reuse rule naming JBang and the OS package managers; three plan horizons under "The plan, in public" |
| Design system | Tokens, editorial front-door layer (`editorial.css`), type scale, components, architecture map, plan horizons, patterns page, print stylesheet |
| Framing | Cargo, uv and npm for Java; one run command across stacks is the destination, plain Java in V1; a workflow tool, not a runtime or a bundled JRE; V1 implemented, release pending |
| JavaScript | Theme init, theme toggle, terminal, copy. All optional. |
| Checks | claims, todos, contrast, shared markup, links, HTML validator, spellcheck |
| Tests | 41 Playwright tests, axe on 13 pages in both themes |
| Assets | Self-hosted subset fonts, SVG favicon, generated 1200x630 OG image |
| CI | cargo jobs unchanged; a `site` job runs checks, tests, Lighthouse |
| Deployment | GitHub Pages preview is live at https://kael-tombo.github.io/Jx/; the architectural and editorial redesign is local, not yet published |
| Production | Blocked by 21 `{{TODO}}` placeholders |
| Final verification | 41 browser tests, `check` and spellcheck pass on the redesign; 26 axe scans pass in light and dark; Lighthouse measures 99–100 performance, 100 accessibility and 100 best practices, with SEO 58 as the intentional preview gate; all 13 pages pass reflow at 320–1280 and at 200% text; landing, vision and roadmap print views were inspected |

## Decisions

* The owner's framing (2026-11-01): Cargo, uv and npm for Java; one `jx run`
  across stacks is the destination, with plain Java in V1. The landing, vision,
  quickstart, Maven/Gradle guide, glossary and social image carry it.
* The owner's framing from a public exchange (2026-11-01): Java needs a platform
  tool, not a larger JRE. jx is the workflow layer, independent and not a
  runtime; `jlink` is optional deployment packaging, not the thesis. The
  landing, vision, FAQ, roadmap and glossary carry the distinction.
* The owner's framing (2026-11-01): jx sits on top of existing tools such as
  JBang and Chocolatey to avoid reimplementing them. The architecture and plan
  state the boundary: jx owns the manifest, lockfile, command path and conflict
  policy; the tools keep their jobs. JBang is wired through `jx script`,
  `jot.toml` import is wired, and the release renderer emits OS package
  manifests; submission and installation stay with those tools.
* 0100 — reference-manual design system, token palette, typography.
* 0101 — design preview status, "Not released" instead of "Not started",
  placeholders and the claims checks.
* 0102 — self-hosted subsets, licences, two preloads, generated assets.
* The 2026-11-01 landing redesign shows the complete engine map under
  "How it works" and the V1, after-V1 and boundary horizons under "The plan, in
  public", with M0–M8 below. The command reference now includes `jx package` and
  `jx image`.
* 0103 — the site may mark V1 behaviour `implemented` with evidence, while
  `shipped` stays reserved for a tag. The noindex preview and owner gates remain;
  milestone state is published without dates, percentages or gate claims.
* The 2026-11-07 redesign moves the front door from a warm reference manual to
  an editorial-technical surface: an inverted hero, square controls, numbered
  chapters, a wide five-property band, a merged stack/interop split and a dark
  follow band. `editorial.css` layers over the existing components, so docs,
  code blocks and content claims stay intact.

Earlier ADRs are the Rust project's; they are linked from `/docs/decisions`.

## Deliberate deviations from the brief

1. Milestones say **Implemented**, not **Not released**, because the 1.0.0
   source tree contains M0–M8. The decision gates stay open or unverified.
2. The docs command reference marks commands **implemented**; flags and output
   may still change before the tag.
3. Two fonts are preloaded; the brief says one. Measured LCP required both.
4. The terminal shows output immediately when motion is reduced, and has Pause
   and Show all controls only when animation is available.

## Open questions for the owner

* Final name and domain. The repository URL is the `origin` remote; confirm
  that repository should be public.
* Final licence confirmation for the site footer. The repository already ships
  Apache-2.0 OR MIT and the footer now states it without "proposed".
* GitHub Pages is enabled for the preview; confirm that its lack of
  `_headers`-based CSP is acceptable for launch, or choose a host that supports
  the strict header. The 404 still needs a live check.
* Signup endpoint and consent wording.
* Analytics: the default is none.
* Accessibility reviewer and launch approver.

## Next step

The architectural and editorial redesign is local and ready to publish.
Publish it, then have the owner fill `docs/LAUNCH.md`. Flip indexing as described
there, choose whether the `_headers` CSP gap is acceptable, run the production
gate and re-measure. Do not add a signup form before an endpoint exists. After a
`v*` tag exists, change `implemented` to `shipped` and fill the install step.

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
