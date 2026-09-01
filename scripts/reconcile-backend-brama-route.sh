#!/usr/bin/env bash
set -euo pipefail

stado_bin="${STADO_BIN:-$HOME/.stado/bin/stado}"
host="${1:-charless-mac-mini}"
consumer="wisent-backend"
bind="127.0.0.1:17604"
work="$HOME/.stado/work/registry-edits/backend-brama-route"
mkdir -p "$work"
before="$work/before.json"
after="$work/after.json"

"$stado_bin" registry pull > "$before"

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

if cmp -s "$before" "$after"; then
  printf '%s\n' "Brama route for $consumer on $host is already reconciled"
  exit 0
fi

"$stado_bin" registry push "$after"
printf '%s\n' "Reconciled Brama route for $consumer on $host at $bind"
