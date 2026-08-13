#!/bin/sh
# Which consumers hold credential-lifecycle grants on this host's vault?
#
# `credential status` now reaches the canonical Skarbiec and is refused with
# `lifecycle:<item> grant required`. Before minting anything, look at what the
# vault already grants: consumers and capability actions only, never a token.
set -eu

SKARBIEC_VAULT_FILE=${SKARBIEC_VAULT_FILE:-$HOME/.stado/skarbiec.vault.json}
export SKARBIEC_VAULT_FILE
skarbiec=${SKARBIEC_BIN:-$HOME/.stado/bin/skarbiec}

printf '== grants mentioning lifecycle ==\n'
"$skarbiec" tokens |
    /usr/bin/jq -r '.[] | . as $t | ($t.capabilities // [])[]
        | select((.action // "") + ":" + (.item // "") | test("lifecycle";"i"))
        | "\($t.consumer)\t\(.action):\(.item)#\(.field // "-")"' |
    /usr/bin/sort -u || printf 'none\n'

printf '== grants touching the azure item ==\n'
"$skarbiec" tokens |
    /usr/bin/jq -r '.[] | . as $t | ($t.capabilities // [])[]
        | select((.item // "") | test("azure";"i"))
        | "\($t.consumer)\t\(.action):\(.item)#\(.field // "-")"' |
    /usr/bin/sort -u || printf 'none\n'
