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

if [ -z "${STADO_CONFIG:-}" ]; then
    echo "FATAL: STADO_CONFIG must name an operator-owned deployment profile"
    echo "Create a neutral local profile with 'stado config init', then set STADO_CONFIG to that file."
    false
fi
if [ ! -r "$STADO_CONFIG" ]; then
    echo "FATAL: missing Stado config at $STADO_CONFIG"
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
if ! "$STADO_BIN" doctor --fix-hints --deployment-preflight; then
    echo "FATAL: Stado preflight failed; resolve the active profile findings above."
    false
fi
COORD_TARGET="${COORD_TARGET:-local-control-plane}"

exec "$STADO_BIN" bootstrap --local --target "$COORD_TARGET"
