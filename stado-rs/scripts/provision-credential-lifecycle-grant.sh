#!/bin/sh
# Give the credential lifecycle its own least-privilege caller.
#
# `credential status|adopt|rotate` refuse with `lifecycle:<item> grant required`
# and the vault holds no such grant. Adding it to `local-operator` would rotate
# that consumer's token - the admin identity every other tool on this host
# uses - so this mints a dedicated consumer that can do exactly one thing on
# exactly one item, and writes its token to an owner-only file.
#
# Idempotent: an existing working token file is reported and left alone. The
# token value is never printed.
#
# Undo: skarbiec token-revoke <consumer>, and remove the token file.
set -eu

item=${CREDENTIAL_ITEM:-platform-admin-azure}
consumer=${LIFECYCLE_CONSUMER:-azure-credential-lifecycle}
token_file=$HOME/.stado/$consumer-skarbiec-token
SKARBIEC_VAULT_FILE=${SKARBIEC_VAULT_FILE:-$HOME/.stado/skarbiec.vault.json}
export SKARBIEC_VAULT_FILE
skarbiec=${SKARBIEC_BIN:-$HOME/.stado/bin/skarbiec}

if [ -s "$token_file" ]; then
    printf 'token file already present for %s\n' "$consumer"
else
    minted=$("$skarbiec" token-mint "$consumer" --capabilities "lifecycle:$item")
    printf '%s' "$minted" | /usr/bin/jq -r '.token' > "$token_file"
    /bin/chmod u=rw,go= "$token_file"
    printf 'minted %s with %s capability(ies)\n' "$consumer" \
        "$(printf '%s' "$minted" | /usr/bin/jq -r '.capabilities | length')"
fi

printf '== granted actions ==\n'
"$skarbiec" tokens |
    /usr/bin/jq -r --arg c "$consumer" '.[] | select(.consumer==$c) | (.capabilities // [])[]
        | "\(.action):\(.item)#\(.field // "-")"' | /usr/bin/sort -u

printf '== credential status %s ==\n' "$item"
"$skarbiec" credential status "$item" --as "$consumer" --token-file "$token_file" ||
    printf 'status refused\n'
