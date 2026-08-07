#!/bin/sh
# Wall time of the path the doctor actually uses: verifier -> HTTP -> Skarbiec.
#
# Reading the same items straight from the vault file takes under a second, so
# if object-auth still exceeds its deadline the cost is in the listener, not in
# gpg. This times the object verifier reading its whole mapped set over HTTP,
# exactly as validate_object_verifier does.
set -eu

config=${STADO_CONFIG:-$HOME/.config/stado/config.json}
stado=${STADO_BIN:-$PWD/target/debug/stado}

url=$(/usr/bin/jq -r '.object_api.skarbiec.url' "$config")
consumer=$(/usr/bin/jq -r '.object_api.skarbiec.consumer' "$config")
token_file=$(/usr/bin/jq -r '.object_api.skarbiec.token_file' "$config" | /usr/bin/sed "s|^~|$HOME|")
items=$(/usr/bin/jq -r '[.object_api.namespaces[].item] | .[]' "$config")
count=$(printf '%s\n' "$items" | /usr/bin/wc -l | /usr/bin/tr -d ' ')

printf 'verifier %s -> %s\n' "$consumer" "$url"

started=$(/bin/date +%s)
for item in $items; do
    WC_SKARBIEC_URL="$url" \
    WC_SKARBIEC_CONSUMER="$consumer" \
    WC_SKARBIEC_TOKEN_FILE="$token_file" \
        "$stado" secrets get "$item" --field token >/dev/null || printf 'refused: %s\n' "$item"
done
finished=$(/bin/date +%s)

printf 'items          : %s\n' "$count"
printf 'serial over http: %s s\n' "$((finished - started))"
