
# `/healthz` is a startup snapshot. The object route revalidates its Skarbiec
# grant per request, so that snapshot can remain true while every protected
# read returns 503. Prove the boundary with the host's existing owner-only
# queue client bearer, passed to curl on stdin so it never appears in argv,
# then require the operator state to carry no object-boundary error.
authenticated_object_ready() {
  [ -r "$object_token_file" ] || return 1
  token=$(/bin/cat "$object_token_file")
  [ -n "$token" ] || return 1
  response="$work/protected-object.json"
  code=$(
    printf 'header = "Authorization: Bearer %s"\n' "$token" |
      /usr/bin/curl --config - --silent --show-error --max-time 5 \
        --output "$response" --write-out '%{http_code}' \
        "${object_url%/}/api/object?uri=stado%3A%2F%2F${object_namespace}%2Fregistry.json"
  ) || return 1
  [ "$code" = 200 ] || return 1
  state="$work/object-state.json"
  /usr/bin/curl --silent --show-error --fail --max-time 5 \
    "${object_url%/}/api/state.json" > "$state" || return 1
  "$program" host object-api-local boundary-ready "$state"
}

# Resolve the route a loaded job was given, rather than the route in the file
# launchd may read next time. The legacy server selected the configured backup
# whenever its client profile selected `stado`; the host's Stado makes that
# promotion explicit so a healthy read from local-backup cannot certify
# local-storage.
inspect_route() {
  "$program" host object-api-local route --mode "$1" --source "$2" --config "$config" \
    --expected "$staged" --runtime "$work/$label.runtime-state.json"
}
