#!/bin/sh
# Install a released jx binary on Linux or macOS.
#
#   sh install.sh [--version v1.0.0] [--dir "$HOME/.local/bin"]
#
# The archive and its SHA-256 are downloaded from the same GitHub release; the
# checksum is verified before the binary is moved into place. No sudo is used.
# Environment overrides:
#   JX_VERSION             v1.0.0, 1.0.0 or "latest" (default)
#   JX_INSTALL_DIR         destination directory (default: ~/.local/bin)
#   JX_RELEASE_BASE_URL    download root for a mirror, including the tag
#   JX_REPO                owner/name (default: kael-tombo/Jx)
#   JX_TARGET              override target detection (for tests/mirrors)

set -eu

REPO="${JX_REPO:-kael-tombo/Jx}"
VERSION="${JX_VERSION:-latest}"
INSTALL_DIR="${JX_INSTALL_DIR:-${HOME}/.local/bin}"
RELEASE_BASE="${JX_RELEASE_BASE_URL:-}"

usage() {
    cat <<'EOF'
usage: install.sh [--version VERSION] [--dir DIRECTORY]

  --version v1.0.0   install a specific tag (default: latest release)
  --dir PATH         install directory (default: ~/.local/bin)
  -h, --help         show this help
EOF
}

die() {
    printf 'jx install: %s\n' "$*" >&2
    exit 1
}

while [ $# -gt 0 ]; do
    case "$1" in
        --version)
            [ $# -ge 2 ] || die "--version needs a value"
            VERSION="$2"
            shift 2
            ;;
        --dir)
            [ $# -ge 2 ] || die "--dir needs a value"
            INSTALL_DIR="$2"
            shift 2
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            die "unknown argument: $1 (try --help)"
            ;;
    esac
done

command -v curl >/dev/null 2>&1 ||
    die "curl is required; install curl and retry"

case "$VERSION" in
    latest) ;;
    v*) ;;
    [0-9]*) VERSION="v${VERSION}" ;;
    *) die "invalid version: $VERSION" ;;
esac
case "$VERSION" in
    *[!0-9A-Za-z._+-]*) die "invalid version: $VERSION" ;;
esac

os=$(uname -s 2>/dev/null || true)
arch=$(uname -m 2>/dev/null || true)
if [ -n "${JX_TARGET:-}" ]; then
    target="$JX_TARGET"
    case "$target" in
        x86_64-unknown-linux-gnu|aarch64-unknown-linux-gnu|x86_64-apple-darwin|aarch64-apple-darwin) ;;
        *) die "unsupported JX_TARGET: ${target}" ;;
    esac
else
    case "${os}:${arch}" in
        Linux:x86_64|Linux:amd64)
            target="x86_64-unknown-linux-gnu"
            ;;
        Linux:aarch64|Linux:arm64)
            target="aarch64-unknown-linux-gnu"
            ;;
        Darwin:x86_64|Darwin:amd64)
            target="x86_64-apple-darwin"
            ;;
        Darwin:arm64|Darwin:aarch64)
            target="aarch64-apple-darwin"
            ;;
        MINGW*|MSYS*|CYGWIN*)
            die "this script installs Linux/macOS builds; on Windows use install.ps1"
            ;;
        *)
            die "no prebuilt jx for ${os:-unknown} ${arch:-unknown}; build from source with cargo"
            ;;
    esac
fi

if [ "$VERSION" = "latest" ]; then
    effective=$(curl -fsSL -o /dev/null -w '%{url_effective}' \
        "https://github.com/${REPO}/releases/latest") ||
        die "no published release was found for ${REPO}; build from source or pass --version"
    VERSION=${effective##*/}
    case "$VERSION" in
        v*) ;;
        *) die "could not determine the latest release tag" ;;
    esac
fi

if [ -n "$RELEASE_BASE" ]; then
    base="${RELEASE_BASE%/}"
else
    base="https://github.com/${REPO}/releases/download/${VERSION}"
fi
asset="jx-${VERSION}-${target}.tar.gz"
stage="jx-${VERSION}-${target}"

tmp=$(mktemp -d "${TMPDIR:-/tmp}/jx-install.XXXXXX") ||
    die "cannot create a temporary directory"
tmp_bin="${INSTALL_DIR}/.jx.tmp.$$"
cleanup() {
    rm -rf "$tmp"
    rm -f "$tmp_bin"
}
trap cleanup 0 1 2 15

printf 'Downloading %s\n' "$asset"
curl -fsSL "${base}/${asset}" -o "${tmp}/${asset}" ||
    die "could not download ${base}/${asset}"
curl -fsSL "${base}/SHA256SUMS" -o "${tmp}/SHA256SUMS" ||
    die "could not download ${base}/SHA256SUMS"

expected=$(awk -v name="$asset" '{ file=$2; sub(/^\*/, "", file); if (file == name) { print $1; exit } }' "${tmp}/SHA256SUMS")
[ -n "$expected" ] || die "SHA256SUMS has no entry for ${asset}"
if command -v sha256sum >/dev/null 2>&1; then
    actual=$(sha256sum "${tmp}/${asset}" | awk '{ print $1 }')
elif command -v shasum >/dev/null 2>&1; then
    actual=$(shasum -a 256 "${tmp}/${asset}" | awk '{ print $1 }')
elif command -v openssl >/dev/null 2>&1; then
    actual=$(openssl dgst -sha256 "${tmp}/${asset}" | awk '{ print $NF }')
else
    die "no SHA-256 tool found (sha256sum, shasum or openssl)"
fi
expected=$(printf '%s' "$expected" | tr 'A-F' 'a-f')
actual=$(printf '%s' "$actual" | tr 'A-F' 'a-f')
[ "$actual" = "$expected" ] ||
    die "checksum mismatch: expected ${expected}, got ${actual}"

tar -xzf "${tmp}/${asset}" -C "$tmp" ||
    die "could not extract ${asset}"
[ -f "${tmp}/${stage}/jx" ] ||
    die "the archive does not contain ${stage}/jx"

mkdir -p "$INSTALL_DIR" || die "cannot create ${INSTALL_DIR}"
cp "${tmp}/${stage}/jx" "$tmp_bin" || die "cannot write to ${INSTALL_DIR}"
chmod 755 "$tmp_bin"
mv -f "$tmp_bin" "${INSTALL_DIR}/jx" || die "cannot install to ${INSTALL_DIR}"

printf 'Installed %s to %s\n' "$VERSION" "${INSTALL_DIR}/jx"
"${INSTALL_DIR}/jx" --version || die "the installed binary did not run"

case ":${PATH}:" in
    *":${INSTALL_DIR}:"*) ;;
    *)
        printf '\nAdd it to PATH for future shells:\n  export PATH="%s:$PATH"\n' "$INSTALL_DIR"
        ;;
esac
