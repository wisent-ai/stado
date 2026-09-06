#!/bin/sh
# invite-a-machine.sh — the `invite` method end to end, operator side.
#
# The point of this method: the operator never touches the machine. No key is
# pasted anywhere by the operator, and the fleet's private key never leaves the
# operator's Skarbiec — only the public line reaches the machine, because the
# fleet dials IN to it.
#
# The method has two modes, and this script runs the one that works without
# publishing anything:
#
#   offline (here)  — `stado fleet invite --offline` prints a fragment. The
#                     machine's owner pastes it into a terminal there, it
#                     installs the fleet's PUBLIC key and prints `user@address`,
#                     they send that back, and the operator closes the
#                     invitation with `stado fleet enroll --ssh … --bootstrap`.
#                     Needs no DNS name, no ingress, no HTTP route at all.
#   one line        — `stado fleet invite` (no flag) prints a single `curl` line
#                     the owner runs. It only does that after proving
#                     `<control-point>/join.sh` answers 200 from this host; when
#                     it cannot, it prints why and falls back to offline mode.
#                     See the commented block at the bottom, and
#                     ../../cli.md#the-control-point-check for the reasons.
#
# Prerequisites here: a reachable `skarbiec serve` with SKARBIEC_VAULT_FILE
# pointing at the operator vault. No STADO_API_URL is needed for offline mode —
# nothing in it contacts a control point.
#
# On the machine being added: nothing. No Stado binary, no credentials, no
# outward HTTPS — its owner pastes one fragment, and Remote Login has to be on
# before the enrollment below can read the machine.
#
# Usage: sh invite-a-machine.sh <name>                  # step 1: mint and print the fragment
#        sh invite-a-machine.sh <name> <user@address>   # step 2: the address came back
set -eu

NAME=$1
ADDRESS=${2:-}
SB=${SKARBIEC_BIN:-skarbiec}

# 0. is this method allowed here, and what are the alternatives?
#    `methods` prints all four (invite, adopt, join, declare) with the registry
#    catalog's verdict; `catalog` prints the gating fields themselves.
stado fleet methods
stado fleet catalog

if [ -z "$ADDRESS" ]; then
  # 1. mint the invitation in offline mode. Mints the channel key
  #    (stado-ssh-<name>, private half stays in the vault), prints its
  #    fingerprint, and prints the paste-ready fragment between two markers.
  #    No token is minted, so there is nothing to intercept, replay or lose:
  #    the fragment carries only the fleet's PUBLIC key and says so itself.
  #    Defaults: one use, 24 hours. `--uses N` and `--expires 30m|24h|7d` widen it.
  stado fleet invite --name "$NAME" --offline

  # 2. the operator's own view: this invitation now reads
  #    `open (offline, awaiting address)` — waiting on a person, not a clock.
  stado fleet invites

  printf '%s\n' \
    "send the fragment above to whoever holds the machine," \
    "then rerun with the user@address their terminal printed:" \
    "  sh invite-a-machine.sh $NAME <user@address>"
  exit 0
fi

# 3. the address came back. Close the invitation with the ordinary
#    probe-then-write enrollment: it opens the channel the paste authorized,
#    reads hostname and uname over it, writes the entry from what it read, and
#    rolls that entry back if the agent install fails. No --install-key here:
#    the key is already in place, which is the whole point of the fragment.
#    There is no `fleet pending` step in this mode — an offline invitation
#    self-reports nothing, so this enrollment IS the registry write.
stado fleet enroll "$NAME" --ssh "$ADDRESS" --bootstrap

# 4. the invitation is `spent` now, and any other one still open can be closed
#    before its expiry.
stado fleet invites
# stado fleet revoke-invite <id>

# 5. the channel the fleet now owns, and the grants a reporting host needs
stado fleet key check "$NAME"
"$SB" token-mint stado-local-agent --scopes 'read:*'
"$SB" token-mint stado-host-health-beacon --scopes 'read:stado-host-health-api'

# 6. recovery program: beacon + managed units on the target
stado host recover "$NAME"

# 7. the proof: the machine reports
stado registry beacon-age

# ---------------------------------------------------------------------------
# The one-line mode, for a fleet that has published a control point. Three
# things must all hold on the host serving STADO_API_URL: the name resolves,
# an ingress fronts the loopback-bound dashboard, and the release there serves
# GET /api/fleet/invite/key, POST /api/fleet/join and GET /join.sh. Until then
# `stado fleet invite` reports which of those is missing and mints an offline
# invitation instead — it never prints a curl line it cannot stand behind.
#
#   export STADO_API_URL=<control-origin-the-machine-can-reach>
#   stado fleet invite --name "$NAME"    # prints the token ONCE, plus the line
#                                        # curl -fsSL <control>/join.sh | sh -s -- <id>.<secret>
#   stado fleet pending                  # the machine reports itself here
#   stado fleet pending --json |
#     jq -r '.pending[] | select(.destination != null) |
#            "\(.hostname)\t\(.target_name)\t\(.destination)\t\(.invite_id)\t\(.ssh_listening)"'
#   stado fleet approve <hostname-the-machine-reported>   # still probes; not a rubber stamp
#
# then steps 5 to 7 above are identical.
