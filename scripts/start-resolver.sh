#!/bin/sh
# Start this host's Stado resolver if its API port is not already answering.
#
# The registry declares `service_resolver` for a host -- an API bind and the
# stable-port adapters every co-located workload dials -- but no unit guarantees
# the process that serves them is running. When it is not, the failure surfaces
# far away and unrecognisably: Brama on this host could not read its own service
# identity because `http://127.0.0.1:17612/v1/items/read` refused the connection,
# which reads as a Skarbiec outage and is a missing resolver.
#
# Idempotent: an answering API port means there is nothing to do. The process is
# detached so the helper channel that started it can close.
set -eu

target=${STADO_RESOLVER_TARGET:-$(hostname -s)}
bin=${STADO_BIN:-$HOME/.stado/bin/stado}
api=${STADO_RESOLVER_API:-127.0.0.1:17600}
log=${STADO_RESOLVER_LOG:-$HOME/.stado/logs/stado-resolver.err}

port=${api##*:}
if /usr/sbin/lsof -nP -iTCP:"$port" -sTCP:LISTEN >/dev/null 2>&1; then
  printf 'resolver already listening on %s\n' "$api"
  exit 0
fi

[ -x "$bin" ] || {
  printf 'no stado binary at %s\n' "$bin" >&2
  exit 1
}

mkdir -p "$(dirname -- "$log")"
nohup "$bin" resolver serve --target "$target" >>"$log" 2>&1 &
started=$!

attempt=0
while [ "$attempt" -lt 100 ]; do
  if /usr/sbin/lsof -nP -iTCP:"$port" -sTCP:LISTEN >/dev/null 2>&1; then
    printf 'resolver started for %s on %s (pid %s)\n' "$target" "$api" "$started"
    exit 0
  fi
  attempt=$((attempt + 1))
  sleep 0.1
done
printf 'resolver did not bind %s within ten seconds\n' "$api" >&2
printf -- '--- %s ---\n' "$log" >&2
tail -n 15 "$log" >&2 || true
exit 1
