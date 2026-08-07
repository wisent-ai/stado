#!/bin/sh
# What does the broker's item listing actually carry per item?
#
# `stado doctor` warns that every whole-item read fails against this broker,
# and the repair depends on whether the listing names an item's fields: if it
# does, the client can rebuild an item from per-field reads instead of every
# caller being rewritten. Keys only; no values are requested or printed.
set -eu

config=${STADO_CONFIG:-$HOME/.config/stado/config.json}
url=$(/usr/bin/jq -r '.secrets.skarbiec.url' "$config")
token=$(/bin/cat "$HOME/.stado/local-operator-skarbiec-token")

/usr/bin/curl -sS -X POST "$url/v1/items/list" \
    -H "Authorization: Bearer $token" \
    -H "X-Consumer: local-operator" \
    -H 'Content-Type: application/json' \
    --data '{}' |
    /usr/bin/jq -c 'if type == "object" then (.items // .) else . end
                    | if type == "array" then .[:1] else . end
                    | map(keys)'
