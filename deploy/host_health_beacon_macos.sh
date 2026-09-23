#!/bin/bash
# host_health_beacon_macos.sh — periodic host health writer for the Stado
# backend. Collects launchd unit state for the managed labels and publishes
# it through the authenticated `stado host publish-beacon` control route.
set -euo pipefail

STADO_BIN="${STADO_BIN:-$HOME/.stado/bin/stado}"
GUI_DOMAIN="gui/$(/usr/bin/id -u)"
LABELS="${WC_HEALTH_UNITS:-com.wisent.skarbiec com.wisent.host-health-beacon}"

reported_at=$(/bin/date -u +%Y-%m-%dT%H:%M:%SZ)
disk_line=$(/bin/df -h / &> /dev/null | /usr/bin/awk '{line=$0} END {if (line != "") print line}' || true)

units_json=""
for lbl in $LABELS; do
    if /bin/launchctl print "${GUI_DOMAIN}/${lbl}" &> /dev/null; then
        info=$(/bin/launchctl print "${GUI_DOMAIN}/${lbl}" &> /dev/null)
        state="active"
        # "last exit code" is the verdict only when it is a number and not
        # zero; "(never exited)" is healthy, not a failure.
        last_exit=$(echo "$info" | /usr/bin/awk -F'=' '/last exit code/ {gsub(/[ \t]/,""); print $2; exit}')
        if [ -n "$last_exit" ] && [ "$last_exit" != "0" ] && [ "$last_exit" != "(neverexited)" ]; then
            state="failed"
        fi
    else
        state="inactive"
    fi
    if [ -n "$units_json" ]; then units_json="$units_json,"; fi
    units_json="$units_json\"$lbl\":{\"state\":\"$state\"}"
done

HOST_SLUG=$(/bin/hostname -s | /usr/bin/tr '[:upper:]' '[:lower:]')
payload="{\"host\":\"$HOST_SLUG\",\"reported_at\":\"$reported_at\",\"disk\":\"$disk_line\",\"units\":{$units_json}}"

"$STADO_BIN" host publish-beacon <(printf '%s' "$payload")
