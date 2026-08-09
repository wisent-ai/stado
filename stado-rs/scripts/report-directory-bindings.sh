#!/bin/sh
# Which vault on this host carries sealed directory bindings, and for what?
#
# The Microsoft rotation was prepared against "the fleet vault", not the host
# vault this operator's tools default to, so the item under rotation is not
# where a first look expects it. This lists every vault file and the items in
# it that carry a sealed `directory` block, with the sealed provider, tenant
# and account. No secret and no password field is read.
set -eu

for vault in "$HOME"/.stado/*.vault.json; do
    [ -f "$vault" ] || continue
    printf '== %s ==\n' "$vault"
    /usr/bin/jq -r '
        (.items // {}) | to_entries[]
        | select(.value.directory != null)
        | "\(.key)\t\(.value.directory.provider // "-")\t\(.value.directory.tenant_id // "-")\t\(.value.directory.account_upn // "-")\t\(.value.directory.sealed_at // "-")"
    ' "$vault" || printf 'unreadable\n'
done
