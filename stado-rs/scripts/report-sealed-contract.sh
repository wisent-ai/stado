#!/bin/sh
# What does one sealed directory contract say, and what does its credential
# carry?
#
# Answers where a rotation actually stands: the seal names the provider,
# tenant, principal and account; the credential's field names say whether the
# provider contract can be honoured yet. Field names and lengths only, never a
# value.
set -eu

credential=${CREDENTIAL_ITEM:-weles-microsoft-jakub-wisent-ai-password}
vault=${SKARBIEC_VAULT_FILE:-$HOME/.stado/weles-skarbiec.vault.json}
export SKARBIEC_VAULT_FILE="$vault"
# A helper runs with launchd's PATH, which carries no Homebrew: without this
# every vault read fails as `spawn gpg: No such file or directory` and reads
# like a missing item rather than a missing binary.
PATH=/opt/homebrew/bin:/usr/local/bin:$PATH
export PATH
skarbiec=${SKARBIEC_BIN:-$HOME/.stado/bin/skarbiec}

printf '== sealed contract ==\n'
"$skarbiec" get "directory:credential/$credential" |
    /usr/bin/jq -c '.fields // .' || printf 'unreadable\n'

printf '== credential fields ==\n'
"$skarbiec" get "$credential" |
    /usr/bin/jq -r '.fields | to_entries[] | "\(.key)\t\((.value|tostring)|length)"' ||
    printf 'credential item absent\n'

printf '== grants on the credential ==\n'
"$skarbiec" tokens |
    /usr/bin/jq -r --arg c "$credential" '.[] | . as $t | ($t.capabilities // [])[]
        | select((.item // "") == $c)
        | "\($t.consumer)\t\(.action):\(.item)#\(.field // "-")"' | /usr/bin/sort -u
