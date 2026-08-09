#!/bin/sh
# Which credentials already carry a sealed directory contract, in every vault?
#
# The seal is stored as its own item, `directory:credential/<id>`, and is only
# mirrored into the credential when that credential is already live. Looking at
# the credential alone therefore reports "no contract" for an item that was
# sealed hours ago, which is exactly how a finished step looks unfinished.
set -eu

for vault in $(/usr/bin/find "$HOME/.stado" -maxdepth 2 -name '*.vault.json'); do
    printf '== %s ==\n' "$vault"
    /usr/bin/jq -r '
        (.items // {}) | keys[]
        | select(startswith("directory:credential/"))
        | sub("^directory:credential/"; "sealed credential: ")
    ' "$vault" || printf 'unreadable\n'
done
