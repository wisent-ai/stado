#!/bin/sh
# Can this host reach the canonical Skarbiec its credential subsystem needs?
#
# `credential status|adopt|rotate` resolve one owner-owned forward file and
# refuse everything when the port behind it is closed. This prints the declared
# endpoint, whether it answers, and the managed state of the Microsoft item.
# No secret is printed.
set -eu

forward=$HOME/.stado/forwards/skarbiec.local
item=${CREDENTIAL_ITEM:-platform-admin-azure}
SKARBIEC_VAULT_FILE=${SKARBIEC_VAULT_FILE:-$HOME/.stado/skarbiec.vault.json}
export SKARBIEC_VAULT_FILE
skarbiec=${SKARBIEC_BIN:-$HOME/.stado/bin/skarbiec}

if [ -r "$forward" ]; then
    endpoint=$(/bin/cat "$forward")
    printf 'declared canonical endpoint: %s\n' "$endpoint"
    /usr/bin/curl -sS -o /dev/null -w 'answers -> %{http_code}\n' --max-time 5 "$endpoint/v1/items/list" ||
        printf 'answers -> unreachable\n'
else
    printf 'no canonical forward file at %s\n' "$forward"
fi

printf '== listeners ==\n'
/usr/sbin/lsof -nP -iTCP -sTCP:LISTEN | /usr/bin/grep -i skarbiec || printf 'no skarbiec listener\n'

printf '== credential status %s ==\n' "$item"
"$skarbiec" credential status "$item" --as local-operator \
    --token-file "$HOME/.stado/local-operator-skarbiec-token" || printf 'status refused\n'
