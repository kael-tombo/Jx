# 0103. Website status after V1 implementation

Date: 2026-11-01
Status: Accepted

## Context and problem

ADR 0101 reserved `implemented` and `shipped` until a public release existed.
That was correct while the site described a plan. The 1.0.0 source tree now
contains the V1 command set, and the site says "planned" or "not released" in
places where the work exists. Both labels are now misleading: one denies the
implementation, the other would imply a publication that has not happened.

The site still has no tagged release, no final domain, and 21 owner-only
placeholders. It must describe the implementation without turning the preview
into a release announcement.

## Decision

1. A page may use `implemented` for behaviour that exists in the 1.0.0 source
   tree. The status carries `data-evidence` and `data-since="1.0.0"`.
   `scripts/check-claims.mjs` continues to require both attributes.
2. `shipped` remains reserved for a tagged public release. No page uses it
   before a `v*` tag and its release assets exist.
3. Product status reads **V1 implemented · release pending**. An install step
   says to build from source until the release archives and installers are
   published from the version tag.
4. An implemented status does not assert that a milestone gate has passed, that
   a third party reviewed the work, or that a release exists. The roadmap keeps
   its gates open or unverified where that is the honest state.
5. The site stays a `noindex` preview. The 21 `{{TODO}}` placeholders and the
   production gate in `docs/LAUNCH.md` are unchanged.
6. The 0101 rule that public milestone progress is not published is superseded:
   the site may say that V1 is implemented, but it does not publish dates,
   percentages or unverified gate results.

## Consequences

The site can describe the command set, interop modes and runtime image without
pretending they are only planned. A tagged release remains a separate event:
publishing v1.0.0 changes `implemented` to `shipped` and fills the install
section, but does not change any other claim. The preview stays out of search
results until the owner closes the launch gates.

## Supersedes

* ADR 0101, points 1–3 and 5, as they apply to the `implemented` status and
  public milestone visibility. The placeholder, checker and indexing rules in
  ADR 0101 still apply.
