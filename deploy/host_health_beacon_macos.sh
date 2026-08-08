#!/bin/bash
# host_health_beacon_macos.sh — periodic host health writer for the Stado
# backend. Collects launchd unit state for the managed labels and publishes
# it through the authenticated `stado host publish-beacon` control route.
set -euo pipefail

STADO_BIN="${STADO_BIN:-$HOME/.stado/bin/stado}"
GUI_DOMAIN="gui/$(/usr/bin/id -u)"
# Which units to report. Asking launchd what it actually has loaded under the
# product prefix keeps this from being a second declaration of what the host runs:
# a service deployed through `stado service deploy` was reported `missing` forever,
# because it was live, managed and simply absent from a two-element list written
# here by hand. WC_HEALTH_UNITS still overrides, for a host that must report a
# narrower set.
loaded_product_units() {
    /bin/launchctl list | /usr/bin/awk '$NF ~ /^com\.wisent\./ { print $NF }'
}
LABELS="${WC_HEALTH_UNITS:-$(loaded_product_units)}"

reported_at=$(/bin/date -u +%Y-%m-%dT%H:%M:%SZ)
# `&> /dev/null` here sent stdout to the void and captured nothing, so every beacon
# ever published carried an empty disk line -- the reading this daemon exists to
# take. Nothing is discarded now; a df that complains says so in the unit log.
disk_line=$(/bin/df -h / | /usr/bin/awk '{line=$0} END {if (line != "") print line}' || true)

units_json=""
for lbl in $LABELS; do
    if /bin/launchctl print "${GUI_DOMAIN}/${lbl}" &> /dev/null; then
        # Same bug as the disk line above: with stdout discarded, `info` was always
        # empty, the exit-code check below could never fire, and a crash-looping
        # unit reported `active`. The state was decided by whether the unit existed,
        # never by how it was doing.
        info=$(/bin/launchctl print "${GUI_DOMAIN}/${lbl}" || true)
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

# Relay for hosts that cannot publish for themselves.
#
# A machine with no stado binary can still collect its own beacon -- that part
# is hostname, df and systemctl -- but it cannot hand it in, and one published
# by hand goes stale within the hour, which is worse than none because it
# still looks like reporting. This host has the binary and the grant, so it
# relays on every tick it already runs: collect over the approved channel,
# publish on that host's behalf.
#
# The list comes from the registry rather than from a name written here, and a
# target that publishes for itself simply has no collector helper installed,
# so its relay attempt fails, says so, and changes nothing. A failed relay
# never takes this host's own beacon down with it.
this_target=$("$STADO_BIN" registry self | { IFS="$(printf '\t')" read -r name _rest || true; printf '%s' "$name"; })
relay_targets=${WC_BEACON_RELAY_TARGETS:-$("$STADO_BIN" registry pull | /opt/homebrew/bin/jq -r '.targets[].name')}
for relay in $relay_targets; do
    [ "$relay" != "$this_target" ] || continue
    if collected=$("$STADO_BIN" host run-helper "$relay" collect-host-health-beacon); then
        printf '%s' "$collected" | /usr/bin/sed -n '/^{/,/^}/p' | "$STADO_BIN" host publish-beacon - >/dev/null \
            || printf '%s\n' "host_health_beacon: publishing on behalf of $relay failed" >/dev/stderr
    else
        printf '%s\n' "host_health_beacon: collecting from $relay failed" >/dev/stderr
    fi
done
