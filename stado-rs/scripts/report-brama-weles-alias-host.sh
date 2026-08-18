#!/bin/sh
# Does Brama actually serve the alias the Weles browser agent is required to use?
#
# Both env files and the API unit now carry WELES_AGENT_MODEL=weles/agent/primary,
# the worker's process environment shows it, and the browser task still refuses
# with the same sentence. Read literally, that sentence is about which alias Brama
# supports - so check the gateway rather than the caller.
#
# Read-only, and no bearer value is printed.
set -u

BRAMA=${BRAMA_URL:-http://127.0.0.1:8080}
printf '== brama health ==\n'
/usr/bin/curl -s -o /dev/null -w 'healthz=%{http_code}\n' --max-time 8 "$BRAMA/healthz"
/usr/bin/curl -s -o /dev/null -w 'models_unauth=%{http_code}\n' --max-time 8 "$BRAMA/v1/models"

printf '== configured aliases on disk ==\n'
for path in \
    "$HOME/.config/brama/service.env" \
    "$HOME/.config/brama/config.json" \
    "$HOME/.config/brama/models.json" \
    "$HOME/.stado/services/brama/config.json"
do
    [ -f "$path" ] || continue
    hits=$(/usr/bin/grep -o 'weles/agent/[a-z-]*' "$path" 2>/dev/null | /usr/bin/sort -u | /usr/bin/tr '\n' ' ')
    printf '%s -> %s\n' "$path" "${hits:-no weles/agent alias}"
done

printf '== brama config files present ==\n'
[ -d "$HOME/.config/brama" ] && /bin/ls -1 "$HOME/.config/brama" 2>/dev/null | /usr/bin/head -10

printf '== aliases advertised with weles router token ==\n'
token_file="$HOME/.stado/local-agent-skarbiec-token"
if [ -s "$token_file" ]; then
    consumer=stado-local-agent
    value=$(/usr/bin/python3 - "$token_file" "$consumer" <<'PY'
import json
import sys
import urllib.request

token = open(sys.argv[1], encoding="utf-8").read().strip()
request = urllib.request.Request(
    "http://127.0.0.1:8895/v1/items/read",
    data=json.dumps({"id": "weles-model-router", "field": "token"}).encode(),
    headers={
        "Authorization": "Bearer " + token,
        "Content-Type": "application/json",
        "X-Consumer": sys.argv[2],
    },
    method="POST",
)
try:
    with urllib.request.urlopen(request, timeout=10) as response:
        print((json.load(response).get("value") or "").strip())
except Exception:
    print("")
PY
)
    if [ -n "$value" ]; then
        /usr/bin/curl -s --max-time 10 -H "Authorization: Bearer $value" "$BRAMA/v1/models" \
            | /usr/bin/tr ',' '\n' | /usr/bin/grep -i 'weles\|agent' | /usr/bin/head -10
    else
        printf 'no weles-model-router token available to this grant\n'
    fi
fi
