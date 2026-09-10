#!/usr/bin/env bash
# Report whether the stable bind actually serves, and who holds it.
#
# The release agent could not bind `127.0.0.1:8080` because a Stado proxy from an
# earlier run holds it, and adoption was declined because that proxy did not answer
# the readiness path. Both facts together mean the port is occupied by something
# that serves nothing, which is worse than either alone: the fleet's gateway looks
# bound and answers no request.
#
# Read-only: the HTTP status of the readiness path, the port holder, and the proxy's
# recorded target. No secret and no mutation.
set -euo pipefail

bind="${STABLE_BIND:-127.0.0.1:8080}"
path="${READINESS_PATH:-/health}"
product="${RELEASE_PRODUCT:-brama}"
state_dir="${RELEASE_STATE_DIR:-$HOME/.stado/release-state}"

printf 'host %s\n' "$(hostname -s 2>/dev/null || hostname)"
printf 'bind %s%s\n' "$bind" "$path"
code=$(/usr/bin/curl -s -o /dev/null -w '%{http_code}' -m 5 "http://$bind$path" 2>/dev/null || echo 000)
printf 'http %s\n' "$code"

printf -- '--- holders ---\n'
/usr/sbin/lsof -nP -iTCP -sTCP:LISTEN 2>/dev/null \
  | /usr/bin/awk -v bind="$bind" '$9 == bind || $9 ~ /:(18080|18081)$/ {print "  " $1, "pid=" $2, $9}' \
  | /usr/bin/head -6

proxy_state="$state_dir/$product-proxy.json"
printf -- '--- proxy target ---\n'
if [ -f "$proxy_state" ]; then
  /usr/bin/env python3 - "$proxy_state" <<'PY'
import json, sys

with open(sys.argv[1], encoding="utf-8") as handle:
    state = json.load(handle)
for key in ("upstream", "generation", "product", "updated_at"):
    if key in state:
        print(f"  {key} {state[key]}")
if not state:
    print("  proxy state document is empty")
PY
else
  printf '  %s absent\n' "$proxy_state"
fi
