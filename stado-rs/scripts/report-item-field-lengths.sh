#!/bin/sh
# Print how long each field of an item actually is, without printing secrets.
#
# The listener answered `{"value":""}` with status 200 for every Twilio field,
# so the question is whether the vault holds empty values or the read path
# loses them. Lengths answer that and reveal nothing.
#
# Usage: report-item-field-lengths.sh <item> [<item> ...]
set -eu

SKARBIEC_VAULT_FILE=${SKARBIEC_VAULT_FILE:-$HOME/.stado/skarbiec.vault.json}
export SKARBIEC_VAULT_FILE
skarbiec=${SKARBIEC_BIN:-$HOME/.stado/bin/skarbiec}

for item in "$@"; do
    printf '=== %s\n' "$item"
    "$skarbiec" get "$item" |
        /usr/bin/jq -c '.fields | to_entries | map({key, len: ((.value|tostring)|length)})'
done
