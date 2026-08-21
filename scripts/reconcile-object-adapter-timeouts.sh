#!/usr/bin/env bash
set -euo pipefail

stado_bin="${STADO_BIN:-$HOME/.stado/bin/stado}"
host="${1:-lukasz-macbook}"
work="$HOME/.stado/work/registry-edits/object-adapter-timeouts"
mkdir -p "$work"
before="$work/before.json"
after="$work/after.json"
"$stado_bin" registry pull > "$before"

jq --arg host "$host" '
  (.targets[] | select(.name == $host) | .service_resolver.adapters[] |
   select(.service == "stado-object-api" and .consumer == "stado-local-agent")) |=
  (. + {connect_seconds: 300, idle_seconds: 300})
' "$before" > "$after"

count="$(jq --arg host "$host" '[.targets[] | select(.name == $host) |
  .service_resolver.adapters[] | select(.service == "stado-object-api" and
  .consumer == "stado-local-agent" and .connect_seconds == 300 and
  .idle_seconds == 300)] | length' "$after")"
[ "$count" = 1 ] || { echo "expected exactly one object adapter on $host" >&2; exit 1; }
"$stado_bin" registry push "$after"
