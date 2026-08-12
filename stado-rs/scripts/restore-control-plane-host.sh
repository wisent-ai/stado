#!/bin/sh
# Bring this host's control plane back up and do not return until it listens.
#
# A backgrounded process that is only spawned and abandoned dies with the SSH
# session that spawned it, and the endpoint stays dead while the caller reads
# success. This waits for the port before returning, so "started" means the
# fleet can reach it.
set -eu

binary="$HOME/.stado/bin/stado"
logs="$HOME/.stado/logs"
skarbiec_url=http://127.0.0.1:8895
port=8765

# This host's configured object-API URL is the tailnet origin fronted by a
# proxy that loops straight back here, so with the process down it cannot boot
# itself. Forcing a local backend instead would move every write onto a disk
# no reader reads. The shared store is reachable without the loop over the
# forward this host already keeps.
WC_STORAGE_BACKEND=local
export WC_STORAGE_BACKEND

[ -x "$binary" ] || { printf '%s\n' "missing $binary" >&2; exit 1; }
/bin/mkdir -p "$logs"

listening() {
  /usr/sbin/lsof -nP -iTCP:"$port" -sTCP:LISTEN -Fc 2>/dev/null \
    | /usr/bin/grep -qx cstado
}

if listening; then
  printf '{"state":"already-listening","port":%s}\n' "$port"
  exit 0
fi

WC_SKARBIEC_URL="$skarbiec_url" /usr/bin/nohup "$binary" local-control-plane \
  < /dev/null >> "$logs/stado-local-control-plane.log" 2>&1 &
pid=$!

waited=0
while [ "$waited" -lt 150 ]; do
  if listening; then
    printf '{"state":"listening","pid":%s,"port":%s,"waited_seconds":%s}\n' \
      "$pid" "$port" "$waited"
    exit 0
  fi
  if ! /bin/kill -0 "$pid" 2>/dev/null; then
    printf '%s\n' "control plane exited during startup; see $logs/stado-local-control-plane.log" >&2
    /usr/bin/tail -n 20 "$logs/stado-local-control-plane.log" >&2 || true
    exit 1
  fi
  /bin/sleep 2
  waited=$((waited + 2))
done

printf '%s\n' "control plane did not bind $port within ${waited}s" >&2
/usr/bin/tail -n 20 "$logs/stado-local-control-plane.log" >&2 || true
exit 1
