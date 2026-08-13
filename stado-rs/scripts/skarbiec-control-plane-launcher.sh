#!/bin/sh
# Local operator Skarbiec for the control plane.
#
# The registry's declared skarbiec endpoint on this host (8787) is occupied by
# the SSH forward that carries brama and the mini's vault, so the operator
# vault -- the one holding every control-plane verifier consumer -- listens
# beside it. `object_api.skarbiec.url` and `secrets.skarbiec.url` name this
# port, so the dashboard resolves it from config with no environment override.
set -eu
umask 077
PATH="/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin"
export PATH
GNUPGHOME="${GNUPGHOME:-$HOME/.gnupg}"
export GNUPGHOME
SKARBIEC_VAULT_FILE="${SKARBIEC_VAULT_FILE:-$HOME/.stado/skarbiec.vault.json}"
export SKARBIEC_VAULT_FILE
[ -f "$SKARBIEC_VAULT_FILE" ] || {
  printf '%s\n' "operator vault is absent: $SKARBIEC_VAULT_FILE" >&2
  exit 1
}
exec "$HOME/.stado/bin/skarbiec" serve --port 8799
