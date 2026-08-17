#!/usr/bin/env bash
# Report whether this host runs a queue consumer, and under what supervision.
#
# `release submit` selects a builder from live capacity publications, not from the
# registry's platform declaration: a host that declares `linux-amd64` but has not
# published capacity within CAPACITY_STALE_SECONDS is not a builder. With only the
# operator laptop publishing, a `linux-amd64` release cannot be built anywhere,
# and the error names the platform rather than the missing consumer.
#
# Read-only: unit states and the last publication attempt, no mutation.
set -euo pipefail

printf 'host %s\n' "$(hostname -s 2>/dev/null || hostname)"

# Which store this host publishes capacity into. `release submit` selects
# builders from capacity it can see, so two hosts pointed at different stores
# cannot see each other's workers no matter how healthy both are.
/usr/bin/env python3 - "${STADO_CONFIG:-$HOME/.config/stado/config.json}" <<'PY' 2>/dev/null || true
import json, sys

try:
    with open(sys.argv[1], encoding="utf-8") as handle:
        data = json.load(handle)
except OSError as error:
    print(f"config unreadable {error.filename}")
    raise SystemExit
storage = data.get("storage") or {}
stado = storage.get("stado") or {}
print(f"config {sys.argv[1]}")
print(f"storage_provider {storage.get('provider')}")
print(f"queue_store_url {stado.get('url')}")
print(f"queue_store_namespace {stado.get('namespace')}")
PY
printf 'stado_binary %s\n' "$([ -x "$HOME/.stado/bin/stado" ] && echo present || echo MISSING)"

if command -v systemctl >/dev/null; then
  printf -- '--- systemd units ---\n'
  for unit in wisent-agent.service stado-agent.service wisent-compute-agent.service; do
    state=$(systemctl is-active "$unit" 2>/dev/null || true)
    enabled=$(systemctl is-enabled "$unit" 2>/dev/null || true)
    printf '%s active=%s enabled=%s\n' "$unit" "${state:-unknown}" "${enabled:-unknown}"
  done
  printf -- '--- any wisent or stado unit ---\n'
  systemctl list-units --all --no-legend --plain --type=service 2>/dev/null \
    | awk '$1 ~ /wisent|stado/ {print "  " $1 " " $3 "/" $4}' | head -12
fi

printf -- '--- running processes ---\n'
ps -eo etime=,command= 2>/dev/null \
  | awk '$0 ~ /stado (agent|worker|queue)|wisent-agent/ {print "  " $0}' \
  | cut -c1-110 | head -6
printf 'process_matches %s\n' \
  "$(ps -eo command= 2>/dev/null | awk '/stado (agent|worker|queue)|wisent-agent/' | wc -l | tr -d ' ')"
