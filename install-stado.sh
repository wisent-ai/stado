#!/usr/bin/env bash
# Install `stado` from one exact signed Stado release: the `release.json` and
# `release.tar.gz` that `stado release submit` publishes under
# stado://releases/stado/<version>/<platform>/, read through the public
# /api/release/object route. The manifest's identity, digest and size are
# checked against the archive before anything is installed; no mutable
# release is ever resolved.
#
# The same file is what `stado bootstrap` runs on a remote host and what
# `stado bootstrap --print-install-script` prints, with STADO_API_URL and
# STADO_RELEASE_VERSION bound in front of it. Its last two lines of output are
# the platform and the installed path.
set -euo pipefail
: "${STADO_API_URL:?set STADO_API_URL to the HTTPS origin of a Stado API}"
: "${STADO_RELEASE_VERSION:?set STADO_RELEASE_VERSION to an exact Stado version}"
BIN_DIR="${STADO_BIN_DIR:-$HOME/.stado/bin}"
mkdir -p "$BIN_DIR"
fail() { echo "$1" >&2; exit 1; }
case "$(uname -s)-$(uname -m)" in
  Linux-x86_64) platform=linux-amd64 ;;
  Darwin-arm64) platform=darwin-arm64 ;;
  *) fail "unsupported platform: $(uname -s) $(uname -m)" ;;
esac
case "$STADO_API_URL" in
  https://*) ;;
  *) fail "STADO_API_URL must use HTTPS" ;;
esac
case "$STADO_RELEASE_VERSION" in
  *[![:alnum:]._-]*|"") fail "invalid STADO_RELEASE_VERSION" ;;
esac
release_api="${STADO_API_URL%/}"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
base="stado://releases/stado/$STADO_RELEASE_VERSION/$platform"
for name in release.json release.tar.gz; do
  curl -fsSL --get \
    --data-urlencode "uri=$base/$name" \
    "$release_api/api/release/object" \
    -o "$tmp/$name"
done
manifest=$(tr -d '\n\r' < "$tmp/release.json")
# One JSON string field of the signed manifest, or nothing.
field() {
  printf '%s' "$manifest" | sed -n "s/.*\"$1\"[[:space:]]*:[[:space:]]*\"\([^\"]*\)\".*/\1/p"
}
[ "$(field product)" = stado ] && [ "$(field version)" = "$STADO_RELEASE_VERSION" ] \
  && [ "$(field platform)" = "$platform" ] || fail "release manifest identity mismatch"
revision=$(field source_revision)
case "$revision" in *[!0-9a-fA-F]*|"") fail "release manifest source revision is invalid" ;; esac
[ "${#revision}" = 40 ] || [ "${#revision}" = 64 ] || fail "release manifest source revision is invalid"
digest=$(field artifact_sha256)
case "$digest" in *[!0-9a-f]*|"") fail "release manifest digest is invalid" ;; esac
[ "${#digest}" = 64 ] || fail "release manifest digest is invalid"
bytes=$(printf '%s' "$manifest" | sed -n 's/.*"artifact_bytes"[[:space:]]*:[[:space:]]*\([0-9][0-9]*\).*/\1/p')
[ -n "$bytes" ] || fail "release manifest artifact size is invalid"
[ "$(wc -c < "$tmp/release.tar.gz" | tr -d ' ')" = "$bytes" ] \
  || fail "release archive is not the size its manifest binds"
if command -v sha256sum >/dev/null 2>&1; then
  actual=$(sha256sum "$tmp/release.tar.gz" | cut -d' ' -f1)
else
  actual=$(shasum -a 256 "$tmp/release.tar.gz" | cut -d' ' -f1)
fi
[ "$actual" = "$digest" ] || fail "release archive digest mismatch"
mkdir "$tmp/out"
member=$(tar -tzf "$tmp/release.tar.gz" | grep -xE '(\./)?stado' || true)
[ "$(printf '%s\n' "$member" | grep -c .)" = 1 ] || fail "release archive has no single stado member"
tar -xzf "$tmp/release.tar.gz" -C "$tmp/out" "$member"
{ [ -f "$tmp/out/stado" ] && [ ! -L "$tmp/out/stado" ]; } \
  || fail "release archive stado member is not a regular file"
chmod 755 "$tmp/out/stado"
mv "$tmp/out/stado" "$BIN_DIR/stado"
echo "$platform"
echo "$BIN_DIR/stado"
