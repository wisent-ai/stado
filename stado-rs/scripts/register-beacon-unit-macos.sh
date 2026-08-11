#!/bin/sh
# Add this host's control-plane daemon to the labels its beacon reports.
#
# `stado service status` reads the beacon, not the host: a unit absent from
# WC_HEALTH_UNITS reads as "missing" however healthy it is. A newly supervised
# unit that nothing reports is indistinguishable from one that is not running,
# which is the failure this whole exercise started from.
set -eu

label=com.wisent.always-on.stado-object-api
beacon=/Library/LaunchDaemons/com.wisent.host-health-beacon.plist
[ -f "$beacon" ] || { printf '%s\n' "no beacon unit at $beacon" >&2; exit 1; }

current=$(/usr/bin/sudo -n /usr/bin/plutil -extract EnvironmentVariables.WC_HEALTH_UNITS raw -o - "$beacon" 2>/dev/null || echo "")
case " $current " in
  *" $label "*)
    printf '{"label":"%s","state":"already-reported"}\n' "$label"
    exit 0
    ;;
esac

/usr/bin/sudo -n /bin/cp "$beacon" "$beacon.pre-object-api"
if [ -z "$current" ]; then
  next="$label"
else
  next="$current $label"
fi
/usr/bin/sudo -n /usr/bin/plutil -replace EnvironmentVariables.WC_HEALTH_UNITS -string "$next" "$beacon"
/usr/bin/sudo -n /bin/launchctl bootout "system/com.wisent.host-health-beacon" >/dev/null 2>&1 || true
/usr/bin/sudo -n /bin/launchctl bootstrap system "$beacon"

printf '{"label":"%s","state":"reported","backup":"%s"}\n' "$label" "$beacon.pre-object-api"
