#!/bin/sh
# Probe this host's loopback listeners for the fleet health API.
#
# Ports are read from `ss`, never written down here, so this reports what the
# host actually has rather than what someone assumed. A health API answers
# `/healthz`; the body says which service it is.
set -eu

PATH="/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"
export PATH

if ! command -v ss >/dev/null; then
  printf 'ss unavailable\n'
  exit
fi

ss -ltn | awk '{print $4}' | grep '^127\.' | sort -u | while IFS= read -r endpoint; do
  body=$(curl -s "http://$endpoint/healthz" || printf '')
  if [ -n "$body" ]; then
    printf '%s -> %s\n' "$endpoint" "$(printf '%s' "$body" | cut -c-'160')"
  else
    printf '%s -> (no /healthz answer)\n' "$endpoint"
  fi
done
