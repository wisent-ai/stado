#!/bin/sh
# Donate the Codex subscription on this host to the `weles` agent.
#
# Brama's browser work runs as agent `weles`, but the codex reauth runner donates
# with `WISENT_APP_AGENT_ID` from its config, so the refreshed credential is
# bounded to `wisent-app` and Weles gets "no active codex credential for agent".
# This repeats the runner's own donation contract with the weles identity:
# POST /v1/subscriptions/weles, HMAC-SHA256 over `agent:timestamp:sha256(body)`,
# Bearer from weles-model-router, body {provider, label, api_key: <auth.json>}.
#
# Nothing secret is printed; the report is the status code and the subscription
# label. The three identity files were installed with `stado host install-secret`
# and are read from disk, never from argv.
set -eu

AGENT_FILE="$HOME/.stado/weles-agent-id"
SECRET_FILE="$HOME/.stado/weles-agent-secret"
BEARER_FILE="$HOME/.stado/weles-router-token"
AUTH_JSON=${CODEX_AUTH_PATH:-$HOME/.codex/auth.json}
ROUTER=${BRAMA_URL:-http://127.0.0.1:8080}

for f in "$AGENT_FILE" "$SECRET_FILE" "$BEARER_FILE" "$AUTH_JSON"; do
    [ -s "$f" ] || { printf '%s\n' "missing $f" >&2; exit 1; }
done

AGENT_FILE=$AGENT_FILE SECRET_FILE=$SECRET_FILE BEARER_FILE=$BEARER_FILE \
AUTH_JSON=$AUTH_JSON ROUTER=$ROUTER /usr/bin/python3 - <<'PY'
import datetime
import hashlib
import hmac
import json
import os
import urllib.error
import urllib.request

agent = open(os.environ["AGENT_FILE"], encoding="utf-8").read().strip()
secret = open(os.environ["SECRET_FILE"], encoding="utf-8").read().strip()
bearer = open(os.environ["BEARER_FILE"], encoding="utf-8").read().strip()
auth_json = open(os.environ["AUTH_JSON"], encoding="utf-8").read().strip()
router = os.environ["ROUTER"].rstrip("/")

body = json.dumps({
    "provider": "codex",
    "label": f"codex-reauth weles {datetime.datetime.now(datetime.timezone.utc).isoformat()}",
    "api_key": auth_json,
})
ts = str(int(datetime.datetime.now(datetime.timezone.utc).timestamp()))
body_hash = hashlib.sha256(body.encode()).hexdigest()
signature = hmac.new(secret.encode(), f"{agent}:{ts}:{body_hash}".encode(), hashlib.sha256).hexdigest()

request = urllib.request.Request(
    f"{router}/v1/subscriptions/{agent}",
    data=body.encode(),
    headers={
        "x-agent-id": agent,
        "x-agent-timestamp": ts,
        "x-agent-signature": signature,
        "Authorization": f"Bearer {bearer}",
        "Content-Type": "application/json",
    },
    method="POST",
)
try:
    with urllib.request.urlopen(request, timeout=60) as response:
        result = json.loads(response.read())
        sub = result.get("subscription") or result
        print(json.dumps({
            "status": response.status,
            "subscription_id": sub.get("id"),
            "label": sub.get("label"),
            "provider": sub.get("provider"),
        }))
except urllib.error.HTTPError as error:
    print(json.dumps({"status": error.code, "error": error.read().decode(errors="replace")[:300]}))
    raise SystemExit(1)
PY
