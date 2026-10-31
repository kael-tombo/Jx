# Repositories, mirrors, proxies and credentials

How jx talks to Maven repositories. The behaviour is specified in
[ADR 0003](decisions/0003-repository-client-and-pom-model.md); this page is
the user-facing summary. The values live in `$JX_HOME/config.toml`, not in
`project.toml` — dependency identity belongs to a project, repository
access belongs to a machine.

## config.toml

```toml
[repositories.internal]
url = "https://repo.example.com/maven"
auth = "env:JX_INTERNAL_TOKEN"          # never an inline secret
```

Order matters: repositories are tried **in declaration order**. When none
are declared, jx uses Maven Central (`https://repo.maven.apache.org/maven2`)
with id `central`. Declaring any repository replaces the default — include
`central` first yourself if you want to keep trying it:

```toml
[repositories.central]
url = "https://repo.maven.apache.org/maven2"
```

## Mirrors

```toml
[mirrors.corp]
of = "external:*"
url = "https://mirror.example.com/maven"
auth = "bearer:env:JX_MIRROR_TOKEN"
```

`of` patterns use Maven settings.xml's `mirrorOf` grammar — **not a glob**:

* ids, comma-separated: `internal,third-party`
* `*` — every repository (competing patterns: see selection below)
* `external:*` — any repository not on the local machine (`file:` urls and
  `localhost`/`127.0.0.1` hosts are never "external")
* `external:http:*` — external plain-HTTP repositories only
* `!id` — exclusion; if any exclusion in the pattern matches, the whole
  pattern does not apply, even if a positive part also matched

Selection: an exact whole-value `of = "internal"` wins first; otherwise the
first declared matching mirror, in declaration order. The mirror's own
`auth` key selects its credentials, exactly as Maven selects server
credentials by id.

## Proxies

Config wins over the environment:

```toml
[proxy]
http = "http://proxy.corp:3128"
https = "http://proxy.corp:3128"       # falls back to `http` when unset
no_proxy = "localhost, *.corp.example.com"
```

When `[proxy]` exists but has no entry for the request's scheme, jx falls
back to the environment for that scheme (`HTTPS_PROXY`/`HTTP_PROXY`, with
lowercase forms honoured; `NO_PROXY` likewise) rather than leaking direct
traffic past a configured proxy. `no_proxy` patterns are comma-separated
suffix matches on the host, case-insensitive; a leading `*` or `.` is
optional and ignored. Proxy selection happens against the **mirror-selected**
URL.

## Credentials

Secrets are never stored in files you typed a password into. The `auth`
value is a credential *reference*, read from the environment at fetch time:

```toml
auth = "env:JX_INTERNAL_TOKEN"       # variable holds user:password -> Basic
auth = "bearer:env:JX_MIRROR_TOKEN"  # variable holds a raw token     -> Bearer
```

Export the variable before the command; a missing variable is a hard error
that names the variable. Variable names are letters, digits and `_`.

## Integrity

Checksum verification is always on and never configurable (Maven 4 `fail`
semantics): response headers first (`x-checksum-sha256`, then
`x-checksum-sha1`, as Central serves), then `.sha256`/`.sha1` sidecar files;
sha256 before sha1. A mismatch is a hard error, and so is a repository that
publishes no checksum at all — jx never downloads unchecksummed artifacts.
The full security review (extraction, TLS, injection) is the M8
`docs/security-checklist.md`, listed in `docs/README.md`.
