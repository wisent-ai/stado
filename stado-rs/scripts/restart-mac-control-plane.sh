#!/bin/sh
set -eu

label=com.wisent.compute.coordinator.charless-control-plane

if ! /bin/launchctl print "system/$label" >/dev/null 2>&1; then
  printf '%s\n' "managed control plane is not loaded: $label" >&2
  exit 1
fi

/usr/bin/sudo -n /bin/launchctl kickstart -k "system/$label"
/bin/launchctl print "system/$label" | /usr/bin/sed -n '1,80p'
