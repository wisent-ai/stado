#!/usr/bin/env bash
set -euo pipefail

stado_bin="${STADO_BIN:-$HOME/.stado/bin/stado}"
host="${1:-lukasz-macbook}"
control_host="${2:-charless-mac-mini}"
work="$HOME/.stado/work/registry-edits/object-adapter-timeouts"
mkdir -p "$work"
pull="$work/pull.json"
before="$work/before.json"
after="$work/after.json"

# `stado registry push --if-generation` exits this when somebody wrote first.
# See docs/examples/fleet/add-remove-host.sh for the canonical loop.
registry_conflict_exit=75
# Bounded re-read/re-apply rounds. An unconditional push erases whatever was
# published between this script's read and its write — the overwrite that
# destroyed a concurrent edit on 2026-09-01.
push_attempts=5

# The reconcile transform plus its checks, pure in the document just read, so
# a conflict is answered by re-reading and re-applying rather than by forcing.
apply_timeouts() {
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
          program: "/Users/charles/.stado/bin/stado",
          args: ["dashboard", "--bind", "127.0.0.1", "--port", "8765"]
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
      .program == "/Users/charles/.stado/bin/stado" and
      .args == ["dashboard", "--bind", "127.0.0.1", "--port", "8765"])] | length' "$after")"
  [ "$program_count" = 1 ] || { echo "object API service program is not declared once" >&2; exit 1; }
}

attempt=1
while :; do
  # One read hands back the document AND the generation it is at. Two pulls
  # would pair a token with a document it does not describe.
  "$stado_bin" registry pull --with-generation > "$pull"
  generation="$(jq -r '.generation' "$pull")"
  jq '.document' "$pull" > "$before"
  apply_timeouts

  if cmp -s "$before" "$after"; then
    printf '%s\n' "object adapter timeouts on $host are already declared"
    break
  fi

  status=0
  "$stado_bin" registry push "$after" --if-generation "$generation" || status=$?
  if [ "$status" -eq 0 ]; then
    break
  fi
  if [ "$status" -ne "$registry_conflict_exit" ]; then
    exit "$status"
  fi
  # Somebody published between the read and the write, so their document is
  # the current one: discard this edit and rebuild it on theirs. --force would
  # not help — it waves past the deleted-key guard, not a moved generation.
  if [ "$attempt" -ge "$push_attempts" ]; then
    printf '%s\n' "registry generation kept moving under $push_attempts attempts; re-run" >&2
    exit "$registry_conflict_exit"
  fi
  printf '%s\n' "registry moved to a newer generation; re-reading (attempt $attempt)" >&2
  attempt=$((attempt + 1))
done
"$stado_bin" service ensure com.wisent.always-on.stado-object-api \
  --host "$control_host" \
  --reason "Release submission requires the declared object API autostart unit and live endpoint" \
  --as-daemon
