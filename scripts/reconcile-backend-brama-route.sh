#!/usr/bin/env bash
set -euo pipefail

stado_bin="${STADO_BIN:-$HOME/.stado/bin/stado}"
host="${1:-charless-mac-mini}"
consumer="wisent-backend"
bind="127.0.0.1:17604"
resolver_service="com.wisent.stado-resolver"
work="$HOME/.stado/work/registry-edits/backend-brama-route"
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

adapter_listening() {
  "$stado_bin" host exec "$host" --json -- lsof -nP -iTCP -sTCP:LISTEN |
    jq -e --arg endpoint "$bind" '
      .exit_code == 0
      and (.stdout | split("\n") | any(
        startswith("stado ")
        and contains("TCP " + $endpoint + " (LISTEN)")
      ))
    ' >/dev/null
}

# One read hands back the document AND the generation it is at. Two pulls
# would pair a token with a document it does not describe.
read_registry() {
  "$stado_bin" registry pull --with-generation > "$pull"
  jq '.document' "$pull" > "$before"
}

# The reconcile transform, pure in its input document: re-running it against a
# freshly read registry is what makes a conflict retryable.
apply_route() {
  jq --arg host "$host" --arg consumer "$consumer" --arg bind "$bind" '
    . as $before
    | (.release_control.products.brama.targets[$host].stable_bind
        // error("Brama stable bind is not declared for " + $host)) as $stable_bind
    | ("http://" + $stable_bind) as $stable_url
    | ([.targets[] | select(.name == $host)
        | .service_resolver.adapters[]
        | select(.service == "brama" and .consumer == $consumer)] | length) as $route_count
    | if $route_count > 1 then
        error("multiple Brama resolver adapters exist for " + $consumer + " on " + $host)
      elif $route_count == 0 then
        (.targets[] | select(.name == $host) | .service_resolver.adapters) += [{
          service: "brama",
          consumer: $consumer,
          bind: $bind,
          connect_seconds: 10,
          idle_seconds: 600
        }]
      else
        (.targets[] | select(.name == $host) | .service_resolver.adapters[]
          | select(.service == "brama" and .consumer == $consumer)) |= (. + {
            bind: $bind,
            connect_seconds: 10,
            idle_seconds: 600
          })
      end
    | .service_directory.services.brama.endpoints[$host].url = $stable_url
    | if . == $before then . else .service_directory.generation += 1 end
  ' "$before" > "$after"

  jq -e --arg host "$host" --arg consumer "$consumer" --arg bind "$bind" '
    ([.targets[] | select(.name == $host) | .service_resolver.adapters[]
      | select(.service == "brama" and .consumer == $consumer
        and .bind == $bind and .connect_seconds == 10 and .idle_seconds == 600)]
      | length) == 1
    and (.service_directory.services.brama.consumers[$consumer].capabilities
      | index("model-routing") != null)
    and (.service_directory.services.brama.endpoints[$host].url
      == ("http://" + .release_control.products.brama.targets[$host].stable_bind))
  ' "$after" >/dev/null
}

attempt=1
while :; do
  read_registry
  generation="$(jq -r '.generation' "$pull")"
  apply_route

  if cmp -s "$before" "$after"; then
    printf '%s\n' "Brama route for $consumer on $host is already declared"
    break
  fi

  status=0
  "$stado_bin" registry push "$after" --if-generation "$generation" || status=$?
  if [ "$status" -eq 0 ]; then
    printf '%s\n' "Reconciled Brama route for $consumer on $host at $bind"
    break
  fi
  if [ "$status" -ne "$registry_conflict_exit" ]; then
    exit "$status"
  fi
  # Somebody published between the read and the write. The other document is
  # the current one, so discard this edit, re-read and re-apply. --force would
  # not help: it waves past the deleted-key guard, not past a moved generation.
  if [ "$attempt" -ge "$push_attempts" ]; then
    printf '%s\n' "registry generation kept moving under $push_attempts attempts; re-run" >&2
    exit "$registry_conflict_exit"
  fi
  printf '%s\n' "registry moved to a newer generation; re-reading (attempt $attempt)" >&2
  attempt=$((attempt + 1))
done

if ! adapter_listening; then
  "$stado_bin" service restart "$resolver_service" --host "$host" --json
fi

if ! adapter_listening; then
  printf '%s\n' "Brama adapter for $consumer is not listening at $bind" >&2
  exit 1
fi

printf '%s\n' "Brama adapter for $consumer is listening at $bind"
