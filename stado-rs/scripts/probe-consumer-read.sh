#!/bin/sh
# Read one item field as one named consumer: status, and the value's length.
#
# Separates "this grant is refused" from "this item holds nothing". The
# listener answers 200 with an empty value for a declared-but-unpopulated
# field, which reads as a refusal from the caller's side unless the length is
# printed. Secrets are never shown; `X-Consumer` is the header the Stado
# client sends, and any other spelling is treated as an unknown consumer.
#
# Usage: probe-consumer-read.sh <consumer> <token-file> <item> <field>
set -eu

config=${STADO_CONFIG:-$HOME/.config/stado/config.json}
consumer=$1
token_file=$(printf '%s' "$2" | /usr/bin/sed "s|^~|$HOME|")
item=$3
field=$4

url=$(/usr/bin/jq -r '.secrets.skarbiec.url' "$config")
body=$(/usr/bin/mktemp)
trap '/bin/rm -f "$body"' EXIT

printf '%s reading %s#%s -> ' "$consumer" "$item" "$field"
/usr/bin/curl -sS -o "$body" -w 'status %{http_code}\n' \
    -X POST "$url/v1/items/read" \
    -H "Authorization: Bearer $(/bin/cat "$token_file")" \
    -H "X-Consumer: $consumer" \
    -H 'Content-Type: application/json' \
    --data "{\"id\":\"$item\",\"field\":\"$field\"}"
/usr/bin/jq -c '{value_len: ((.value // "") | length), error}' "$body"
