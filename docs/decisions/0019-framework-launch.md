# 0019. Framework launch rules stay faithful to the runtime

Date: 2026-11-07
Status: accepted

## Context and problem

ADR 0018 made `[package] framework` select a jar layout without running a
framework plugin. Packaging alone is not enough to start every framework
faithfully: a Vert.x verticle is not a static main, JavaFX cannot launch its
`Application` subclass from the classpath, Spring Boot 4 raised the JDK
baseline, Micronaut generates bean metadata through an annotation processor,
and Quarkus cannot run before augmentation.

## Decision

Framework support remains composition, not plugin execution. jx applies the
runtime's own launch rule from the locked dependencies:

* `vertx` runs a hand-written static main directly. A verticle needs
  `[run] main`; Vert.x 5 uses `io.vertx:vertx-launcher-application` and
  records `Main-Verticle`, while Vert.x 3/4 uses `io.vertx.core.Launcher`
  with its `run` command. Launcher-based entries cannot be exported.
* `javafx` runs a separate launcher class on the classpath and refuses an
  `Application` subclass or more than one platform classifier for the same
  artifact.
* `spring-boot` keeps the Boot 3.2-and-later layout for Boot 4 and requires
  JDK 17 or newer when the locked loader is Boot 4 or later.
* `micronaut` warns when `[processors]` does not contain
  `io.micronaut:micronaut-inject-java`.
* `quarkus` is refused by `jx run` because plain compiled classes are not a
  runnable Quarkus application.
* `jx image` refuses Boot, Micronaut and Vert.x executables because it builds
  a thin `target/dist` plus a jlinked runtime.

## Options considered

* **Infer or download missing launchers** — jx would invent coordinates and
  versions outside the locked graph. Rejected.
* **Merge launcher metadata into every fat jar** — a `Main-Verticle` header
  without the Vert.x launcher, or a classpath-launched JavaFX application,
  would start incorrectly. Rejected.
* **Run framework dev/build steps for Quarkus or export a launcher POM** —
  that would make jx own plugin behavior ADR 0012 deliberately refuses.
  Rejected.

## Consequences

* `jx run`, `jx package`, `jx image` and `jx export` fail with the owning
  coordinate or build step instead of producing an entry point the runtime
  cannot honor.
* Framework diagnostics name exact locked coordinates and manifest keys, so
  the fix stays in `project.toml` and Maven Central.
