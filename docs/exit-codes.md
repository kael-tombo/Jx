# jx exit codes

Every jx command exits with a code a script can rely on.

| Code | Meaning | Categories that produce it |
| --- | --- | --- |
| 0 | Success. Warnings do not change the exit code (`jx doctor` exits 1 only when a check cannot run). | — |
| 1 | A diagnosed failure (io, network, toolchain, manifest, lock, resolve, build). | error kinds other than `usage` |
| 2 | Bad usage: unknown flag, unparseable arguments, or a command jx does not yet implement. | `usage` |

Notes:

* `jx doctor` exits 0 when it completes, even with warnings; exits 1 only when
  a check itself cannot complete.
* The global `--locked` flag makes `jx lock`, `jx add`, `jx remove` and the
  build fast path fail with a diagnosed `lock` error when the lockfile is
  missing or would change. That is its contract, not an internal failure;
  the exit code is still 1 like any other diagnosed error.
* Exit codes 2 and above always accompany a rendered `error:` message in the
  documented format: `error [kind]: message` with `at:` and `hint:` lines.
