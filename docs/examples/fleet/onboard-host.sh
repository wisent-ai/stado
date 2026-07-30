#!/bin/sh
# onboard-host.sh — bring a new device to reporting life in the fleet.
#
# Prerequisites: the target has Remote Login enabled (host recover rides
# the approved channel), a skarbiec serve is reachable from the target,
# and SKARBIEC_VAULT_FILE points at the operator vault here.
#
# Usage: sh onboard-host.sh <host> <ssh-destination>
set -eu

HOST=$1
DEST=$2
SB=${SKARBIEC_BIN:-skarbiec}

# 1. registry membership
stado registry host add "$HOST" --ssh "$DEST"

# 2. compute agent on the target (over the approved channel, or --local here)
stado bootstrap --target "$HOST"

# 3. skarbiec side: the two grants every reporting host needs
"$SB" token-mint stado-local-agent --scopes 'read:*'
"$SB" token-mint stado-host-health-beacon --scopes 'read:stado-host-health-api'
"$SB" get stado-host-health-api > /dev/null

# 4. recovery program: beacon + managed units on the target
stado host recover "$HOST"

# 5. the proof: the host reports
stado registry beacon-age
