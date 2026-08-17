#!/bin/sh
# Report the object verifier grant's shape without revealing any credential.
set -eu
umask 077
PATH="/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
export PATH

config=${STADO_CONFIG:-$HOME/.config/stado/config.json}
skarbiec_bin=${SKARBIEC_BIN:-$HOME/.stado/bin/skarbiec}
vault_file=${SKARBIEC_VAULT_FILE:-$HOME/.stado/skarbiec.vault.json}
[ -f "$config" ]
[ -x "$skarbiec_bin" ]
[ -f "$vault_file" ]
stage_dir=$(mktemp -d "$HOME/.stado/.object-verifier-report.XXXXXX")
trap 'rm -rf "$stage_dir"' EXIT HUP INT TERM
jq -r '[.object_api.namespaces[].item] | unique | sort | .[]' "$config" >"$stage_dir/expected"
SKARBIEC_VAULT_FILE="$vault_file" "$skarbiec_bin" tokens \
    | jq -r '.[] | select(.consumer == "stado-object-api-verifier") | .capabilities[] | select(.action == "read" and .field == "token") | .item' \
    | sort -u >"$stage_dir/actual"

printf '%s\n' 'expected:'
cat "$stage_dir/expected"
printf '%s\n' 'actual:'
cat "$stage_dir/actual"
printf '%s\n' 'missing:'
comm -23 "$stage_dir/expected" "$stage_dir/actual"
printf '%s\n' 'unexpected:'
comm -13 "$stage_dir/expected" "$stage_dir/actual"
: >"$stage_dir/token-digests"
printf '%s\n' 'token fields:'
while IFS= read -r item
do
    value=$(SKARBIEC_VAULT_FILE="$vault_file" "$skarbiec_bin" get "$item")
    state=$(printf '%s\n' "$value" | jq -r '
        if (.fields.token // null) == null then "missing"
        elif (.fields.token | type) == "string" and .fields.token == "" then "empty"
        elif (.fields.token | type) == "object" and (.fields.token.value // "") == "" then "empty"
        else "present"
        end
    ')
    token=$(printf '%s\n' "$value" | jq -r '
        if (.fields.token | type) == "object" then (.fields.token.value // "")
        else (.fields.token // "")
        end
    ')
    [ -z "$token" ] || printf '%s %s\n' "$(printf '%s' "$token" | shasum -a 256 | cut -d' ' -f1)" "$item" >>"$stage_dir/token-digests"
    printf '%s %s\n' "$item" "$state"
done <"$stage_dir/expected"
printf '%s\n' 'duplicate token owners:'
sort "$stage_dir/token-digests" | cut -d' ' -f1 | uniq -d | while IFS= read -r digest
do
    grep "^$digest " "$stage_dir/token-digests" | cut -d' ' -f2-
done
