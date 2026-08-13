#!/bin/sh
# Take back the dedicated credential-lifecycle caller.
#
# The inverse of provision-credential-lifecycle-grant.sh. A lifecycle grant
# minted to answer one question is residue once the question is answered, and
# the script that made it re-makes it in one command when a rotation actually
# starts.
set -eu

consumer=${LIFECYCLE_CONSUMER:-azure-credential-lifecycle}
token_file=$HOME/.stado/$consumer-skarbiec-token
SKARBIEC_VAULT_FILE=${SKARBIEC_VAULT_FILE:-$HOME/.stado/skarbiec.vault.json}
export SKARBIEC_VAULT_FILE
PATH=/opt/homebrew/bin:/usr/local/bin:$PATH
export PATH
skarbiec=${SKARBIEC_BIN:-$HOME/.stado/bin/skarbiec}

"$skarbiec" token-revoke "$consumer" > /dev/null && printf 'revoked %s\n' "$consumer" ||
    printf 'no grant to revoke for %s\n' "$consumer"
/bin/rm -f "$token_file"
printf 'token file removed: %s\n' "$token_file"

printf '== remaining grants for %s ==\n' "$consumer"
"$skarbiec" tokens |
    /usr/bin/jq -r --arg c "$consumer" '.[] | select(.consumer==$c) | .consumer' |
    /usr/bin/sort -u || printf 'none\n'
