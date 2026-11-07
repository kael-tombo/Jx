# 0020. Import sets `[package] framework` from dependency groups

Date: 2026-11-07
Status: accepted

## Context and problem

Every `jx import` (Maven, Gradle, jot) produced a `plain` manifest, even for
an obvious Spring Boot, Quarkus, Micronaut, Vert.x, JavaFX or Jakarta EE
module. The framework rules from ADR 0018 and ADR 0019 only engage through
`[package] framework`, so an imported application silently lost them: a
Quarkus app would launch unaugmented bytecode instead of being refused,
and Boot, Vert.x and JavaFX apps lost their packaging and launch entries
until the user discovered the framework key by hand.

## Decision

Import detects the framework from dependency group ids and writes it into
the generated manifest, with a note naming the marker:

* `org.springframework.boot`, `io.quarkus`, `io.micronaut`, `io.vertx`,
  `org.openjfx` and `jakarta.platform`, in that precedence order; the first
  group present wins.
* Main dependencies and annotation processors count (a Micronaut processor
  implies a Micronaut app). Test-only dependencies never trigger detection:
  a test utility such as a Vert.x JUnit helper must not turn a plain
  library into a framework app.
* `jakarta.platform` alone marks Jakarta EE. Narrower `jakarta.*` groups
  (servlet, inject, persistence) are plain dependencies any app may use.
* Plugins are never consulted: the Gradle Spring Boot plugin still only
  contributes its BOM as a platform entry, and that entry's
  `org.springframework.boot` group is what the detector sees. Removing
  `[package] framework` restores a plain jar when the guess is wrong.

## Options considered

* **Leave every import `plain`** — preserves the old opt-in stance but keeps
  the silent behavior loss above, including the misleading Quarkus launch.
  Rejected.
* **Detect from plugins instead of (or as well as) groups** — plugin
  presence is the more direct signal, but reading plugin semantics
  contradicts ADR 0012's refusal to own plugin behavior, and group ids are
  already on the table the converter walks. Rejected.
* **Refuse to guess and demand a flag** — an extra `--framework` would keep
  imports explicit, but every information needed is already in the
  dependencies, and the note plus the one-line removal keep the guess
  reviewable in the existing import-notes workflow. Rejected.

## Consequences

* An imported framework app locks, runs, packages and refuses exactly like
  a hand-written one; the converter's output still passes the same
  `jx-manifest` validation before anything is written.
* A wrong guess costs one deleted `[package]` section, documented in the
  import notes and in `KNOWN_LIMITATIONS`.
* Framework detection lives in `jx-interop` so Maven, Gradle and jot share
  one marker table. The hand-written site makes no claim about import
  inferred frameworks, so no site page contradicts the new notes.
