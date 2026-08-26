#!/usr/bin/env bash
set -euo pipefail

stado_bin="${STADO_BIN:-$HOME/.stado/bin/stado}"
host="${1:-lukasz-macbook}"
control_host="${2:-charless-mac-mini}"
work="$HOME/.stado/work/registry-edits/object-adapter-timeouts"
mkdir -p "$work"
before="$work/before.json"
after="$work/after.json"
"$stado_bin" registry pull > "$before"

jq --arg host "$host" --arg control_host "$control_host" '
  (.targets[] | select(.name == $host) | .service_resolver.adapters[] |
   select(.service == "stado-object-api" and .consumer == "stado-local-agent")) |=
  (. + {connect_seconds: 300, idle_seconds: 300})
  | .service_directory.generation +=
      (if .service_directory.services["stado-object-api"].managed_service
          == "com.wisent.always-on.stado-object-api" then 0 else 1 end)
  | .service_directory.services["stado-object-api"].managed_service =
      "com.wisent.always-on.stado-object-api"
  | (.targets[] | select(.name == $control_host) | .services[] |
      select(.name == "com.wisent.always-on.stado-object-api")) |=
      (. + {
        program: "/Users/charles/.stado/services/com.wisent.always-on.stado-object-api/current/darwin-arm/stado",
        args: [
          "dashboard", "--bind", "127.0.0.1", "--port", "8765",
          "dashboard", "--bind", "127.0.0.1", "--port", "8765"
        ]
      })
' "$before" > "$after"

count="$(jq --arg host "$host" '[.targets[] | select(.name == $host) |
  .service_resolver.adapters[] | select(.service == "stado-object-api" and
  .consumer == "stado-local-agent" and .connect_seconds == 300 and
  .idle_seconds == 300)] | length' "$after")"
[ "$count" = 1 ] || { echo "expected exactly one object adapter on $host" >&2; exit 1; }
route="$(jq -r '.service_directory.services["stado-object-api"].managed_service' "$after")"
[ "$route" = "com.wisent.always-on.stado-object-api" ] || {
  echo "object API route does not name its managed autostart unit" >&2
  exit 1
}
program_count="$(jq --arg control_host "$control_host" '[.targets[] |
  select(.name == $control_host) | .services[] |
  select(.name == "com.wisent.always-on.stado-object-api" and
    .program == "/Users/charles/.stado/services/com.wisent.always-on.stado-object-api/current/darwin-arm/stado" and
    .args == [
      "dashboard", "--bind", "127.0.0.1", "--port", "8765",
      "dashboard", "--bind", "127.0.0.1", "--port", "8765"
    ])] | length' "$after")"
[ "$program_count" = 1 ] || { echo "object API service program is not declared once" >&2; exit 1; }
"$stado_bin" registry push "$after"
"$stado_bin" service ensure com.wisent.always-on.stado-object-api \
  --host "$control_host" \
  --reason "Release submission requires the declared object API autostart unit and live endpoint" \
  --as-daemon
