#!/bin/sh
# One routed model call per client identity, status codes only.
#
# Tells apart "Brama authorizes nobody" from "only the weles client is refused",
# which decide where the remaining fault lives. No response body or token is
# printed.
set -eu
BRAMA=${BRAMA_URL:-http://127.0.0.1:8080}

probe() {
    label=$1
    token_file=$2
    model=$3
    [ -s "$token_file" ] || { printf '%s=missing-token-file\n' "$label"; return; }
    body="{\"model\":\"$model\",\"max_tokens\":1,\"messages\":[{\"role\":\"user\",\"content\":\"ping\"}]}"
    code=$(/usr/bin/curl -s -o /tmp/brama-$label.out -w '%{http_code}' --max-time 30 \
        -H "Authorization: Bearer $(/bin/cat "$token_file")" -H 'Content-Type: application/json' \
        -d "$body" "$BRAMA/v1/chat/completions" || true)
    printf '%s=%s %s\n' "$label" "$code" "$(/usr/bin/head -c 140 /tmp/brama-$label.out | /usr/bin/tr -d '\n')"
    /bin/rm -f "/tmp/brama-$label.out"
}

probe wisent-backend "$HOME/.stado/wisent-backend-router-token" wisent-backend/chat/primary
probe weles "$HOME/.stado/weles-router-token" weles/agent/primary
