#!/bin/sh
# Why does Brama answer 403 for the `weles` client?
#
# Brama runs, its identity list contains `weles` with the alias
# `weles/agent/primary`, and the browser agent still gets
# `model router 403 authorization_error`. Two candidates: the worker presents a
# token that no longer matches the one in Skarbiec, or the agent identity it
# sends does not match what Brama expects.
#
# This compares fingerprints, never values: the first eight hex characters of a
# SHA-256 are enough to tell "same secret" from "different secret" and cannot be
# replayed. It then makes one minimal routed request and prints only the status.
set -u

VAR=WELES_STADO_MODEL_ROUTER_TOKEN
BRAMA=${BRAMA_URL:-http://127.0.0.1:8080}

fingerprint() {
    printf '%s' "$1" | /usr/bin/shasum -a 256 | /usr/bin/cut -c1-8
}

# 1) What the worker process actually carries.
pid=$(/usr/bin/sudo -n /bin/launchctl print system/com.wisent.always-on.weles 2>/dev/null \
    | /usr/bin/awk '$1=="pid"{print $3; exit}')
worker_token=$(/usr/bin/sudo -n /bin/ps -Eww -o command= -p "${pid:-0}" 2>/dev/null \
    | /usr/bin/tr ' ' '\n' | /usr/bin/grep "^$VAR=" | /usr/bin/head -1 | /usr/bin/cut -d= -f2-)
printf 'worker_pid=%s worker_token_present=%s worker_fp=%s\n' \
    "${pid:-none}" "$([ -n "$worker_token" ] && echo yes || echo no)" \
    "$([ -n "$worker_token" ] && fingerprint "$worker_token" || echo n/a)"

# 2) What the env file on disk carries.
file_token=""
[ -f "$HOME/.weles/secrets.env" ] && file_token=$(/usr/bin/grep -m1 "^$VAR=" "$HOME/.weles/secrets.env" | /usr/bin/cut -d= -f2-)
printf 'env_file_token_present=%s env_fp=%s\n' \
    "$([ -n "$file_token" ] && echo yes || echo no)" \
    "$([ -n "$file_token" ] && fingerprint "$file_token" || echo n/a)"

# 3) What Skarbiec holds for the same item, through the broker.
vault_token=$(/usr/bin/python3 - <<'PY'
import json
import os
import urllib.error
import urllib.request

path = os.path.expanduser("~/.stado/wisent-backend-api-service-deployer-skarbiec-token")
try:
    grant = open(path, encoding="utf-8").read().strip()
except OSError:
    print("")
    raise SystemExit
request = urllib.request.Request(
    "http://127.0.0.1:8895/v1/items/read",
    data=json.dumps({"id": "weles-model-router", "field": "token"}).encode(),
    headers={
        "Authorization": "Bearer " + grant,
        "Content-Type": "application/json",
        "X-Consumer": "wisent-backend-api-service-deployer",
    },
    method="POST",
)
try:
    with urllib.request.urlopen(request, timeout=10) as response:
        print((json.load(response).get("value") or "").strip())
except urllib.error.HTTPError:
    print("")
PY
)
printf 'vault_token_present=%s vault_fp=%s\n' \
    "$([ -n "$vault_token" ] && echo yes || echo no)" \
    "$([ -n "$vault_token" ] && fingerprint "$vault_token" || echo n/a)"

# 4) One minimal routed request with whichever token we have, status only.
probe() {
    label=$1
    token=$2
    [ -n "$token" ] || { printf 'probe_%s=skipped\n' "$label"; return; }
    body='{"model":"weles/agent/primary","max_tokens":1,"messages":[{"role":"user","content":"ping"}]}'
    out=$(/usr/bin/curl -s -o /tmp/brama-probe.out -w '%{http_code}' --max-time 25 \
        -H "Authorization: Bearer $token" -H 'Content-Type: application/json' \
        -d "$body" "$BRAMA/v1/chat/completions" || true)
    printf 'probe_%s=%s detail=%s\n' "$label" "$out" \
        "$(/usr/bin/head -c 160 /tmp/brama-probe.out | /usr/bin/tr -d '\n')"
    /bin/rm -f /tmp/brama-probe.out
}
probe worker "$worker_token"
probe vault "$vault_token"
