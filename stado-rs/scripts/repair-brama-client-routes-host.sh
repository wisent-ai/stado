#!/bin/sh
# Let Brama read the two client-router tokens its own startup check requires.
#
# After the alias key was repaired, Brama got as far as binding port 8080 and then
# exited with:
#   BRAMA_MODEL_ROUTER_CLIENT_IDENTITIES must give `weles` its exact required alias set
# The launcher builds that identity list by acquiring each client's router token
# through the Skarbiec capability broker, and `~/.stado/capability-routes.json`
# grants provider and Apple items but not a single `*-model-router`. So the list
# came out empty, and the server refuses to start without `weles` and
# `wisent-backend`.
#
# Only those two capabilities are added - the gateway resolves any other bearer
# against Skarbiec on demand, so a warm-start entry for every client would widen
# secret access for no gain.
set -eu

ROUTES="$HOME/.stado/capability-routes.json"
LABEL=com.wisent.always-on.brama
[ -f "$ROUTES" ] || { printf '%s\n' "missing $ROUTES" >&2; exit 1; }

stamp=$(/bin/date -u +%Y%m%dT%H%M%SZ)
/bin/cp -p "$ROUTES" "$ROUTES.bak-$stamp"

ROUTES="$ROUTES" /usr/bin/python3 - <<'PY'
import json
import os

path = os.environ["ROUTES"]
with open(path, encoding="utf-8") as source:
    routes = json.load(source)

required = {
    "weles-model-router": {"item": "weles-model-router", "field": "token"},
    "wisent-backend-model-router": {"item": "wisent-backend-model-router", "field": "token"},
}
added = []
for capability, route in required.items():
    if routes.get(capability) != route:
        routes[capability] = route
        added.append(capability)

with open(path, "w", encoding="utf-8") as target:
    json.dump(routes, target, indent=2, sort_keys=True)
    target.write("\n")
print(json.dumps({"added_or_fixed": added, "total_capabilities": len(routes)}, separators=(",", ":")))
PY

/usr/bin/sudo -n /bin/launchctl kickstart -k "system/$LABEL" >/dev/null 2>&1 || true
health=000
for _ in 1 2 3 4 5 6 7 8 9 10 11 12; do
    /bin/sleep 5
    health=$(/usr/bin/curl -s -o /dev/null -w '%{http_code}' --max-time 6 http://127.0.0.1:8080/healthz || true)
    case "$health" in 200|401|403) break ;; esac
done
printf 'backup=%s healthz=%s listeners=%s\n' "$ROUTES.bak-$stamp" "$health" \
    "$(/usr/sbin/lsof -nP -iTCP:8080 -sTCP:LISTEN 2>/dev/null | /usr/bin/grep -c LISTEN || true)"
/usr/bin/tail -3 "$HOME/.stado/logs/brama-always-on.err" 2>/dev/null | /usr/bin/cut -c1-170
case "$health" in 200|401|403) exit 0 ;; *) exit 1 ;; esac
