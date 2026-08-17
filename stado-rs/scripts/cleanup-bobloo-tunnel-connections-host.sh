#!/bin/sh
# Clear stale connector registrations on the bobloo tunnel, then reattach.
#
# Symptom this exists for: the connector reports four ready connections and zero
# request errors, the origin answers 200 on both loopback families, and yet every
# cache-busted public request returns a bare Cloudflare `error code: 502` with no
# `via` header and no matching entry in the connector log. The edge is holding
# connections from earlier connector runs and routing to them instead of to the
# live process. Restarting the connector cannot fix that; only clearing the
# tunnel's connection list can.
#
# The connector token already encodes the account tag, tunnel id and tunnel
# secret, so `cloudflared tunnel cleanup` needs no API token: the credentials
# file is reconstructed from the token that is already on this host, written
# owner-only, and deleted again on the way out. No secret is printed and none is
# passed on a command line.
set -eu

TOKEN_FILE="$HOME/.stado/cloudflared-token"
BIN=""
for candidate in /opt/homebrew/bin/cloudflared /usr/local/bin/cloudflared; do
    [ -x "$candidate" ] && BIN="$candidate" && break
done
[ -n "$BIN" ] || { printf '%s\n' "cloudflared is not installed" >&2; exit 1; }
[ -s "$TOKEN_FILE" ] || { printf '%s\n' "missing connector token: $TOKEN_FILE" >&2; exit 1; }

work=$(/usr/bin/mktemp -d /tmp/tunnel-cleanup.XXXXXX)
/bin/chmod 700 "$work"
trap '/bin/rm -rf "$work"' EXIT HUP INT TERM

tunnel_id=$(TOKEN_FILE="$TOKEN_FILE" WORK="$work" /usr/bin/python3 - <<'PY'
import base64
import json
import os
import pathlib

raw = pathlib.Path(os.environ["TOKEN_FILE"]).read_text().strip()
padded = raw + "=" * (-len(raw) % 4)
claims = json.loads(base64.urlsafe_b64decode(padded))
tunnel_id = claims["t"]
credentials = {
    "AccountTag": claims["a"],
    "TunnelID": tunnel_id,
    "TunnelSecret": claims["s"],
}
path = pathlib.Path(os.environ["WORK"]) / f"{tunnel_id}.json"
path.write_text(json.dumps(credentials), encoding="utf-8")
path.chmod(0o600)
print(tunnel_id)
PY
)
printf 'tunnel_id=%s\n' "$tunnel_id"

printf '== connections before ==\n'
"$BIN" --origincert /dev/null tunnel --cred-file "$work/$tunnel_id.json" info "$tunnel_id" 2>&1 \
    | /usr/bin/head -12 || true

printf '== cleanup ==\n'
"$BIN" --origincert /dev/null tunnel --cred-file "$work/$tunnel_id.json" cleanup "$tunnel_id" 2>&1 \
    | /usr/bin/head -6 || true

printf '== restart connector in place ==\n'
/usr/bin/sudo -n /bin/launchctl kickstart -k system/com.wisent.cloudflared >/dev/null 2>&1 || true
/bin/sleep 20
printf 'ready=%s\n' "$(/usr/bin/curl -s --max-time 5 http://127.0.0.1:20241/ready)"
