#!/bin/sh
# The fleet's object API: the one process that reads the disk store directly and
# serves it to every other reader.
#
# Its listening port comes from its OWN declaration, never from
# `storage.stado.url`. That URL is a CLIENT address: on the registry authority
# it happens to name this service, but on an operator laptop it names the
# resolver adapter, so deriving the bind from it made this service try to take
# the port the resolver already holds -- "Address already in use", every retry,
# with nothing listening for anyone.
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
# `object_api.port` is this service's own statement about where it listens.
# Absent, the historical port stands: 18765 on a member host, and the authority
# host declares 8765 explicitly, which is what its tailnet ingress maps.
STADO_OBJECT_PORT="${STADO_OBJECT_PORT:-$(/usr/bin/python3 -c \
  'import json, sys; print((json.load(open(sys.argv[1])).get("object_api") or {}).get("port") or "")' \
  "$STADO_CONFIG")}"
STADO_OBJECT_PORT="${STADO_OBJECT_PORT:-18765}"
case "$STADO_OBJECT_PORT" in
  ''|*[!0-9]*)
    printf '%s\n' "object_api.port is not a port number in $STADO_CONFIG" >&2
    exit 1
    ;;
esac
# Refuse the one address that cannot be right: a port this host publishes as a
# resolver adapter belongs to the resolver, and binding it here would replace a
# working client route with a service that answers the wrong questions.
if /usr/bin/python3 -c 'import json, sys
config = json.load(open(sys.argv[1]))
port = sys.argv[2]
targets = (config.get("service_resolver") or {}).get("adapters") or []
sys.exit(0 if any(str(entry.get("bind", "")).endswith(":" + port) for entry in targets) else 1)' \
  "$STADO_CONFIG" "$STADO_OBJECT_PORT" 2>/dev/null; then
  printf '%s\n' "refusing to bind $STADO_OBJECT_PORT: this host publishes it as a resolver adapter" >&2
  exit 1
fi
exec "$HOME/.stado/bin/stado" dashboard --bind 127.0.0.1 --port "$STADO_OBJECT_PORT"
