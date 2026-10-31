# 0102. Website fonts and assets

Date: 2026-10-31
Status: accepted

## Context and problem

The brief asks for a serif display face, a sans for body text and a monospace for
code, self-hosted as WOFF2 under an open licence. The first implementation used
the full Latin subsets of Newsreader, IBM Plex Sans and IBM Plex Mono. That was
178 KB of fonts and produced a 2.3 s LCP on throttled mobile, above the brief's
2.0 s target. IBM Plex also carries the reserved name "Plex", so an ad-hoc
subset would need a renaming step to comply with the OFL.

## Decision

Use three OFL families with no reserved font name in their copyright line:

| Role | Family | File | Bytes |
| --- | --- | --- | --- |
| Display | Newsreader 400 | `newsreader.woff2` | 14,112 |
| Body | Public Sans 400–600 | `public-sans.woff2` | 16,768 |
| Mono | DM Mono 400 | `dm-mono.woff2` | 10,340 |

The files are text subsets generated through the Google Fonts CSS API with
`text=` covering printable ASCII plus the punctuation the pages actually use
(`· × – — ‘ ’ “ ” → − ≥`). The full OFL text for each family is kept beside the
fonts. No font is fetched from a third party at runtime.

Two fonts are preloaded (Newsreader and Public Sans), a deliberate deviation
from the brief's "preload one". LCP is body text, so preloading only the display
face did not help. After the swap: LCP 1.4 s, CLS 0, Lighthouse mobile
performance 100.

The favicon is hand-written SVG. The 1200 by 630 Open Graph image is generated
from `design/og.html` by `scripts/make-og.mjs` (`npm run og`), using the local
Playwright browser. No stock art or third-party image is used.

## Options considered

* Keep the full fonts. Rejected: LCP 2.3 s and 178 KB.
* Use system fonts only. Rejected: it would lose the editorial display face that
  carries the identity.
* Subset IBM Plex. Rejected without renaming: the OFL reserved name makes an
  unrenamed subset a licence problem.
* Use `font-display: optional` to hide the late swap. Rejected: the brief asks
  for `swap`, and slow connections would lose the chosen face.

## Consequences

Regenerate the subsets if the visible copy gains characters outside the set
above; otherwise those characters fall back to the system stack. The Google
Fonts request and the chosen files are recorded here so the result can be
reproduced. The assets add 41 KB in total, the landing JavaScript is 2.1 KB
gzipped, the whole landing page is 65 KiB over the wire, and the site remains
fully local.
