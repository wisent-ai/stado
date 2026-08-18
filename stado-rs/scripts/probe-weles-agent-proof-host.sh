#!/bin/sh
# The same weles model call, once with a bare bearer and once with the agent
# HMAC proof, exactly as the codex reauth runner builds it.
#
# `wisent-backend` passes with a bearer alone; `weles` carries an agent_id in
# Brama's identity list, so the gateway may demand the signed trio before it
# even looks at the model. This is the only remaining explanation consistent
# with both probes.
set -eu
BRAMA=${BRAMA_URL:-http://127.0.0.1:8080}
AGENT_FILE="$HOME/.stado/weles-agent-id"
SECRET_FILE="$HOME/.stado/weles-agent-secret"
BEARER_FILE="$HOME/.stado/weles-router-token"

for f in "$AGENT_FILE" "$SECRET_FILE" "$BEARER_FILE"; do
    [ -s "$f" ] || { printf '%s\n' "missing $f" >&2; exit 1; }
done

AGENT_FILE=$AGENT_FILE SECRET_FILE=$SECRET_FILE BEARER_FILE=$BEARER_FILE BRAMA=$BRAMA \
/usr/bin/python3 - <<'PY'
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
url = os.environ["BRAMA"].rstrip("/") + "/v1/chat/completions"
body = json.dumps({
    "model": "weles/agent/primary",
    "max_tokens": 1,
    "messages": [{"role": "user", "content": "ping"}],
})

def call(signed):
    headers = {"Authorization": f"Bearer {bearer}", "Content-Type": "application/json"}
    if signed:
        ts = str(int(datetime.datetime.now(datetime.timezone.utc).timestamp()))
        digest = hashlib.sha256(body.encode()).hexdigest()
        headers.update({
            "x-agent-id": agent,
            "x-agent-timestamp": ts,
            "x-agent-signature": hmac.new(
                secret.encode(), f"{agent}:{ts}:{digest}".encode(), hashlib.sha256
            ).hexdigest(),
        })
    request = urllib.request.Request(url, data=body.encode(), headers=headers, method="POST")
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            return response.status, response.read()[:120].decode(errors="replace")
    except urllib.error.HTTPError as error:
        return error.code, error.read()[:160].decode(errors="replace")

status_bare, _ = call(False)
status_signed, detail = call(True)
print(json.dumps({
    "bearer_only": status_bare,
    "bearer_plus_agent_proof": status_signed,
    "detail": detail,
}))
PY
