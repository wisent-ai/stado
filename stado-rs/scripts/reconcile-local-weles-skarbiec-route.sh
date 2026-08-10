#!/bin/sh
set -eu

umask 077
stado="$HOME/.stado/bin/stado"
if [ ! -x "$stado" ]; then
  printf '%s\n' 'Stado runtime is required' >&2
  exit 1
fi

stage_dir=$(/usr/bin/mktemp -d "$HOME/.stado/local-weles-skarbiec-route.XXXXXX")
trap '/bin/rm -rf "$stage_dir"' EXIT HUP INT TERM
current="$stage_dir/current.json"
updated="$stage_dir/updated.json"
"$stado" registry pull >"$current"

if /usr/bin/jq -e '
  .service_directory.services.skarbiec.consumers.weles.capabilities == ["secret-acquisition"]
  and any(
    .targets[]
    | select(.name == "lukasz-macbook")
    | .service_resolver.adapters[];
    . == {"bind": "127.0.0.1:17613", "consumer": "weles", "service": "skarbiec"}
  )
' "$current" >/dev/null
then
  printf '%s\n' 'Local Weles Skarbiec route already reconciled'
  exit 0
fi

/usr/bin/jq '
  .service_directory.generation += 1
  | .service_directory.services.skarbiec.consumers.weles = {
      "capabilities": ["secret-acquisition"]
    }
  | (.targets[] | select(.name == "lukasz-macbook") | .service_resolver.adapters) |=
      ([.[]
        | select(
            .bind != "127.0.0.1:17613"
            and (.consumer != "weles" or .service != "skarbiec")
          )]
       + [
           {"bind": "127.0.0.1:17613", "consumer": "weles", "service": "skarbiec"}
         ])
' "$current" >"$updated"
"$stado" registry push "$updated"
