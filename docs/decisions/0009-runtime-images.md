# 0009. Runtime images with jdeps and jlink

Date: 2026-11-01
Status: Accepted

## Context and problem

A jx project can be packaged as a thin jar plus `dist/lib`, but running it
still requires a system Java or a full JDK. M6 adds a runtime image: a JDK
subset produced by `jlink`, placed next to the application in `target/dist`.

The hard part is not invoking `jlink`; it is choosing modules safely. `jdeps`
reads class references, so it cannot see reflection, service loading through
configuration, or classes loaded by name. A runtime that is too small fails
only at startup, long after the build looked successful. A runtime that is
always full is safe but gives up the point of the milestone.

## Decision

`jx image` builds the thin dist first, then creates `target/dist/runtime`.

* **Analysis:** `jdeps --print-module-deps --ignore-missing-deps
  --multi-release <feature> --class-path <main dependencies>` runs against
  the compiled classes. The module list is sorted and de-duplicated. jx only
  accepts syntactically conservative module names. `--ignore-missing-deps`
  keeps optional annotation classes from blocking analysis; it also means a
  genuinely missing dependency can yield an incomplete module set, which is
  why the full fallback exists.
* **Linking:** `jlink --add-modules <modules> --strip-debug --no-header-files
  --no-man-pages`. No compression flag is passed: its spelling changed across
  JDK releases, and a portable command matters more than a few image bytes.
* **Fallback:** if jdeps cannot run, exits non-zero, or produces no module
  list, jx builds a full runtime with `ALL-MODULE-PATH` and prints the reason.
  `jx image --full` skips analysis deliberately. A full image is large but
  still a valid, self-contained runtime.
* **Atomic replacement:** `jlink` writes a process-unique temporary directory
  beside the target. jx starts that runtime with `--list-modules`, checks that
  every requested module is present, and only then replaces the previous
  `runtime`. Before packaging, the whole previous `target/dist` is moved to
  `.jx-dist-backup-<pid>`; any later failure removes the incomplete dist and
  restores that backup. A failed image therefore does not destroy a working
  app/runtime pair.
* **Application only:** the app must have a main class. jx prints the exact
  `.../runtime/bin/java -jar ...` command; it does not create launch scripts,
  native installers or a jpackage bundle yet.
* **No compression, no cross-compilation, no signing.** The image is for the
  current platform and the pinned JDK. A macOS image is not produced on
  Windows, and macOS distribution signing is outside this milestone.

The generated tree is:

```text
target/dist/
  <name>-<version>.jar
  lib/...
  runtime/
    bin/java
    lib/...
```

`jx package` rebuilds `target/dist` from scratch, so it removes a previously
generated `runtime`. Run `jx image` again when the image is wanted. The backup
is only held for the duration of `jx image`; a successful image removes it.

## Options considered

* **Always fall back silently to a full runtime** — convenient, but hides the
  fact that trimming did not happen. The mode and reason are printed.
* **Fail when jdeps cannot trim** — forces users to discover `--full`, but
  makes a common optional-dependency failure block packaging. Fallback is the
  better default because the resulting image still works.
* **Run the application as verification** — would catch reflective failures,
  but builds must not execute arbitrary user programs. `--list-modules` is the
  strongest side-effect-free check available.
* **`--compress=2`** — smaller images, but the option and accepted values
  changed in recent JDKs. Omitting it keeps the command valid across the
  supported feature releases.
* **jpackage** — produces installers and platform metadata, which is a much
  larger surface than M6 promises. Deferred.

## Consequences

* A trimmed image is typically a few tens of megabytes instead of a full JDK;
  the smoke check produced 47 MB for an app needing `java.sql` and 135 MB for
  the full fallback on Temurin 17.
* Reflection and dynamic module loading remain a real risk. jx prints a note
  on trimmed images and offers `--full`; it does not claim that `jdeps` proves
  runtime correctness.
* JDK 8 cannot produce images because it has no `jlink`; jx reports that
  before writing anything.
* `jx image` requires a full JDK with `jmods`. The jx-installed Temurin JDKs
  include them.
