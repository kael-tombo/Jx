# 0018. Framework packaging is a layout, not a plugin build

Date: 2026-11-03
Status: Accepted

## Context and problem

Spring Boot, Micronaut, Vert.x and Quarkus applications are plain JVM
programs, but each ecosystem packages them differently: Boot rewrites a jar
into `BOOT-INF/classes` plus `BOOT-INF/lib` behind a loader, Micronaut's
shadow jar merges everything, Vert.x applications are usually shaded, and
Quarkus generates `quarkus-app/` while augmenting. A jx project building a
Boot app got a thin jar or a merged fat jar that `java -jar` could not start
the way the framework expects.

jx deliberately does not run Maven or Gradle plugins (ADR 0012), so
"support the framework" cannot mean invoking `spring-boot-maven-plugin`.
Scaffolding and running were already covered: `jx run` starts any main class,
and imports bring the dependency graph in.

## Decision

`project.toml` gains one table:

```toml
[package]
framework = "spring-boot"
```

`framework` is one of `plain` (the default), `spring-boot`, `micronaut`,
`vertx`, `quarkus` or `jakarta-ee`. It selects only the layout `jx package`
writes:

* `spring-boot` writes the Boot 3.2+ executable layout: loader classes at the
  archive root, project classes under `BOOT-INF/classes`, runtime jars copied
  (not merged) under `BOOT-INF/lib`, and `Main-Class` pointing at
  `org.springframework.boot.loader.launch.JarLauncher` with `Start-Class` in
  the manifest. `--fat` is refused because the layout is already one jar.
  The loader is not transitive from any starter, so the project must declare
  `org.springframework.boot:spring-boot-loader`; a missing loader fails
  before compilation with the exact `jx add` coordinate, and a pre-3.2 loader
  is refused before `target/dist` is cleared.
* `micronaut` and `vertx` select the merged fat jar, the shape their
  shadow/fat-jar builds produce.
* `quarkus` and `jakarta-ee` are recognized and refused with the build step
  that owns the layout. Quarkus generates its runnable tree while augmenting;
  a Jakarta EE WAR needs container-provided APIs and jx has no `provided`
  scope, so a WAR would bundle the container's own classes.
* `plain` keeps today's behavior: a thin `dist/` or an explicit `--fat` jar.

Dependency resolution, compilation, `[processors]` and `[run] main` are
unchanged by the setting.

## Options considered

* **Run the framework plugins through a delegated Maven/Gradle build** — the
  project would need a second build file, and jx already refuses plugin
  execution (ADR 0012); it also would not help projects that only have
  `project.toml`. Rejected.
* **Merge every framework into the existing fat jar** — Boot's loader expects
  nested jars and its own manifest headers, so a merged jar may start but
  loses the framework's class loading, `loader.path` and layer metadata.
  Rejected.
* **Generate a WAR for Jakarta EE** — jx has no `provided` scope, so the WAR
  would either duplicate the container's Jakarta APIs or need a scope the
  manifest cannot express. Refused with a specific message instead.
* **Add a Spring AI adapter** — Spring AI is a set of Boot starters, not a
  packaging layout; a Boot project that depends on it is already covered.
  Rejected.
* **Scaffold projects per framework** — a separate feature with its own
  templates and maintenance; this decision is about packaging existing
  sources.

## Consequences

* One manifest key replaces plugin configuration for the layouts jx can write
  faithfully; unsupported layouts fail with the owning command instead of
  producing a jar that starts incorrectly.
* Boot projects must add the loader dependency themselves; the diagnostic
  names the coordinate at the app's Boot version.
* Quarkus and Jakarta EE projects keep their external build. jx still
  resolves and compiles them, but `jx package` is not their packaging step.
* A future adapter (Quarkus fast-jar, Boot layers) needs its own layout
  writer and tests; the enum leaves room for that.
