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
pull="$stage_dir/pull.json"
current="$stage_dir/current.json"
updated="$stage_dir/updated.json"

# `stado registry push --if-generation` exits this when somebody wrote first.
# See docs/examples/fleet/add-remove-host.sh for the canonical loop.
registry_conflict_exit=75
# Bounded re-read/re-apply rounds. An unconditional push erases whatever was
# published between this script's read and its write — the overwrite that
# destroyed a concurrent edit on 2026-09-01.
push_attempts=5

# One read hands back the document AND the generation it is at. Two pulls
# would pair a token with a document it does not describe.
read_registry() {
  "$stado" registry pull --with-generation >"$pull"
  generation=$(/usr/bin/jq -r '.generation' "$pull")
  /usr/bin/jq '.document' "$pull" >"$current"
}

# Both of these are pure in the document just read, which is what makes a
# conflict retryable: re-read, re-check, re-apply, push again.
already_reconciled() {
  /usr/bin/jq -e '
    .service_directory.services["stado-object-api"] == {
      "managed_service": "com.wisent.always-on.stado-object-api",
      "active_host": "charless-mac-mini",
      "endpoints": {"charless-mac-mini": {"url": "http://127.0.0.1:8765"}},
      "consumers": {
        "stado-local-agent": {"capabilities": ["object-storage"]},
        "weles": {"capabilities": ["object-storage"]}
      }
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
}

apply_route() {
  /usr/bin/jq '
    .service_directory.generation += 1
    | .service_directory.services["stado-object-api"] = {
        "managed_service": "com.wisent.always-on.stado-object-api",
        "active_host": "charless-mac-mini",
        "endpoints": {"charless-mac-mini": {"url": "http://127.0.0.1:8765"}},
        "consumers": {
          "stado-local-agent": {"capabilities": ["object-storage"]},
          "weles": {"capabilities": ["object-storage"]}
        }
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
}

attempt=1
while :; do
  read_registry
  if already_reconciled; then
    printf '%s\n' 'Weles object route already reconciled'
    exit 0
  fi
  apply_route

  status=0
  "$stado" registry push "$updated" --if-generation "$generation" || status=$?
  if [ "$status" -eq 0 ]; then
    printf '%s\n' 'Reconciled Weles object route'
    exit 0
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
