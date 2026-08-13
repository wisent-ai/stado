#!/bin/sh
# Point this host's beacon at the endpoint that fronts the fleet's store.
#
# A beacon is only worth publishing where it will be read. This host's own
# control plane owns a different disk, so a beacon posted to it is stored,
# reported as success, and never seen -- which is exactly how a serving host
# read as "down" for hours. One endpoint for every beacon, and it is the
# supervised one that owns the shared store.
set -eu

endpoint=https://lukaszs-macbook-pro-4007-2.tail6443b3.ts.net
beacon=/Library/LaunchDaemons/com.wisent.host-health-beacon.plist
label=com.wisent.host-health-beacon

[ -f "$beacon" ] || { printf '%s\n' "no beacon unit at $beacon" >&2; exit 1; }

current=$(/usr/bin/sudo -n /usr/bin/plutil -extract EnvironmentVariables.STADO_HOST_HEALTH_API_URL raw -o - "$beacon" 2>/dev/null || echo "")
if [ "$current" = "$endpoint" ]; then
  printf '{"endpoint":"%s","state":"already-pointed"}\n' "$endpoint"
  exit 0
fi

/usr/bin/sudo -n /bin/cp "$beacon" "$beacon.pre-fleet-store"
/usr/bin/sudo -n /usr/bin/plutil -replace EnvironmentVariables.STADO_HOST_HEALTH_API_URL -string "$endpoint" "$beacon"
/usr/bin/sudo -n /bin/launchctl bootout "system/$label" >/dev/null 2>&1 || true
/usr/bin/sudo -n /bin/launchctl bootstrap system "$beacon"

printf '{"endpoint":"%s","previous":"%s","state":"pointed","backup":"%s"}\n' \
  "$endpoint" "${current:-unset}" "$beacon.pre-fleet-store"
