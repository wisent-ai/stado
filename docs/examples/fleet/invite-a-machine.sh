#!/bin/sh
# invite-a-machine.sh — the `invite` method end to end, operator side.
#
# The point of this method: the operator never touches the machine. No key is
# pasted anywhere by hand, and the fleet's private key never leaves the
# operator's Skarbiec — only the public line reaches the machine, because the
# fleet dials IN to it.
#
# Prerequisites here: a reachable `skarbiec serve` with SKARBIEC_VAULT_FILE
# pointing at the operator vault, and STADO_API_URL set to the control address
# the machine will reach (that is the address printed in the one-liner).
#
# On the machine being added: nothing. No Stado binary, no credentials — the
# owner runs one line, and Remote Login has to be on before approval.
#
# Usage: sh invite-a-machine.sh <name> <hostname-the-machine-reports>
#        (if you do not know the hostname yet, run this script's step 3 —
#         `stado fleet pending` — first: the request prints it.)
set -eu

NAME=$1
HOSTNAME_REPORTED=$2
SB=${SKARBIEC_BIN:-skarbiec}

# 0. is this method allowed here, and what are the alternatives?
#    `methods` prints all four (invite, adopt, join, declare) with the registry
#    catalog's verdict; `catalog` prints the gating fields themselves.
stado fleet methods
stado fleet catalog

# 1. mint the invitation. Prints the token ONCE — `<id>.<secret>` — plus the
#    single line to forward to whoever holds the machine. The store keeps only
#    secret_sha256; nothing can reprint the token.
#    Defaults: one use, 24 hours. `--uses N` and `--expires 30m|24h|7d` widen it.
stado fleet invite --name "$NAME"

# 2. the operator's own view of live invitations: id, target, status, spend,
#    expiry — never the token.
stado fleet invites

# 3. the machine's owner now runs the printed line:
#      curl -fsSL "$STADO_API_URL/join.sh" | sh -s -- <id>.<secret>
#    which installs the fleet's PUBLIC key and reports the machine in. Wait for
#    the request to appear; it carries the destination approval will probe, the
#    invite id, the installed key fingerprint, and whether ssh answered.
stado fleet pending

#    the invited requests alone, machine-readable: the hostname `approve` takes,
#    the target name the invite reserved (the fleet name this machine gets), the
#    destination approval will probe, the invitation, and whether ssh answered.
stado fleet pending --json |
  jq -r '.pending[] | select(.destination != null) |
         "\(.hostname)\t\(.target_name)\t\(.destination)\t\(.invite_id)\t\(.ssh_listening)"'

# 4. approve. This is not a rubber stamp: it takes the destination from the
#    request and runs the same probe-then-write enrollment as `fleet enroll` —
#    hostname and uname read over the channel before the registry is written,
#    with rollback if the agent install fails.
stado fleet approve "$HOSTNAME_REPORTED"

# 5. the channel the fleet now owns, and the grants a reporting host needs
stado fleet key check "$NAME"
"$SB" token-mint stado-local-agent --scopes 'read:*'
"$SB" token-mint stado-host-health-beacon --scopes 'read:stado-host-health-api'

# 6. recovery program: beacon + managed units on the target
stado host recover "$NAME"

# 7. the proof: the machine reports
stado registry beacon-age

# 8. housekeeping: a single-use invitation is spent by now, but any invitation
#    still open can be closed before its expiry.
stado fleet invites
# stado fleet revoke-invite <id>
