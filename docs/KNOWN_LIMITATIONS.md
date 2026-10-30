# Known limitations

jx must never claim support for something untested. This file records honest
gaps, milestone by milestone.

## Current (M0 foundations)

- **Only `jx --version` and `jx doctor` are implemented.** Every other command
  parses and then reports itself as unimplemented, naming its milestone.
  Nothing in `--help` claims working behaviour.
- **`--offline`, `--locked` and `--manifest-path` print a "no effect yet"
  warning**; they are planned for M2/M3.
- **doctor's network probe** performs a single HTTPS HEAD against Maven
  Central with a 5 s connect timeout. Proxy environments (`HTTPS_PROXY`) are
  only exercised when the M2 proxy fixtures land; corporate proxies with
  custom CAs may currently fail the probe. `NO_PROXY` handling arrives with
  the M2 repository client.
- **The cache layer stores and verifies bytes** but no artifact download path
  exists yet (M2).
- **config.toml parsing is permissive**: a stray unknown key is ignored for
  now; strict diagnostics with did-you-mean arrive with M2/M3.
- **CI runs checks on Linux, macOS and Windows, but the milestone's
  end-to-end scenarios are not yet exercised anywhere.**
- **No release binaries, no install script** (M8).
