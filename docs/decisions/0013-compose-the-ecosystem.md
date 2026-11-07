# 0013. Compose the ecosystem; do not reimplement it

Date: 2026-11-01
Status: Accepted

## Context and problem

jx is a workflow layer for Java, not a replacement for the Java ecosystem.
Every tool around a project already has an owner: the JDK compiles and links,
Maven Central stores artifacts, Maven and Gradle build, JBang runs scripts,
and Chocolatey, Homebrew, Scoop or the platform installer put tools on the
machine.

The temptation is to absorb each of them. A package manager is convenient; a
script runner is a small feature; a build-tool evaluator would make import
exact; bundling a JRE would make `jx run` standalone. Each absorption would
duplicate a mature tool, add platform-specific surface, and create a second
source of truth. It would also make the five-minute path heavier, not
smoother.

The product claim is the path across those tools: one manifest, one lockfile,
one command surface, and an exit. jx should be the layer that knows which tool
owns the work and calls it well.

## Decision

### Ownership

| Concern | Owner | jx's role |
| --- | --- | --- |
| Project state | jx | `project.toml`, `jx.lock`, resolution policy, diagnostics |
| Artifact bytes and metadata | Maven Central (or a configured repository) | checksum-verified client, content cache, lockfile pins |
| Compilation, tests, linking | the pinned JDK (`javac`, JUnit, `jdeps`, `jlink`) | orchestrate, fingerprint, report; never reimplement the compiler or launcher |
| Build lifecycle | jx, or Maven/Gradle when `[build] engine` delegates | own the jx path, or plan and run the external tool with the pinned JDK (ADR 0012) |
| JDK installation | Eclipse Temurin over the Adoptium API today; the platform's installer in general | resolve, verify and install into jx's own store; never modify a system Java installation |
| OS-level installation | Chocolatey, Homebrew, Scoop, winget and the platform installer | generate release manifests where useful; do not wrap or replace the managers (ADR 0017) |
| Scripts and single-file programs | JBang | delegate through `jx script`; do not grow a jx script runner or absorb `//DEPS` (ADR 0015) |
| Runtime | the JDK | optional `jlink` image packaging (ADR 0009); jx is not a runtime |

The resolver is the deliberate exception that proves the rule. jx does not run
Maven to resolve dependencies, because a reproducible lockfile built on
Maven Central metadata is the product. It also does not emulate Maven's full
POM model: unsupported features are diagnosed, not guessed, and the exit is
`jx export maven`.

### Adapter rules

An external tool may be integrated only when all of these hold:

1. **It is opt-in.** The five-minute path (`jx new`, `jx run` on plain Java)
   never requires Maven, Gradle, JBang or an OS package manager.
2. **Absence is diagnosed.** The error names the tool, what jx wanted from it,
   and how to install or configure it; jx never silently falls back to a
   partial reimplementation.
3. **The tool keeps its output and exit code.** jx adds one line of context
   (which tool, which goal, which JDK), not a translation layer that hides
   the real build.
4. **Nothing is bundled or downloaded behind the user's back.** A wrapper may
   download its own distribution, but jx does not vendor another build tool.
5. **It is tested without the tool and verified with it.** Fake adapters cover
   arguments, environment and exit codes in CI; at least one real run is
   recorded in the ADR or the limitations file.
6. **It does not duplicate a JDK tool that already works.** `javac`, `java`,
   `jar`, `jdeps` and `jlink` are called directly; jx does not grow its own
   compiler, linker or class-file reader.

### What jx will not build

* A package manager, an OS installer or a JDK version manager for other
  people's installations.
* A JVM, a bundled JRE or a general runtime.
* A Maven/Gradle script evaluator. Import reads declarations; delegation runs
  the real tool; neither pretends to execute arbitrary build scripts.
* A JBang-compatible script runner or dependency catalog.
* A Maven Central proxy or a second artifact repository.
* A framework hot-reload or deployment system.

## Options considered

* **Absorb JBang for scripts in V1** — a script path has no manifest, lockfile
  or JDK pin, so it would introduce a second, weaker product model. JBang is
  the right owner; an optional adapter can come after V1.
* **Install JDKs through Chocolatey/Homebrew/Scoop** — makes jx a package
  manager, requires platform-specific packaging and admin rights, and gives up
  the checksum-verified, reproducible install. Users keep their installer;
  jx keeps its store.
* **Run Maven/Gradle to resolve dependencies** — exact for Maven-shaped
  projects, but it makes jx require the tool it is meant to replace, and it
  loses jx's lockfile policy. Import plus delegation covers that need.
* **Bundle a JRE for `jx run`** — contradicts the workflow-layer claim and
  produces stale runtimes; the pinned JDK install is already one command.
* **Evaluate Gradle scripts with a Groovy/Kotlin interpreter** — more grammar
  than the problem needs and still unable to evaluate arbitrary values; the
  conservative reader (ADR 0011) is the honest subset.

## Consequences

* Maven/Gradle delegation (ADR 0012) is the first implemented adapter and the
  template for later ones: explicit selection, diagnosed absence, tool-owned
  exit code, fake-tested in CI.
* The post-V1 adapters follow that shape: JBang scripts (`jx script`, ADR
  0015), `jot.toml` import (ADR 0016), and release-generated OS package
  manifests (ADR 0017). The winget manifests were also checked with Microsoft's
  validator; the other package managers keep their own submission review.
* A future adapter needs a new ADR (or an amendment here) describing what the
  tool owns, how absence is diagnosed, and how it is tested. Adding one is a
  deliberate product decision, not a refactor.
* jx stays a single binary with no runtime dependencies, and the happy path
  stays small. The cost is that some workflows need the other tool installed;
  that is the intended trade, not a gap to paper over with a partial clone.

## Amendment (2026-11-01)

The post-V1 adapter work is implemented under the ownership table and rules
above. `jx script` delegates to JBang (ADR 0015); `jx import` reads Java-only
`jot.toml` projects (ADR 0016); and the release workflow generates Chocolatey,
Homebrew, Scoop and winget manifests while leaving installation and upgrades
to those package managers (ADR 0017). jx still does not install a JDK through
an OS package manager, bundle a runtime, or publish itself to a package feed.
