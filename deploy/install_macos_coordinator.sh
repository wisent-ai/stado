#!/bin/bash
# Install the Rust Stado coordinator on this Mac through Stado's own
# provider-neutral bootstrap path. Cloud credentials stay in Skarbiec and
# storage/provider selection comes only from the Stado config file.

set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

STADO_BIN="${STADO_BIN:-$HOME/.stado/bin/stado}"
if [ ! -x "$STADO_BIN" ]; then
    STADO_BIN="$(command -v stado || true)"
fi
if [ -z "$STADO_BIN" ] || [ ! -x "$STADO_BIN" ]; then
    echo "FATAL: Rust stado binary not found; set STADO_BIN"
    false
fi

STADO_CONFIG="${STADO_CONFIG:-$SCRIPT_DIR/local/stado.config.json}"
if [ ! -r "$STADO_CONFIG" ]; then
    echo "FATAL: missing Stado config at $STADO_CONFIG"
    echo "Use deploy/local/stado.config.json for outage mode, or explicitly set STADO_CONFIG."
    false
fi
if grep -q '<[^>]*>' "$STADO_CONFIG"; then
    echo "FATAL: unresolved placeholder in $STADO_CONFIG"
    false
fi

export STADO_CONFIG

# Gate service installation on the same resolved configuration and dependency
# probes the running Rust control plane uses. The selected profile decides
# whether local outage storage or fenced Azure production resources apply.
if ! "$STADO_BIN" config validate; then
    echo "FATAL: Stado deployment config is not ready; resolve every ERROR above."
    false
fi
if ! "$STADO_BIN" doctor --fix-hints; then
    echo "FATAL: Stado preflight failed; resolve the active profile findings above."
    false
fi
COORD_TARGET="${COORD_TARGET:-local-control-plane}"

exec "$STADO_BIN" bootstrap --local --target "$COORD_TARGET"
