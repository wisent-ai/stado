#!/bin/sh
set -eu

if [ "$(uname -s)" != Darwin ]; then
  printf '%s\n' "local control-plane restart requires Darwin" >&2
  exit 1
fi

uid=$(/usr/bin/id -u)
label=com.wisent.compute.coordinator.local-control-plane
domain="gui/$uid"

if ! /bin/launchctl print "$domain/$label" >/dev/null 2>&1; then
  printf '%s\n' "managed local control plane is not loaded: $label" >&2
  exit 1
fi

/bin/launchctl kickstart -k "$domain/$label"
/bin/launchctl print "$domain/$label" | /usr/bin/sed -n '1,40p'
