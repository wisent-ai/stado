#!/bin/sh
# The fleet's object API on the always-on host.
#
# This endpoint served the whole fleet as an unmanaged process: nothing owned
# it, nothing restarted it, and it went on running a build old enough to write
# host beacons where the current readers no longer look. A TLS terminator in
# front of it publishes it on the tailnet; this process is the loopback origin
# behind that.
#
# The server owns the disk store and must not resolve storage through the
# object API, or it would recurse into its own socket. Everything else --
# verifier grants, namespaces, release publishers, the storage root -- comes
# from the host's own config file.
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
exec "$HOME/.stado/bin/stado" dashboard --bind 127.0.0.1 --port 8765
