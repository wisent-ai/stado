#!/bin/sh
# The fleet's object API. `storage.stado.url` and the tailnet ingress both name
# this port, and every remote agent reads the registry and the release channel
# through it.
#
# The server itself must not resolve storage through the object API, or it
# would recurse into its own socket; it is the one process that reads the disk
# store directly. Every other setting -- verifier grants, namespaces, release
# publishers -- comes from the config file, so this launcher declares nothing
# the control plane could disagree with.
set -eu
PATH="/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin"
export PATH
WC_STORAGE_BACKEND=local
export WC_STORAGE_BACKEND
STADO_CONFIG="${STADO_CONFIG:-$HOME/.config/stado/config.json}"
export STADO_CONFIG
[ -f "$STADO_CONFIG" ] || {
  printf '%s\n' "control-plane config is absent: $STADO_CONFIG" >&2
  exit 1
}
exec "$HOME/.stado/bin/stado" dashboard --bind 127.0.0.1 --port 18765
