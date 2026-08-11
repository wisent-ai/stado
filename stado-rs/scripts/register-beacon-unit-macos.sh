#!/bin/sh
# Report this host's control-plane daemon in its beacon.
#
# `stado service status` reads the beacon, not the host: a unit absent from
# WC_HEALTH_UNITS reads as "missing" however healthy it is.
#
# The list lives in the beacon's launcher, which exports it and therefore
# overrides anything set in the launchd unit. An earlier version of this script
# wrote the key into the plist, where it was inert and became a second source
# of truth for the same list; that key is removed here, and the launcher --
# the one place the value is read from -- is what gets edited.
set -eu

label=com.wisent.always-on.stado-object-api
launcher="$HOME/.stado/bin/host-health-beacon-launcher"
beacon=/Library/LaunchDaemons/com.wisent.host-health-beacon.plist
unit=com.wisent.host-health-beacon

[ -f "$launcher" ] || { printf '%s\n' "no beacon launcher at $launcher" >&2; exit 1; }
[ -f "$beacon" ] || { printf '%s\n' "no beacon unit at $beacon" >&2; exit 1; }

# Retire the inert duplicates a previous attempt injected into the unit.
for key in WC_HEALTH_UNITS STADO_HOST_HEALTH_API_URL; do
  if /usr/bin/sudo -n /usr/bin/plutil -extract "EnvironmentVariables.$key" raw -o - "$beacon" >/dev/null 2>&1; then
    /usr/bin/sudo -n /usr/bin/plutil -remove "EnvironmentVariables.$key" "$beacon"
  fi
done

if /usr/bin/grep -q "$label" "$launcher"; then
  printf '{"label":"%s","state":"already-reported"}\n' "$label"
else
  /bin/cp "$launcher" "$launcher.pre-object-api"
  /usr/bin/sed -i '' "s|^export WC_HEALTH_UNITS=\"\(.*\)\"$|export WC_HEALTH_UNITS=\"\1 $label\"|" "$launcher"
  /usr/bin/grep -q "$label" "$launcher" || {
    /bin/cp "$launcher.pre-object-api" "$launcher"
    printf '%s\n' "could not append $label to WC_HEALTH_UNITS; launcher restored" >&2
    exit 1
  }
fi

/usr/bin/sudo -n /bin/launchctl bootout "system/$unit" >/dev/null 2>&1 || true
/usr/bin/sudo -n /bin/launchctl bootstrap system "$beacon"
printf '{"label":"%s","state":"reported","launcher":"%s"}\n' "$label" "$launcher"
