# jx exit codes

Every jx command exits with a code a script can rely on.

| Code | Meaning | Categories that produce it |
| --- | --- | --- |
| 0 | Success. Warnings do not change the exit code (except `jx lock --check`, M3, and `jx doctor` only when a check cannot run). | — |
| 1 | A diagnosed failure (io, network, toolchain, manifest, lock, resolve, build). | error kinds other than `usage` |
| 2 | Bad usage: unknown flag, unparseable arguments, or a command jx does not yet implement. | `usage` |

Notes:

* `jx doctor` exits 0 when it completes, even with warnings; exits 1 only when
  a check itself cannot complete.
* `jx lock --check` (planned M3) exits non-zero when the lock is stale, as its
  contract, not as an error.
* Exit codes 2 and above always accompany a rendered `error:` message in the
  documented format: `error [kind]: message` with `at:` and `hint:` lines.
