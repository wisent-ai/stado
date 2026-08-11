#!/bin/sh
# Collect this host's beacon and print it, publishing nothing.
#
# `deploy/host_health_beacon.sh` supports exactly this through
# WC_BEACON_COLLECT_ONLY, because a host that cannot reach the loopback health
# API can still produce the evidence and let an operator's stado hand it in.
# That is this host's situation: its installed unit still publishes to GCS for
# project wisent-480400, whose billing is detached on purpose, so it has reported
# nothing since June while ssh and its agent stayed healthy.
#
# Reads the script from the pinned checkout this host already builds stado from,
# so the collector and the binary come from one revision.
set -eu

PATH="$HOME/.cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"
export PATH

CHECKOUT="$HOME/.stado/build-work/stado"
SCRIPT="$CHECKOUT/deploy/host_health_beacon.sh"
[ -f "$SCRIPT" ] || { printf 'no beacon script at %s\n' "$SCRIPT" >&2; exit 1; }

STADO_BIN="$HOME/.stado/bin/stado"
export STADO_BIN
export WC_BEACON_COLLECT_ONLY=yes

exec /usr/bin/bash "$SCRIPT"
