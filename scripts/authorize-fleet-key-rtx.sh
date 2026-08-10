#!/bin/sh
# Authorize one dedicated Stado fleet key on this host, additively.
#
# Run through `stado host install-helper` + `run-helper`, which passes no
# arguments, so the public key is pinned in this file. Only the public half is
# here: it is not a secret, and pinning it makes the authorization auditable in
# git rather than typed into a shell.
#
# Additive and idempotent by design. It never removes a line, so the operator's
# own key keeps working and a failed rotation cannot lock anyone out. Removing a
# superseded key is a separate, later decision.
set -eu

FLEET_KEY='ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIPyHIVQjo+gdTaW9CCM9eliYum2WVEJ1Tp2TF+Nd38O4 stado-fleet-ubuntu-server-rtx-pro-6000'

SSH_DIR="$HOME/.ssh"
AUTHORIZED="$SSH_DIR/authorized_keys"

mkdir -p "$SSH_DIR"
chmod 700 "$SSH_DIR"
[ -f "$AUTHORIZED" ] || : >"$AUTHORIZED"
chmod 600 "$AUTHORIZED"

material=$(printf '%s' "$FLEET_KEY" | cut -d' ' -f2)

if grep -q -- "$material" "$AUTHORIZED"; then
  printf 'already authorized\n'
else
  printf '%s\n' "$FLEET_KEY" >>"$AUTHORIZED"
  printf 'authorized\n'
fi

printf 'authorized_keys lines: %s\n' "$(wc -l <"$AUTHORIZED" | tr -d ' ')"
printf 'fingerprints now present:\n'
ssh-keygen -l -f "$AUTHORIZED" 2>/dev/null | sed 's/^/  /'
