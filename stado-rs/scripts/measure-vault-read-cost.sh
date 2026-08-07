#!/bin/sh
# What the object-auth sweep costs in wall time, measured rather than assumed.
#
# `stado doctor` reports "probe did not answer within 8s" for object-auth once
# the four verifier grants are aligned. Before touching the deadline, measure
# the work: every mapped item is one gpg decryption in the vault, and the
# service verifier re-reads the object and release sets to prove the bearers
# are distinct, so those two sets are decrypted twice per probe.
set -eu

config=${STADO_CONFIG:-$HOME/.config/stado/config.json}
export SKARBIEC_VAULT_FILE=${SKARBIEC_VAULT_FILE:-$HOME/.stado/skarbiec.vault.json}
skarbiec=$HOME/.stado/bin/skarbiec

items=$(/usr/bin/jq -r '[.object_api.namespaces[].item] | .[]' "$config")
count=$(printf '%s\n' "$items" | /usr/bin/wc -l | /usr/bin/tr -d ' ')

started=$(/bin/date +%s)
for item in $items; do
    "$skarbiec" get "$item" >/dev/null
done
finished=$(/bin/date +%s)

printf 'object namespace items : %s\n' "$count"
printf 'serial read of that set: %s s\n' "$((finished - started))"
printf 'sets read per probe    : object, release, machine, service, then object and release again\n'
