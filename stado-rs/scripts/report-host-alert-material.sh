#!/bin/sh
# Report whether this host's vault holds any alert-channel material.
#
# `stado doctor` fails alerts because the local most-twilio item declares five
# fields that are all empty. Before calling that a value only the operator
# holds, look on the hosts that actually run the coordinator. Field lengths
# only: no secret is ever printed.
set -eu

vault=${SKARBIEC_VAULT_FILE:-$HOME/.stado/skarbiec.vault.json}
skarbiec=${SKARBIEC_BIN:-$HOME/.stado/bin/skarbiec}
export SKARBIEC_VAULT_FILE="$vault"

if [ ! -f "$vault" ]; then
    printf 'no vault at %s\n' "$vault"
elif [ ! -x "$skarbiec" ]; then
    printf 'no skarbiec binary at %s\n' "$skarbiec"
else
    printf 'candidates '
    "$skarbiec" list |
        /usr/bin/jq -c '[.[] | select(.id | test("most|twilio|alert|slack|telegram|sendgrid|sms")) | .id]'
    for item in most-twilio stado-alerts; do
        "$skarbiec" get "$item" |
            /usr/bin/jq -c --arg item "$item" \
                '{item: $item, fields: (.fields | to_entries | map({key, len: ((.value|tostring)|length)}))}'
    done
fi
