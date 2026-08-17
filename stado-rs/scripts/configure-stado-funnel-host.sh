#!/bin/sh
set -eu

binary=/Applications/Tailscale.app/Contents/MacOS/Tailscale
[ -x "$binary" ] || { printf '%s\n' "Tailscale CLI is not installed" >&2; exit 1; }
"$binary" funnel --bg 8765
"$binary" funnel status
