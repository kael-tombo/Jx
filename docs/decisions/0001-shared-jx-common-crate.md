# 0001. A shared jx-common crate for errors, config, paths and cache

Date: 2026-10-30
Status: accepted

## Context and problem

The brief's workspace layout (section 11) names 11 crates but does not name a
home for three cross-cutting M0 deliverables it requires: an error-rendering
framework ("every error says what happened, where, and what to try next"),
config handling around `JX_HOME`, and the content-addressed cache.

Without a shared home, each crate would pick its own error type and path
helpers, which the brief explicitly forbids by demanding one error format and
one cache.

## Decision

Add one shared library crate, `jx-common`, holding:

* `error::JxError` — kind, message, location, hint, cause chain, and the
  stable exit-code mapping (docs/exit-codes.md).
* `color` — the `--color` / `NO_COLOR` / `TERM=dumb` decision.
* `paths` — `JX_HOME` resolution (default `~/.jx`).
* `config` — `<JX_HOME>/config.toml` parsing (repositories/mirrors).
* `cache` — the sha256 content-addressed artifact cache under
  `<JX_HOME>/cache/artifacts/content/`, verified on every read.

## Options considered

* Scatter these modules into whichever crate needs them first — rejected:
  duplicates logic and invites inconsistent error text for the same classes
  of failure.
* One crate per concern (jx-error, jx-cache, …) — rejected for now: heavier
  workspace for cross-cutting code whose API is still settling; can be split
  later without breaking dependents.

## Consequences

The workspace deviates from section 11's literal list of crates by one added
crate; it adds no new behaviour beyond M0 scope. `JxError` becomes the single
error type across the workspace, so future crates must not invent their own.
If config grows a second consumer with incompatible needs, revisit the split.
