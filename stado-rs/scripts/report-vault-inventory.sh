#!/bin/sh
# List every vault item with its kind and field names. Names only, no values.
#
# Answers "does any channel material exist anywhere in this vault" without
# guessing at item naming: the alerts remedy needs a slack webhook, a telegram
# bot token, a sendgrid key, or Twilio material, and those can live under a
# product's own item name.
set -eu

SKARBIEC_VAULT_FILE=${SKARBIEC_VAULT_FILE:-$HOME/.stado/skarbiec.vault.json}
export SKARBIEC_VAULT_FILE
skarbiec=${SKARBIEC_BIN:-$HOME/.stado/bin/skarbiec}

"$skarbiec" list | /usr/bin/jq -r '.[] | "\(.id)\t\(.kind)\t\((.fields // []) | join(","))"'
