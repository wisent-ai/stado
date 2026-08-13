#!/bin/sh
set -eu

umask 077
stado="$HOME/.stado/bin/stado"
if [ ! -x "$stado" ]; then
  printf '%s\n' 'Stado runtime is required' >&2
  exit 1
fi
stage_dir=$(/usr/bin/mktemp -d "$HOME/.stado/weles-object-route.XXXXXX")
trap '/bin/rm -rf "$stage_dir"' EXIT HUP INT TERM
current="$stage_dir/current.json"
updated="$stage_dir/updated.json"
"$stado" registry pull >"$current"

if /usr/bin/jq -e '
  .service_directory.services["stado-object-api"] == {
    "managed_service": "com.wisent.always-on.weles",
    "active_host": "charless-mac-mini",
    "endpoints": {"charless-mac-mini": {"url": "http://127.0.0.1:8765"}},
    "consumers": {"weles": {"capabilities": ["object-storage"]}}
  }
  and any(
    .targets[]
    | select(.name == "charless-mac-mini")
    | .service_resolver.adapters[];
    . == {"bind": "127.0.0.1:17602", "consumer": "weles", "service": "skarbiec-weles"}
  )
  and any(
    .targets[]
    | select(.name == "charless-mac-mini")
    | .service_resolver.adapters[];
    . == {"bind": "127.0.0.1:17603", "consumer": "weles", "service": "stado-object-api"}
  )
  and all(
    .targets[]
    | select(.name == "charless-mac-mini")
    | .service_resolver.adapters[];
    .bind != "127.0.0.1:17613"
  )
' "$current" >/dev/null
then
  printf '%s\n' 'Weles object route already reconciled'
  exit 0
fi

/usr/bin/jq '
  .service_directory.generation += 1
  | .service_directory.services["stado-object-api"] = {
      "managed_service": "com.wisent.always-on.weles",
      "active_host": "charless-mac-mini",
      "endpoints": {"charless-mac-mini": {"url": "http://127.0.0.1:8765"}},
      "consumers": {"weles": {"capabilities": ["object-storage"]}}
    }
  | (.targets[] | select(.name == "charless-mac-mini") | .service_resolver.adapters) |=
      ([.[]
        | select(
            .service != "stado-object-api"
            and .service != "skarbiec-weles"
            and .bind != "127.0.0.1:17602"
            and .bind != "127.0.0.1:17603"
            and .bind != "127.0.0.1:17613"
          )]
       + [
           {"bind": "127.0.0.1:17602", "consumer": "weles", "service": "skarbiec-weles"},
           {"bind": "127.0.0.1:17603", "consumer": "weles", "service": "stado-object-api"}
         ])
' "$current" >"$updated"
"$stado" registry push "$updated"
