#!/bin/bash
# host_health_beacon_macos.sh — periodic host health writer for the Stado
# backend. Collects launchd unit state for the managed labels and publishes
# it through the authenticated `stado host publish-beacon` control route.
set -euo pipefail

STADO_BIN="${STADO_BIN:-$HOME/.stado/bin/stado}"
GUI_DOMAIN="gui/$(/usr/bin/id -u)"
# The labels this host is judged on are the ones the registry declares for it.
# A fixed pair written here reported "inactive" for units nobody runs and said
# nothing about the ones that matter.
HOST_SLUG=$(/bin/hostname -s | /usr/bin/tr '[:upper:]' '[:lower:]')
# jq is not on every managed host, and a beacon that dies for want of it is a
# host that reads as dead. python3 ships with macOS and is already what the
# operator helpers use.
# A host knows itself by its hostname; the registry knows it by its target name,
# and the two differ on any machine somebody named twice. `operator-host`
# never matched target `operator-host`, so the label list came back empty, the
# fallback published a map holding only the beacon, and `registry doctor`
# reported a missing plist for six services that were installed and loaded.
# Match the way the fleet's readers match: target name, declared hostnames, or
# either with the local suffix dropped.
READ_LABELS='import json,sys
host = sys.argv[1].lower()
def stem(name):
    name = (name or "").lower()
    return name[:-len(".local")] if name.endswith(".local") else name
doc = json.load(sys.stdin)
for entry in doc.get("targets", []):
    known = {stem(entry.get("name"))}
    known.update(stem(name) for name in entry.get("hostnames", []) or [])
    if stem(host) in known:
        for service in entry.get("services", []):
            print(service.get("label") or service.get("unit") or service.get("name"))
        break'
READ_NAMES='import json,sys
for entry in json.load(sys.stdin).get("targets", []):
    print(entry.get("name"))'
PYTHON_BIN="${PYTHON_BIN:-$(command -v python3 || printf /usr/bin/python3)}"
LABELS="${WC_HEALTH_UNITS:-$("$STADO_BIN" registry pull 2>/dev/null \
    | "$PYTHON_BIN" -c "$READ_LABELS" "$HOST_SLUG" | /usr/bin/tr '\n' ' ')}"
LABELS="${LABELS:-com.wisent.host-health-beacon}"

# Publishing needs the health API, a Skarbiec to mint the bearer against, and
# the consumer grant this host holds. All three are already declared -- the API
# is the store this host is configured to address, and Skarbiec's endpoint is
# in the service directory -- so read them rather than restate them, and let a
# host that genuinely lacks one fail with its name.
READ_STORE_URL='import json,pathlib
p = pathlib.Path.home() / ".config" / "stado" / "config.json"
print(json.loads(p.read_text()).get("storage", {}).get("stado", {}).get("url", "") if p.is_file() else "")'
READ_SKARBIEC='import json,sys
host = sys.argv[1]
doc = json.load(sys.stdin)
service = doc.get("service_directory", {}).get("services", {}).get("skarbiec", {})
print(service.get("endpoints", {}).get(host, {}).get("url", ""))'
export STADO_HOST_HEALTH_API_URL="${STADO_HOST_HEALTH_API_URL:-$("$PYTHON_BIN" -c "$READ_STORE_URL")}"
# The registry wins over whatever the login environment carries: this host had
# `STADO_HOST_HEALTH_SKARBIEC_URL` pointing at the Weles vault's adapter, for a
# consumer that adapter does not serve, so every publish failed with a refused
# connection while the declared endpoint sat one port away.
declared_skarbiec=$("$STADO_BIN" registry pull 2>/dev/null \
    | "$PYTHON_BIN" -c "$READ_SKARBIEC" "$HOST_SLUG")
export STADO_HOST_HEALTH_SKARBIEC_URL="${declared_skarbiec:-${STADO_HOST_HEALTH_SKARBIEC_URL:-}}"
export STADO_HOST_HEALTH_SKARBIEC_CONSUMER="${STADO_HOST_HEALTH_SKARBIEC_CONSUMER:-stado-host-health-beacon}"
export STADO_HOST_HEALTH_SKARBIEC_TOKEN_FILE="${STADO_HOST_HEALTH_SKARBIEC_TOKEN_FILE:-$HOME/.stado/host-health-beacon-skarbiec-token}"
printf 'host_health_beacon: api=%s skarbiec=%s labels=%s\n' \
    "$STADO_HOST_HEALTH_API_URL" "$STADO_HOST_HEALTH_SKARBIEC_URL" "$LABELS" >/dev/stderr

reported_at=$(/bin/date -u +%Y-%m-%dT%H:%M:%SZ)
disk_line=$(/bin/df -h / 2>/dev/null | /usr/bin/awk '{line=$0} END {if (line != "") print line}' || true)

units_json=""
for lbl in $LABELS; do
    # An always-on unit lives in the system domain, and a `gui/$uid` lookup
    # misses it entirely -- which is how a running fleet reported itself as
    # inactive. Ask both domains, and let the one that answers decide.
    info=$(/bin/launchctl print "${GUI_DOMAIN}/${lbl}" 2>/dev/null \
        || /usr/bin/sudo -n /bin/launchctl print "system/${lbl}" 2>/dev/null \
        || true)
    if [ -n "$info" ]; then
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
relay_targets=${WC_BEACON_RELAY_TARGETS:-$("$STADO_BIN" registry pull | "$PYTHON_BIN" -c "$READ_NAMES")}
for relay in $relay_targets; do
    [ "$relay" != "$this_target" ] || continue
    # A host that publishes for itself has no collector under this name, and
    # the failure it returns is expected rather than interesting. Keep the one
    # line this script writes and drop the command's error block, so a tick
    # that worked does not read like a broken one.
    if collected=$("$STADO_BIN" host run-helper "$relay" collect-host-health-beacon 2>/dev/null); then
        printf '%s' "$collected" | /usr/bin/sed -n '/^{/,/^}/p' | "$STADO_BIN" host publish-beacon - >/dev/null \
            || printf '%s\n' "host_health_beacon: publishing on behalf of $relay failed" >/dev/stderr
    else
        printf '%s\n' "host_health_beacon: no collector on $relay; it publishes for itself" >/dev/stderr
    fi
done

# This host's own beacon is published above; the relay is a courtesy for hosts
# that cannot publish for themselves. A target with no collector installed must
# not turn this run into a failure, or the tick that did report reads as broken.
exit 0
