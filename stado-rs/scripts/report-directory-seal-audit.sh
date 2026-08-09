#!/bin/sh
# What has this host's Skarbiec recorded about directory seals and adoptions?
#
# `seal-directory` writes the seal as its own vault item and appends an audit
# row naming the credential, provider, tenant, principal object id and account.
# That row is the only place those values sit in the clear, which is what makes
# a rotation resumable without asking anyone to remember them. Passwords never
# appear in the audit and none is read here.
set -eu

for audit in "$HOME"/.stado/*.audit.json "$HOME"/.stado/*.audit.jsonl; do
    [ -f "$audit" ] || continue
    printf '== %s ==\n' "$audit"
    /usr/bin/grep -aE 'credential-directory-sealed|credential-directory-resealed|credential-adopt|lifecycle' "$audit" |
        /usr/bin/tail | /usr/bin/cut -c1-400 || printf 'no credential lifecycle rows\n'
done
