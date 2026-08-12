#!/bin/sh
set -eu

for candidate in \
  /Applications/Tailscale.app/Contents/MacOS/Tailscale \
  /usr/local/bin/tailscale \
  /opt/homebrew/bin/tailscale \
  /usr/bin/tailscale
do
  if [ -x "$candidate" ]; then
    printf 'binary=%s\n' "$candidate"
    "$candidate" version
    "$candidate" serve status --json
    "$candidate" serve --help
    exit 0
  fi
done

printf '%s\n' "tailscale executable not found" >&2
exit 1
