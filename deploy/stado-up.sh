#!/bin/sh
# stado-up: fetch the Rust binary, preflight the explicitly selected profile,
# then let `stado bootstrap --local` own persistent service installation.
set -eu
SCRIPT_DIR="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"

TARGET="${1:-local-control-plane}"

BIN_DIR="${HOME}/.stado/bin"
mkdir -p "$BIN_DIR"
STADO_BIN="${STADO_BIN:-$BIN_DIR/stado}"
# Bootstrap reads a caller-pinned immutable version through Stado's public,
# release-only GET route. Direct provider URLs and mutable latest pointers are
# intentionally unsupported.
RELEASE_API="${STADO_RELEASE_API_URL:?set the HTTPS Stado control origin}"
VERSION="${STADO_RELEASE_VERSION:?pin the exact immutable Stado version}"
PLATFORM="${STADO_RELEASE_PLATFORM:?pin the exact Stado release platform}"
case "$RELEASE_API" in
    https://*) ;;
    *) echo "FATAL: STADO_RELEASE_API_URL must use HTTPS"; false ;;
esac
case "$VERSION" in
    *[![:alnum:]._-]*|"") echo "FATAL: invalid STADO_RELEASE_VERSION"; false ;;
esac
case "$PLATFORM" in
    *[![:alnum:]._-]*|"") echo "FATAL: invalid STADO_RELEASE_PLATFORM"; false ;;
esac
RELEASE_API="${RELEASE_API%/}"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
download_release() {
    object="$1"
    destination="$2"
    curl -fsSL --get \
        --data-urlencode "uri=stado://releases/stado/${VERSION}/${PLATFORM}/${object}" \
        "${RELEASE_API}/api/release/object" \
        -o "$destination"
}
RELEASE_BINARIES="stado stado-coverage stado-fix stado-watchdog stado-mcp"
for name in $RELEASE_BINARIES; do
    download_release "$name" "${TMP}/${name}"
done
download_release SHA256SUMS "${TMP}/SHA256SUMS"
for name in $RELEASE_BINARIES; do
    EXPECTED="$(grep -E "[ *]${name}$" "${TMP}/SHA256SUMS")"
    EXPECTED="${EXPECTED%% *}"
    ACTUAL="$(openssl dgst -sha256 "${TMP}/${name}")"
    ACTUAL="${ACTUAL##* }"
    [ -n "$EXPECTED" ] && [ "$ACTUAL" = "$EXPECTED" ]
done
for name in $RELEASE_BINARIES; do
    target="${BIN_DIR}/${name}"
    if [ "$name" = stado ]; then
        target="$STADO_BIN"
    fi
    chmod +x "${TMP}/${name}"
    mv "${TMP}/${name}" "${target}.new"
    mv "${target}.new" "$target"
done

# Persistent profile ownership stays with SecretStateRepair. This installer
# only installs the scoped release on the current host.
if [ -z "${STADO_CONFIG:-}" ] || [ ! -r "$STADO_CONFIG" ]; then
    echo "FATAL: STADO_CONFIG must name the readable profile installed by SecretStateRepair"
    false
fi
if [ "$TARGET" != "local-control-plane" ]; then
    echo "FATAL: stado-up installs only the local-control-plane owner; remote agents use stado bootstrap without --local"
    false
fi
export STADO_CONFIG
"$STADO_BIN" config validate
"$STADO_BIN" doctor --fix-hints
exec "$STADO_BIN" bootstrap --local --target "$TARGET"
