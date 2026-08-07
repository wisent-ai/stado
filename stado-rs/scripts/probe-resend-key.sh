#!/bin/sh
# Which stored Resend key does the provider still accept, and for which domain?
#
# `stado alerts send` came back with "API key is invalid", so the question is
# whether the vault holds a live key at all. `GET /domains` answers that
# without sending anything. Status and verified domains only; the key itself
# is read into a variable and never printed.
#
# Usage: probe-resend-key.sh <item> <field>
set -eu

SKARBIEC_VAULT_FILE=${SKARBIEC_VAULT_FILE:-$HOME/.stado/skarbiec.vault.json}
export SKARBIEC_VAULT_FILE
skarbiec=${SKARBIEC_BIN:-$HOME/.stado/bin/skarbiec}

item=$1
field=$2
key=$("$skarbiec" get "$item" | /usr/bin/jq -r --arg field "$field" '.fields[$field]')

printf '%s#%s -> ' "$item" "$field"
/usr/bin/curl -sS -w 'status %{http_code}\n' \
    -H "Authorization: Bearer $key" \
    https://api.resend.com/domains |
    /usr/bin/jq -c 'if type == "object" and has("data")
                    then {domains: [.data[] | {name, status}]}
                    else . end' -R -r
