# jx exit codes

Every jx command exits with a code a script can rely on.

| Code | Meaning | Categories that produce it |
| --- | --- | --- |
| 0 | Success. Warnings do not change the exit code (`jx doctor` exits 1 only when a check cannot run). | — |
| 1 | A diagnosed failure (io, network, toolchain, manifest, lock, resolve, build). | error kinds other than `usage` |
| 2 | Bad usage: unknown flag, unparseable arguments, or a command jx does not yet implement; `jx test` also uses 2 when no tests are found. | `usage`; JUnit console no-tests |

Notes:

* `jx doctor` exits 0 when it completes, even with warnings; exits 1 only when
  a check itself cannot complete.
* `jx test` propagates the JUnit Platform console launcher's exit code:
  0 when every test passes, 1 when the suite has failures, and 2 when no
  tests are found (`--fail-if-no-tests`); a `--filter` that selects nothing
  is an empty run and exits 2 for the same reason. jx's own failures (missing
  JDK, compile error, missing launcher) use the normal 1/2 codes above.
* The global `--locked` flag makes `jx lock`, `jx add`, `jx remove` and the
  build fast path fail with a diagnosed `lock` error when the lockfile is
  missing or would change. `jx update` refuses it outright: its job is to
  write jx.lock, while `jx lock --locked` is the read-only check. That is
  its contract, not an internal failure; the exit code is still 1 like any
  other diagnosed error. It is refused for delegated `build`/`test` because
  those commands do not read jx.lock.
* A delegated `build`, `test` or `clean` exits with the build tool's own
  exit code (`mvn`/`gradle`), which is how a failing external build fails CI.
  jx's own refusals in delegate mode are usage errors (2) and are rendered
  with the usual `error:` block.
* jx's own exit codes 2 and above always accompany a rendered `error:`
  message in the documented format: `error [kind]: message` with `at:` and
  `hint:` lines. `jx test`'s propagated launcher result is test output, not
  a jx error, and may carry no such line.
