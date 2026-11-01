# 0101. Website truth, status and placeholders

Date: 2026-10-31
Status: Superseded by ADR 0103 for the `implemented` status and public milestone visibility; the placeholder, checker and indexing rules still apply.

## Context and problem

The brief's highest priority is truthfulness. The tool has no public release.
The repository, however, already contains work through M4, with M5 partly
prepared. The brief's own wording ("not started", "none is implemented") would
therefore be misleading in this repository.

Three things must hold at once: the site must never imply a release, every claim
must be traceable, and the language must not deny work that exists in the
repository.

## Decision

1. The public site is a **design preview**. It never says that jx is released,
   installable, fast or used by anyone.
2. Milestones and commands are labelled **Not released** or **part of the V1
   plan**. The brief's "Not started" and "not implemented" labels are not used,
   because the repository contains implementation work. This is a deliberate
   deviation from the brief, recorded here.
3. Public milestone progress is not published. The owner must decide whether to
   expose it. The conservative label "Not released" remains true either way.
4. The six statuses are idea, draft, proposed, accepted, implemented and shipped.
   `implemented` and `shipped` require `data-evidence` and `data-since`; with no
   release, neither may appear. The checker enforces this.
5. Unknown owner facts become `{{TODO: ...}}`. They block production with
   `node scripts/check-todos.mjs --production`, but not the preview.
6. Banned hype words are checked by `scripts/check-claims.mjs`. Numbers with
   units need an entry in `scripts/allowed-numbers.json`; the only performance
   number is the five-minute target.
7. Preview pages carry `noindex`, `robots.txt` disallows crawling and
   `site.config.json` has `indexable: false`. Launch flips all three together.

## Options considered

* Follow the brief literally. Rejected: it would state that nothing is built
  while the repository shows otherwise.
* Publish internal milestone progress. Not authorised by the brief, and it would
  require a release/status taxonomy the site does not have yet.
* Remove the roadmap. Rejected: the roadmap is a required part of the public
  design argument.

## Consequences

The preview is honest about the absence of a release without pretending that no
code exists. The roadmap is conservative, not exhaustive. The 21 remaining
placeholders are listed in `docs/LAUNCH.md`; production is blocked until the
owner supplies them. The preview's Lighthouse SEO score is 58 because indexing
is deliberately off and the canonical URL is a placeholder. That is a launch
gate, not a defect in the pages.
