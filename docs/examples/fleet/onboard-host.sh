#!/bin/sh
# onboard-host.sh — bring a new device to reporting life in the fleet.
#
# Prerequisites, all on the target and all before this script: it is reachable
# at <ssh-destination> (any destination that opens — a .local name on the same
# network or a tailnet name both work), Remote Login is enabled, and the public
# key printed by `stado fleet key generate <host>` is in its
# ~/.ssh/authorized_keys. Enrollment probes the machine over that key before it
# writes anything, so there is no order in which this script can create the
# channel it needs. Here: a reachable `skarbiec serve`, and SKARBIEC_VAULT_FILE
# pointing at the operator vault.
#
# Usage: sh onboard-host.sh <host> <ssh-destination>
set -eu

HOST=$1
DEST=$2
SB=${SKARBIEC_BIN:-skarbiec}

# 1. registry membership, verified: the machine's own hostname and release
#    platform are probed over the channel before the entry is written, and a
#    failed agent install rolls the entry back.
stado fleet enroll "$HOST" --ssh "$DEST" --bootstrap

# 2. the channel the rest of this script rides
stado fleet key check "$HOST"

# 3. skarbiec side: the two grants every reporting host needs
"$SB" token-mint stado-local-agent --scopes 'read:*'
"$SB" token-mint stado-host-health-beacon --scopes 'read:stado-host-health-api'
"$SB" get stado-host-health-api > /dev/null

# 4. recovery program: beacon + managed units on the target
stado host recover "$HOST"

# 5. the proof: the host reports
stado registry beacon-age
