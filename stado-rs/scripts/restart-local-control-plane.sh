#!/bin/sh
set -eu

label=com.wisent.compute.service.com.wisent.stado-host-health-api
domain="gui/$(/usr/bin/id -u)"

if ! /bin/launchctl print "$domain/$label" >/dev/null 2>&1; then
  printf '%s\n' "managed local control plane is not loaded: $domain/$label" >&2
  exit 1
fi

/bin/launchctl kickstart -k "$domain/$label"
/bin/launchctl print "$domain/$label" | /usr/bin/sed -n '1,80p'
