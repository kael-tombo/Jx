# 0015. Delegate scripts to JBang

Date: 2026-11-01
Status: Accepted

## Context and problem

ADR 0013 named JBang as the owner of scripts and single-file programs. V1
deliberately did not grow a script runner: a script has no `project.toml`, no
lockfile and no JDK pin, so treating one as a jx project would create a second,
weaker product model.

The adapter is still useful. A developer moving between a jx project and a
scratch `.java` file expects one front door, and JBang already resolves
`//DEPS`, caches scripts and installs its own JDK. Reimplementing any of that
would violate the composition rule.

## Decision

`jx script <file> [args...]` delegates to JBang:

* The command is opt-in. `jx new`, `jx build` and `jx run` never require
  JBang.
* JBang is resolved from `JX_JBANG` when set, otherwise as `jbang` on `PATH`.
  A missing tool stops before any download and names the tool, the install
  page and the override.
* `--offline` is passed as JBang's own `--offline` before the script path.
  Arguments after the script path go to the script, not to jx.
* When the script is inside a jx project, jx installs and exports that
  project's pinned JDK through `JAVA_HOME` and `PATH`. The manifest search
  starts at the script's directory, not the shell's working directory; an
  explicit `--manifest-path` overrides it. Without a manifest, jx leaves
  JBang's Java policy untouched; `jx script` does not download a JDK just to
  run a scratch file.
* JBang inherits stdout/stderr and its exit code is jx's exit code. jx prints
  one `delegating …` line unless `--quiet`.
* jx does not read JBang catalogs, aliases or `//DEPS`; those stay JBang's
  state. There is no `jx script` cache and no fallback runner.

### Tests

Unit tests cover argument construction, offline forwarding, `PATH` lookup,
`JX_JBANG` overrides, missing scripts and missing JBang. CLI tests run a fake
JBang executable, check the recorded arguments and verify that its exit code
is returned; they also prove that a missing tool is diagnosed before a JDK
download and that a project above the script supplies the pinned JDK. The
fake-tool tests are the CI gate, because CI does not install JBang.

A real run was performed on Windows x64 with JBang 0.143.0 and JDK 17:
`jx script hello.java alpha beta` printed `hello from jbang: alpha, beta` and
exited 0. The same run is recorded in
[KNOWN_LIMITATIONS.md](../KNOWN_LIMITATIONS.md).

## Options considered

* **Make `jx run hello.java` work** — overloads a project command with a
  manifest-less path, hides which tool owns dependency resolution, and makes
  `jx run` mean two different things. Rejected.
* **Bundle JBang or download it on first use** — contradicts rule 4 in ADR
  0013 and adds a second distribution channel to secure. Rejected.
* **Implement a small JBang-compatible runner** — reimplements `//DEPS`,
  catalogs and script caching with worse compatibility. Rejected.
* **Always use the installed system Java** — ignores a project's JDK pin when
  the script lives beside a `project.toml`. Rejected; the adapter uses the pin
  when it exists and otherwise leaves JBang alone.

## Consequences

* Scripts keep JBang's dependency graph, cache and exit behaviour; jx supplies
  discovery and, when available, the same JDK as the project.
* The script path is still separate from the reproducible project path. A
  scratch script cannot use `jx.lock` or `--locked`, and this is documented
  rather than implied.
* JBang must be installed. The error is actionable, and the five-minute path
  remains free of external tools.
* A real JBang run is a manual verification item; CI uses a fake executable
  under ADR 0013's adapter rules.
