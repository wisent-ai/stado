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
    echo "FATAL: Stado dependency preflight failed; resolve the active profile findings above."
    false
fi
COORD_TARGET="${COORD_TARGET:-local-control-plane}"
"$STADO_BIN" bootstrap --local --target "$COORD_TARGET"

# Replacing the binary does not replace a process that launchd already has in
# memory. Refresh the co-located resolver in place after every Stado upgrade;
# otherwise it can keep validating new registry fields with the previous
# release indefinitely while every adapter continues accepting connections.
SELF_TARGET="$("$STADO_BIN" registry self --name-only)"
RESOLVER_UNIT="com.wisent.stado-resolver"
RESOLVER_COUNT="$(
    "$STADO_BIN" service list --json |
        jq --arg host "$SELF_TARGET" --arg unit "$RESOLVER_UNIT" \
            '[.[] | select(.host == $host and .unit_id == $unit)] | length'
)"
case "$RESOLVER_COUNT" in
    0) ;;
    1)
        "$STADO_BIN" service restart "$RESOLVER_UNIT" --host "$SELF_TARGET" --json
        RESOLVER_READY=false
        for attempt in $(seq 1 12); do
            RESOLVER_REPORT="$(
                "$STADO_BIN" resolver status --target "$SELF_TARGET" --json 2>/dev/null || true
            )"
            if printf '%s' "$RESOLVER_REPORT" |
                jq -e '.state == "serving" and .api.listening == true' >/dev/null; then
                RESOLVER_READY=true
                break
            fi
            sleep "$((attempt * 2))"
        done
        if [ "$RESOLVER_READY" != true ]; then
            printf '%s\n' "$RESOLVER_REPORT"
            echo "FATAL: upgraded Stado resolver did not load the service directory."
            false
        fi
        ;;
    *)
        echo "FATAL: $SELF_TARGET declares $RESOLVER_COUNT copies of $RESOLVER_UNIT"
        false
        ;;
esac
if ! "$STADO_BIN" doctor --fix-hints --release-verification; then
    echo "FATAL: deployed Stado does not serve the exact published release."
    false
fi
