#!/bin/sh
# What does the credential subsystem say about the Microsoft identities?
#
# The rotation was reported as blocked on two operator-only steps: moving a
# field from `value` to `password`, and supplying the current password to
# `adopt`. This prints the managed state of every credential item and the
# field each one exposes, so "blocked" can be checked instead of remembered.
# No secret is printed.
set -eu

SKARBIEC_VAULT_FILE=${SKARBIEC_VAULT_FILE:-$HOME/.stado/skarbiec.vault.json}
export SKARBIEC_VAULT_FILE
skarbiec=${SKARBIEC_BIN:-$HOME/.stado/bin/skarbiec}

printf '== credential status ==\n'
"$skarbiec" credential status | /usr/bin/jq -c '.' || printf 'status unavailable\n'
