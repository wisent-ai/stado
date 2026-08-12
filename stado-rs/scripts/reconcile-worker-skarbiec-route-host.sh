#!/bin/sh
# Keep the remote workload-agent HTTPS route on the canonical Skarbiec vault.
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

[ -n "${tailscale:-}" ] || { printf '%s\n' 'tailscale executable not found' >&2; exit 1; }
/usr/sbin/lsof -nP -iTCP:8895 -sTCP:LISTEN -Fc 2>/dev/null \
  | /usr/bin/grep -qx cskarbiec
"$tailscale" serve --yes --bg --https=9443 http://127.0.0.1:8895
"$tailscale" serve status --json \
  | /usr/bin/jq -e '.Web["charless-mac-mini.tail6443b3.ts.net:9443"].Handlers["/"].Proxy == "http://127.0.0.1:8895"'
printf '%s\n' 'worker Skarbiec route: https://charless-mac-mini.tail6443b3.ts.net:9443 -> http://127.0.0.1:8895'
