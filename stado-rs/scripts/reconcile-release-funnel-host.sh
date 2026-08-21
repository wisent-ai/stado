#!/bin/sh
set -eu

for candidate in \
  /Applications/Tailscale.app/Contents/MacOS/tailscale \
  /usr/bin/tailscale \
  /usr/local/bin/tailscale \
  /opt/homebrew/bin/tailscale; do
  if [ -x "$candidate" ]; then
    tailscale_bin=$candidate
    break
  fi
done
: "${tailscale_bin:?Tailscale CLI is not installed}"

"$tailscale_bin" funnel --bg --yes 8765 >/dev/null
"$tailscale_bin" funnel status --json
