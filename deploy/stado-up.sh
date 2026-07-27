#!/bin/sh
# stado-up: fetch the Rust binary, preflight the explicitly selected profile,
# then let `stado bootstrap --local` own persistent service installation.
set -eu
SCRIPT_DIR="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"

TARGET="${1:?"usage: stado-up <target-name>"}"

BIN_DIR="${HOME}/.stado/bin"
mkdir -p "$BIN_DIR"
STADO_BIN="${STADO_BIN:-$BIN_DIR/stado}"
# Initial release channel. There is deliberately no GCS fallback. Operator
# laptops have no managed identity for private Azure Blob reads, so provide a
# public channel or append a container SAS to WC_RELEASE_BASE_URL. Once Rust
# Stado is installed, its own fetcher uses the configured Azure identity.
if [ -n "${WC_RELEASE_BASE_URL:-}" ]; then
    RELEASE_BASE="$WC_RELEASE_BASE_URL"
elif [ -n "${WC_AZURE_STORAGE_ACCOUNT:-}" ]; then
    RELEASE_BASE="https://${WC_AZURE_STORAGE_ACCOUNT}.blob.core.windows.net/${WC_AZURE_CONTAINER:-stado}/releases/stado"
else
    echo "FATAL: release channel unresolved; set WC_RELEASE_BASE_URL (Azure Blob URL plus SAS when private)"
    false
fi
RELEASE_QS=""
case "$RELEASE_BASE" in
    *\?*)
        RELEASE_QS="?${RELEASE_BASE#*\?}"
        RELEASE_BASE="${RELEASE_BASE%%\?*}"
        ;;
esac
RELEASE_BASE="${RELEASE_BASE%/}"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
curl -fsSL "${RELEASE_BASE}/latest.json${RELEASE_QS}" -o "${TMP}/latest.json"
VERSION="$(/usr/bin/plutil -extract version raw -o - "${TMP}/latest.json")"
BASE="${RELEASE_BASE}/${VERSION}/darwin-arm64"
RELEASE_BINARIES="stado wc stado-coverage stado-fix stado-watchdog stado-mcp"
for name in $RELEASE_BINARIES; do
    curl -fsSL "${BASE}/${name}${RELEASE_QS}" -o "${TMP}/${name}"
done
curl -fsSL "${BASE}/SHA256SUMS${RELEASE_QS}" -o "${TMP}/SHA256SUMS"
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

# The bootstrap-only SAS is never persisted, but all binaries needed by Rust
# bootstrap are installed before it is removed from the channel URL.
export WC_RELEASE_BASE_URL="$RELEASE_BASE"
if [ -z "${STADO_CONFIG:-}" ]; then
    STADO_CONFIG="$HOME/.config/stado/config.json"
    mkdir -p "${STADO_CONFIG%/*}"
    cp "$SCRIPT_DIR/local/stado.config.json" "$STADO_CONFIG"
    chmod u=rw,go= "$STADO_CONFIG"
fi
if [ ! -r "$STADO_CONFIG" ]; then
    echo "FATAL: missing Stado config at $STADO_CONFIG"
    echo "Use deploy/local/stado.config.json for outage mode, or explicitly set STADO_CONFIG."
    false
fi
export STADO_CONFIG
"$STADO_BIN" config validate
"$STADO_BIN" doctor --fix-hints
exec "$STADO_BIN" bootstrap --local --target "$TARGET"
