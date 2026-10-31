# 0006. Fat jar merge rules

Date: 2026-10-31
Status: Accepted

## Context and problem

ADR 0005 chose a thin jar and `dist/lib` as the default package. Users still
want a single deployable file, but merging dependency jars raises questions
the thin layout avoids: which class wins when two artifacts ship the same
path, how `META-INF/services` files combine, what happens to signatures and
module descriptors, and whether multi-release entries remain valid.

A careless fat jar is worse than no fat jar: it can hide a version conflict,
break service discovery, or produce a jar whose signature check fails at
runtime.

## Decision

`jx package --fat` writes
`target/dist/<name>-<version>-all.jar` and no `lib/` tree. The manifest has
`Main-Class` when one is known and never a `Class-Path`.

Merge rules:

* **Project entries win.** Project classes and resources are written first;
  a dependency entry with the same path is dropped.
* **Dependency duplicates are first-wins in lock order.** The lock is already
  canonically sorted, so the result is deterministic. The command prints how
  many duplicate entries were dropped.
* **Service descriptors are merged.** Every `META-INF/services/<interface>`
  entry from the project and dependencies is parsed into provider names,
  de-duplicated in sorted order, and written once. Comments and blank lines
  are not preserved.
* **Manifests, signatures, `INDEX.LIST` and `module-info.class` are
  omitted.** Signed dependency jars cannot stay valid after merging, a stale
  index list is wrong, and merging module descriptors would require a real
  module composition pass.
* **Multi-release entries are omitted.** `META-INF/versions/...` entries are
  skipped and counted; the merged jar does not claim `Multi-Release: true`.
* **Unsafe entry names are refused.** Absolute paths, backslashes and `.`/
  `..` segments from dependency jars are not copied; a count is reported.
* The jar is written with deterministic sorted entries and deflate
  compression, like the thin jar.

The default remains `jx package` (thin), because moving an entire `dist`
tree is transparent about which artifact supplied each class.

## Options considered

* **Reject fat jars entirely** — safe, but fails the common "one file to
  deploy" need. Not chosen now that the merge rules are explicit.
* **Use a shading library with relocation** — would solve overlapping
  packages and service metadata, but relocation is a much larger feature and
  silently renaming packages breaks reflection and serialization.
* **Include multi-release entries and set `Multi-Release: true`** — only
  correct when every merged jar agrees on versioned layout; a single
  dependency with conflicting versioned classes would silently change
  behavior. Deferred until the merge can validate that.
* **Preserve dependency signature files** — guaranteed to fail verification
  after any merge. Never viable.

## Consequences

* A fat jar is self-contained for ordinary classpath applications and
  libraries, with no `dist/lib` requirement.
* Multi-release dependencies and JPMS module descriptors do not behave as
  they would from the original jars; use the thin layout when those matter.
* Duplicate class/resource paths are not reported individually, only as a
  count. The first artifact in lock order wins; use `jx tree` to inspect
  ordering.
* Fat jars are not signed and are not a replacement for M6's jlink images or
  a future relocation-aware shading mode.
