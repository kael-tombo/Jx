# 0012. Delegated builds (Maven/Gradle compatibility mode)

Date: 2026-11-01
Status: Accepted

## Context and problem

Some Maven and Gradle projects cannot be represented by `project.toml` at all:
custom source sets, shading, annotation processing, multi-module builds, or
plugins that compute their own classpaths. `jx import` refuses what it cannot
translate (ADRs 0010 and 0011), and `jx export` only goes the other way. Those
projects still want what jx is good at — a pinned, downloaded JDK and one
command surface — without giving up their build.

Running the external build is not the same as importing it. Maven and Gradle
own dependency resolution, compilation and packaging in such a project; jx
must not pretend its manifest describes the result. The compatibility mode
therefore has to be explicit about who owns what.

## Decision

### Selecting an engine

`project.toml` gains a strict, optional table:

```toml
[build]
engine = "maven"   # "jx" (default), "maven" or "gradle"
```

Unknown keys are rejected with a did-you-mean suggestion, and an unknown
engine is rejected with the allowed set. The global `--engine jx|maven|gradle`
flag overrides the table for one invocation, so a project can delegate
persistently and still ask jx for its own view when needed.

Delegation always requires a `project.toml`: it is where the JDK pin lives.
There is no manifest-less "jx on a Maven repo" mode in this milestone; that
would need a separate JDK policy. `jx import` remains the entry point.

### What is delegated

Only the lifecycle commands with a direct equivalent are delegated:

| jx | Maven | Gradle |
| --- | --- | --- |
| `jx build` | `compile` | `classes` |
| `jx test` | `test` | `test` |
| `jx clean` | `clean` | `clean` |

The tool is the project's wrapper when one exists (`mvnw`/`mvnw.cmd`,
`gradlew`/`gradlew.bat`), otherwise `mvn` or `gradle` from `PATH`. Every lookup
is diagnosed: a missing build file or executable stops before any JDK download
and names the file, the tool and the remedy.

jx installs the pinned JDK, runs the tool in the manifest's directory with
`JAVA_HOME` set to the install and `<jdk>/bin` first on `PATH`, and inherits
the tool's stdout/stderr. The tool's exit code is jx's exit code. `--offline`
is forwarded as Maven `-o` or Gradle `--offline`. jx prints one `delegating
to …` line unless `--quiet`.

### What is refused

The external build owns dependency resolution, compilation and packaging, so
jx refuses commands that would edit or describe a graph the build does not
use: `add`, `remove`, `lock`, `fetch`, `tree`, `why`, `run`, `package`,
`image`, `export maven` and `export gradle`. The error names the command, the
engine and the two escapes: edit the external build, pass `--engine jx` for
this invocation, or remove `[build] engine`. The export refusal also says it
would write a second build beside the one being delegated to.

`--locked` is refused for delegated `build` and `test`: jx.lock is not read,
so the flag would claim a guarantee jx cannot make. `clean` does not resolve
anything and ignores it, as before.

`jx clean` with the jx engine now cleans the manifest project's `target/`,
even when invoked from a subdirectory. A directory with no manifest keeps the
old behaviour and removes `target/` below the current directory.

## Options considered

* **Delegate only when no `project.toml` exists** — the smoothest first run,
  but it gives jx no JDK pin, no engine choice and no place to record intent.
  It also makes `jx build` silently run a different build tool depending on
  files in the directory. Deferred to its own decision.
* **Delegate `run` and `package` too** — Maven has no built-in run goal
  (`exec:java` needs a plugin and a main class), and the two tools produce
  different package layouts. A mapping would be a guess. Refusing with the
  `--engine jx` escape is honest.
* **Keep dependency commands available and warn** — `jx add` would edit
  `project.toml` while the delegated build ignores the edit; `jx tree` would
  show a graph that is not the build's. Warnings are too easy to miss for an
  operation that silently has no effect.
* **Read the external build's graph for `tree`/`why`** — requires invoking
  Maven's dependency plugin or Gradle with a custom init script, plus parsing
  tool-specific output. Valuable, but a separate milestone.
* **Copy the manifest's dependencies into the delegated build** — would make
  jx the source of truth again and break plugin-managed builds. Out of scope.
* **Spawn through a shell string** — quoting project paths and user data is a
  security and portability hazard. Commands are built with `Command` and
  fixed arguments; Windows only uses `cmd /C` to execute `.cmd`/`.bat`
  wrappers, which the OS cannot launch directly.

## Consequences

* Delegation is the first implemented instance of the composition rule in
  ADR 0013: the external build keeps its job, jx supplies the JDK and the
  command surface, and absence of the tool is diagnosed rather than emulated.
* A project whose build jx cannot reproduce can still use `jx build`,
  `jx test` and `jx clean` with jx's pinned JDK, while Maven or Gradle keeps
  full control of dependencies, plugins and packaging.
* Reproducibility is the external build's. jx.lock and `jx fetch` are not
  involved in delegated commands, and the build tool may use its own cache
  and network policy. The documentation says so instead of implying otherwise.
* The build tool must be installed or wrapped. The first wrapper run may
  download the distribution; `--offline` does not control that download,
  only the tool's own dependency access.
* Windows executes `.cmd`/`.bat` wrappers through `cmd /C`; a Unix wrapper
  must be executable. The spawn error names the wrapper and suggests
  `chmod +x`.
* Merging import and delegation in one project is allowed but the manifest's
  dependency table is inactive while the engine is delegated. `--engine jx`
  exposes it; `jx tree` says why it is refused otherwise.
* M7 is complete: Maven and Gradle import, Maven and Gradle export, and
  delegated lifecycle commands. M8 (release hardening, security checklist,
  install script) is next.
* Tests use a fake installed JDK and fake wrappers that record `JAVA_HOME`
  and arguments, so CI needs neither Maven nor Gradle. Delegated build, test
  and clean were also run manually against Gradle 8.14.3 with a real JDK 17.
  Maven was not installed in the development environment, so a real Maven
  delegation run remains part of M8 verification.
