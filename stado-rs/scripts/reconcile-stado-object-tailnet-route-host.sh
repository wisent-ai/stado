#!/bin/sh
set -eu

for candidate in \
  /Applications/Tailscale.app/Contents/MacOS/Tailscale \
  /usr/local/bin/tailscale \
  /opt/homebrew/bin/tailscale \
  /usr/bin/tailscale
do
  if [ -x "$candidate" ]; then
    tailscale=$candidate
    break
  fi
done
[ -n "${tailscale:-}" ] || {
  printf '%s\n' 'tailscale executable not found' >&2
  exit 1
}

"$tailscale" serve --bg --https=8443 http://127.0.0.1:8765
"$tailscale" serve status --json \
  | /usr/bin/python3 -c '
import json
import sys
status = json.load(sys.stdin)
web = status.get("Web", {})
route = next((entry for name, entry in web.items() if name.endswith(":8443")), None)
proxy = ((route or {}).get("Handlers", {}).get("/") or {}).get("Proxy")
if proxy != "http://127.0.0.1:8765":
    raise SystemExit(f"dedicated Stado object route mismatch: {proxy!r}")
print("dedicated Stado object route: https://*:8443 -> http://127.0.0.1:8765")
'
