#!/bin/sh
# Ask Weles to read how `bobloo.com` is routed at Cloudflare. Read-only.
#
# What is known from outside: the connector is attached to tunnel `bobloo-gcp`
# (17010c0f-a708-404c-b2f0-2c60eaf2f866), the tunnel's own config routes
# `bobloo.com` to `http://localhost:3000`, the origin answers 200 on both
# loopback families, and yet every cache-busted public request returns a bare
# Cloudflare `error code: 502` with no `via` header and nothing in the connector
# log. That combination means the edge is not sending the request to this tunnel.
#
# The missing fact lives only in the Cloudflare dashboard: what DNS records the
# apex actually has, and whether the tunnel public-hostname route still exists.
# Browsing runs on the dedicated Weles host, never on the operator's machine, and
# this job is explicitly read-only: it changes nothing and reveals no secret.
set -eu

WELES_ENV=${WELES_ENV_FILE:-$HOME/.weles/secrets.env}
WELES_URL=${WELES_URL:-http://127.0.0.1:8788/run}
[ -f "$WELES_ENV" ] || { printf '%s\n' "missing $WELES_ENV" >&2; exit 1; }

set -a
. "$WELES_ENV"
set +a
token=${WELES_API_TOKEN:-${WELES_CONSOLE_API_TOKEN:-}}
[ -n "$token" ] || { printf '%s\n' "no Weles API token in host runtime" >&2; exit 1; }

WELES_URL="$WELES_URL" WELES_TOKEN="$token" /usr/bin/python3 - <<'PY'
import json
import os
import urllib.error
import urllib.request

objective = (
    "Perform a strictly read-only inspection of how the apex hostname bobloo.com is routed in "
    "Cloudflare for the account that owns zone bobloo.com. Use the existing authenticated "
    "Cloudflare session for credential platform-admin-cloudflare. Change nothing: do not add, "
    "edit, delete, proxy, unproxy, or purge anything, and never reveal a secret or token value. "
    "Return structured JSON with these sections. 1) zone: exact zone name, plan, and status. "
    "2) dns_apex: every DNS record whose name is bobloo.com or @, with type, exact content, "
    "proxied true/false, and TTL - include CNAME targets verbatim, especially any ending in "
    "cfargotunnel.com. 3) dns_www: the same for the www hostname. 4) tunnels: every Cloudflare "
    "Tunnel in the account with its name, id, status, connector count, and each configured public "
    "hostname with its service; state explicitly whether a public hostname for bobloo.com exists "
    "and which tunnel id owns it. 5) tunnel_bobloo_gcp: for the tunnel named bobloo-gcp "
    "(id 17010c0f-a708-404c-b2f0-2c60eaf2f866): status, number of connected connectors, their "
    "colo locations, and the configured ingress rules verbatim. 6) rules: any Redirect, Origin, "
    "Transform, WAF custom or rate-limiting rule that matches bobloo.com or a path under it, with "
    "its action. 7) recent_errors: whatever the dashboard shows for 5xx or origin errors on this "
    "zone in the last 24 hours. For each section include observed_at, the console URL, and a "
    "status of observed, empty, blocked or unknown with the exact blocker text when not observed."
)
payload = {
    "action": "generic_browser_task",
    "params": {
        "url": "https://dash.cloudflare.com/",
        "objective": objective,
        "flow_name": "cloudflare-bobloo-routing-readonly",
        "session_label": "platform-admin-cloudflare",
        "admin_credential_id": "platform-admin-cloudflare",
        "platform_key": "cloudflare",
        "proxy": "none",
        "headless": False,
        "constraints": {
            "platform": "cloudflare",
            "zone": "bobloo.com",
            "read_only": True,
        },
    },
    "timeout_ms": 1800000,
    "creds": "redact",
}
request = urllib.request.Request(
    os.environ["WELES_URL"],
    data=json.dumps(payload).encode(),
    headers={
        "Authorization": "Bearer " + os.environ["WELES_TOKEN"],
        "Content-Type": "application/json",
        "Accept": "application/json",
    },
    method="POST",
)
try:
    with urllib.request.urlopen(request, timeout=1850) as response:
        body = response.read().decode(errors="replace")
        print(body[:6000])
except urllib.error.HTTPError as error:
    detail = error.read().decode(errors="replace")[:2000]
    raise SystemExit(f"Weles returned HTTP {error.code}: {detail}")
PY
