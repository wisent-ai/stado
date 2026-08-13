#!/bin/sh
# Publish a Linux fleet host's health beacon from this operator machine.
#
# Why this exists: `stado host publish-beacon` posts to the scoped health API,
# which is loopback-only on the control-plane host. charless-mac-mini reaches it
# through a dedicated reverse-tunnel LaunchAgent; ubuntu-server-rtx-pro-6000 has
# no such channel, and its own installed unit still publishes to GCS for project
# wisent-480400, whose billing is detached on purpose. So it reported nothing from
# 19 June onward while ssh, its agent and the box itself stayed healthy, and
# `stado host ping` called it stale.
#
# `deploy/host_health_beacon.sh` was written for exactly this split: with
# WC_BEACON_COLLECT_ONLY it prints the beacon and publishes nothing, so a host
# that cannot reach the API still produces the evidence and an operator hands it
# in. That is what this does, in one invocation:
#
#   scripts/publish-linux-host-beacon.sh ubuntu-server-rtx-pro-6000
#
# It requires `scripts/collect-linux-beacon.sh` installed on the target, which
# `stado host install-helper` does and this script performs when missing.
set -eu

TARGET="${1:-ubuntu-server-rtx-pro-6000}"
STADO="${STADO_BIN:-$HOME/.local/bin/stado}"
SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
BEACON_PLIST="$HOME/Library/LaunchAgents/com.wisent.host-health-beacon.plist"

[ -x "$STADO" ] || { printf 'no stado binary at %s\n' "$STADO" >&2; exit 1; }

# The publisher's endpoints and grant are whatever this machine's own beacon
# agent already uses; reading them there keeps one source of truth.
plist_value() {
    [ -f "$BEACON_PLIST" ] || return 0
    /usr/libexec/PlistBuddy -c "Print :EnvironmentVariables:$1" "$BEACON_PLIST" 2>/dev/null || true
}
: "${STADO_HOST_HEALTH_API_URL:=$(plist_value STADO_HOST_HEALTH_API_URL)}"
: "${STADO_HOST_HEALTH_SKARBIEC_URL:=$(plist_value STADO_HOST_HEALTH_SKARBIEC_URL)}"
: "${STADO_HOST_HEALTH_SKARBIEC_CONSUMER:=$(plist_value STADO_HOST_HEALTH_SKARBIEC_CONSUMER)}"
: "${STADO_HOST_HEALTH_SKARBIEC_TOKEN_FILE:=$(plist_value STADO_HOST_HEALTH_SKARBIEC_TOKEN_FILE)}"
export STADO_HOST_HEALTH_API_URL STADO_HOST_HEALTH_SKARBIEC_URL \
       STADO_HOST_HEALTH_SKARBIEC_CONSUMER STADO_HOST_HEALTH_SKARBIEC_TOKEN_FILE

for name in STADO_HOST_HEALTH_API_URL STADO_HOST_HEALTH_SKARBIEC_URL \
            STADO_HOST_HEALTH_SKARBIEC_CONSUMER STADO_HOST_HEALTH_SKARBIEC_TOKEN_FILE
do
    eval "value=\${$name}"
    [ -n "$value" ] || { printf '%s is unset and this machine has no beacon agent to read it from\n' "$name" >&2; exit 1; }
done

collect() {
    "$STADO" host run-helper "$TARGET" collect-linux-beacon.sh --json 2>/dev/null \
        | /usr/bin/python3 -c 'import json,sys; print(json.load(sys.stdin)["stdout"], end="")'
}

beacon=$(collect || true)
if [ -z "$beacon" ]; then
    "$STADO" host install-helper "$TARGET" \
        "$SCRIPT_DIR/collect-linux-beacon.sh" collect-linux-beacon.sh >/dev/null
    beacon=$(collect)
fi
[ -n "$beacon" ] || { printf 'collected no beacon from %s\n' "$TARGET" >&2; exit 1; }

printf '%s' "$beacon" | "$STADO" host publish-beacon -
"$STADO" host ping "$TARGET"
