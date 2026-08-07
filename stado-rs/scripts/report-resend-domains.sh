#!/bin/sh
# Which sending domains does the live Resend account actually accept?
#
# An alert channel that resolves but whose sender domain is unverified fails
# at the moment it is needed. This prints name and status only.
#
# Usage: report-resend-domains.sh <item> <field>
set -eu

SKARBIEC_VAULT_FILE=${SKARBIEC_VAULT_FILE:-$HOME/.stado/skarbiec.vault.json}
export SKARBIEC_VAULT_FILE
skarbiec=${SKARBIEC_BIN:-$HOME/.stado/bin/skarbiec}

key=$("$skarbiec" get "$1" | /usr/bin/jq -r --arg field "$2" '.fields[$field]')
/usr/bin/curl -sS -H "Authorization: Bearer $key" https://api.resend.com/domains |
    /usr/bin/jq -r '.data[] | "\(.status)\t\(.name)"' | /usr/bin/sort
