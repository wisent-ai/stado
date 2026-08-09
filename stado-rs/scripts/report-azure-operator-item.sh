#!/bin/sh
# Does this host hold an Azure operator session, and which identity fields?
#
# `credential seal-directory` needs the tenant, the principal object id and the
# account UPN. Two of the three are in the record; the object id is not, and
# sealing a guessed identity would bind the credential to the wrong principal.
# This reports which identity fields the operator item carries, by name and
# length only, so the seal can be derived rather than invented.
set -eu

item=${AZURE_OPERATOR_ITEM:-stado-azure-operator}
SKARBIEC_VAULT_FILE=${SKARBIEC_VAULT_FILE:-$HOME/.stado/skarbiec.vault.json}
export SKARBIEC_VAULT_FILE
skarbiec=${SKARBIEC_BIN:-$HOME/.stado/bin/skarbiec}

printf '== items matching azure ==\n'
"$skarbiec" list | /usr/bin/jq -r '.[] | select(.id|test("azure|entra";"i")) | "\(.id)\t\(.kind)"'

printf '== fields of %s ==\n' "$item"
"$skarbiec" get "$item" |
    /usr/bin/jq -r '.fields | to_entries[] | "\(.key)\t\((.value|tostring)|length)"' ||
    printf 'absent\n'
