# 0100. Website design system

Date: 2026-10-31
Status: accepted

## Context and problem

The brief pins a direction: a well-made reference manual, warm paper, ink text,
one ember accent, real typography, diagrams instead of decoration. It rejects
stock imagery, gradient blobs, laptop photographs and Java logos. The site must
work without a framework, bundler or build step, and the same tokens must hold
up in light and dark themes.

## Decision

Use a small token set in `public/css/tokens.css`:

* Paper `#faf8f4`, ink `#14171c`, muted `#5b616b`, ember `#c2410c`, teal
  `#0b7a6f`, with dark equivalents.
* Newsreader for display, Public Sans for body, DM Mono for commands and the
  wordmark. One display weight only.
* A 72rem content width, a 12-column rhythm, an 8px spacing scale, 6px radii
  and hairline rules. No shadows, no cards with elevation, no stock images.
* Flat ruled lists instead of nested cards; the terminal block is the one
  deliberate visual set piece.
* Status is always a word plus a shape, never colour alone.

The diagram is inline SVG with a wide and a narrow variant, themed with CSS
variables. The terminal and tables are the only complex components.

## Options considered

* A CSS framework or component library. Rejected: a dependency and a build
  step for a site this size, and it would blur the identity.
* A dark-first palette. Rejected: the brief asks for warm paper first and a dark
  alternative.
* A second display weight. Rejected after measuring the font payload; one
  weight keeps the page fast and the hierarchy still reads.
* Separate rounded cards for the five properties. Rejected: ruled rows and a
  single spanning cell look more like a manual and less like a dashboard.

## Consequences

The design depends on typography, spacing and the diagram, not decoration. The
contrast script checks every text and UI pair in both themes. A decorative
hairline is not treated as a UI boundary; `--control-line` is the 3:1 border.
The terminal is the only motion, and it is off under reduced motion. The font
decision and its performance measurements are in 0102.
