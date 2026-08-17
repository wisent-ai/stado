#!/bin/sh
# Exercise the object verifier's two startup calls without exposing its bearer.
set -eu
PATH="/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
export PATH
config=${STADO_CONFIG:-$HOME/.config/stado/config.json}
token_file=${WC_OBJECT_SKARBIEC_TOKEN_FILE:-$HOME/.stado/stado-object-api-verifier-skarbiec-token}
url=${WC_OBJECT_SKARBIEC_URL:-$(jq -r '.object_api.skarbiec.url // .skarbiec.url // "http://127.0.0.1:8895"' "$config")}
consumer=${WC_OBJECT_SKARBIEC_CONSUMER:-stado-object-api-verifier}
[ -s "$token_file" ]
token=$(cat "$token_file")
status=$(curl -sS -o "$HOME/.stado/.object-verifier-list.$$" -w '%{http_code}' -X POST -H "X-Consumer: $consumer" -H "Authorization: Bearer $token" -H 'Content-Type: application/json' --data '{}' "$url/v1/items/list")
printf 'list HTTP %s\n' "$status"
if [ "$status" != 200 ]; then cat "$HOME/.stado/.object-verifier-list.$$"; printf '\n'; fi
rm -f "$HOME/.stado/.object-verifier-list.$$"
for item in $(jq -r '[.object_api.namespaces[].item] | unique | sort | .[]' "$config")
do
    status=$(curl -sS -o /dev/null -w '%{http_code}' -X POST -H "X-Consumer: $consumer" -H "Authorization: Bearer $token" -H 'Content-Type: application/json' --data "{\"id\":\"$item\",\"field\":\"token\"}" "$url/v1/items/read")
    printf 'read %s HTTP %s\n' "$item" "$status"
done
