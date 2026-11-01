#!/usr/bin/env bash
# Package one release archive for a Unix target. Used by the release workflow
# and runnable locally against a debug binary:
#
#   scripts/package-release.sh x86_64-unknown-linux-gnu v1.0.0 \
#     target/debug/jx
#
# Writes dist/jx-<tag>-<target>.tar.gz containing the binary, README, licence
# texts and SECURITY.md at the archive root (one top directory).

set -euo pipefail

usage() {
    printf 'usage: %s <target> <tag> [binary]\n' "$0" >&2
    exit 2
}

target="${1:-}"
tag="${2:-}"
binary="${3:-target/${target}/release/jx}"

[ -n "$target" ] && [ -n "$tag" ] || usage
case "$tag" in
    v[0-9]*) ;;
    *) printf 'tag must look like v1.0.0: %s\n' "$tag" >&2; exit 2 ;;
esac
case "$target" in
    *windows*) printf 'Windows archives are packaged by install.ps1/release.yml\n' >&2; exit 2 ;;
esac
[ -f "$binary" ] || {
    printf 'missing binary: %s\n' "$binary" >&2
    exit 1
}

stage="jx-${tag}-${target}"
out="dist"
asset="${out}/${stage}.tar.gz"

rm -rf "${out:?}/${stage}" "$asset"
mkdir -p "${out}/${stage}"
cp "$binary" "${out}/${stage}/jx"
chmod 755 "${out}/${stage}/jx"
cp README.md LICENSE-APACHE LICENSE-MIT SECURITY.md "${out}/${stage}/"

tar -czf "$asset" -C "$out" "$stage"
printf '%s\n' "$asset"
