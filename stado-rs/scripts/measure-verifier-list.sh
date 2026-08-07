#!/bin/sh
# Time the endpoint each validator calls first: /v1/items/list per verifier.
#
# Reads are fast and gpg is fast, so an 8s object-auth deadline being exceeded
# points at the listing step rather than the per-item reads. Every validator
# lists before it compares its mapped set, so four listings happen per probe.
set -eu

config=${STADO_CONFIG:-$HOME/.config/stado/config.json}

for section in object_api release_api machine_api service_api; do
    consumer=$(/usr/bin/jq -r ".${section}.skarbiec.consumer // empty" "$config")
    [ -n "$consumer" ] || continue
    token_file=$(/usr/bin/jq -r ".${section}.skarbiec.token_file" "$config" | /usr/bin/sed "s|^~|$HOME|")
    url=$(/usr/bin/jq -r ".${section}.skarbiec.url // empty" "$config")
    [ -n "$url" ] || url=$(/usr/bin/jq -r '.object_api.skarbiec.url' "$config")
    code_and_time=$(/usr/bin/curl -sS -o /dev/null \
        -w '%{http_code} %{time_total}' \
        -H "Authorization: Bearer $(/bin/cat "$token_file")" \
        -H "X-Skarbiec-Consumer: $consumer" \
        "$url/v1/items/list" || printf 'curl-failed -')
    printf '%-12s %-32s %s\n' "$section" "$consumer" "$code_and_time"
done
