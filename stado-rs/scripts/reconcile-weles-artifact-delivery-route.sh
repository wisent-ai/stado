#!/bin/sh
set -eu

umask 077
stado="$HOME/.stado/bin/stado"
if [ ! -x "$stado" ]; then
  printf '%s\n' 'Stado runtime is required' >&2
  exit 1
fi

stage_dir=$(/usr/bin/mktemp -d "$HOME/.stado/weles-artifact-delivery-route.XXXXXX")
trap '/bin/rm -rf "$stage_dir"' EXIT HUP INT TERM
current="$stage_dir/current.json"
updated="$stage_dir/updated.json"
"$stado" registry pull >"$current"

service='{
  "managed_service": "com.wisent.always-on.weles",
  "active_host": "charless-mac-mini",
  "endpoints": {"charless-mac-mini": {"url": "http://127.0.0.1:58101"}},
  "consumers": {"operator": {"capabilities": ["artifact-delivery"]}}
}'
adapter='{"bind": "127.0.0.1:17615", "consumer": "operator", "service": "weles-artifact-delivery"}'

if /usr/bin/jq -e --argjson service "$service" --argjson adapter "$adapter" '
  .service_directory.services["weles-artifact-delivery"] == $service
  and any(
    .targets[]
    | select(.name == "lukasz-macbook")
    | .service_resolver.adapters[];
    . == $adapter
  )
' "$current" >/dev/null
then
  printf '%s\n' 'Weles artifact delivery route already reconciled'
  exit 0
fi

/usr/bin/jq --argjson service "$service" --argjson adapter "$adapter" '
  .service_directory.generation += 1
  | .service_directory.services["weles-artifact-delivery"] = $service
  | (.targets[] | select(.name == "lukasz-macbook") | .service_resolver.adapters) |=
      ([.[]
        | select(
            .bind != "127.0.0.1:17615"
            and .service != "weles-artifact-delivery"
          )]
       + [$adapter])
' "$current" >"$updated"
"$stado" registry push "$updated"
