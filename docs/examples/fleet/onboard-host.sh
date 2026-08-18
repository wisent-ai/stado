#!/bin/sh
# onboard-host.sh — bring a new device to reporting life in the fleet, over a
# channel that already exists.
#
# This is the sequence after the machine already trusts the fleet's public key.
# Getting it there is a choice of method (`stado fleet methods` lists all four):
#   * adopt  — `stado fleet enroll <host> --ssh <dest> --install-key`, when a
#              plain `ssh <dest>` already works for you; Stado installs the key
#              itself and this script's step 1 is that same command.
#   * invite — `stado fleet invite --offline`, when you cannot reach the machine
#              at all: its owner pastes the printed fragment and sends back an
#              address, and step 1 below closes the invitation. See
#              invite-a-machine.sh; in the one-line mode of the same method it
#              is `stado fleet approve` that replaces step 1 instead.
#   * paste  — the public line from `stado fleet key generate <host>`, appended
#              by hand on the machine, which is what this script assumes.
# In every one of them only the PUBLIC key reaches the machine; the private half
# stays in the operator's vault.
#
# Prerequisites on the target: it is reachable at <ssh-destination> (any
# destination that opens — a .local name on the same network or a tailnet name
# both work), Remote Login is enabled, and the fleet's public key is in its
# ~/.ssh/authorized_keys. Enrollment probes the machine over that key before it
# writes anything. Here: a reachable `skarbiec serve`, and SKARBIEC_VAULT_FILE
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
