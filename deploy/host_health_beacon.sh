#!/bin/bash
# Out-of-band host health beacon for the configured Stado backend.
#
# The local writer collects the same host/unit recovery evidence as before,
# then delegates publication to `stado host publish-beacon`. That command uses
# only the dedicated stado-host-health-beacon Skarbiec grant and authenticated
# Stado control route; it has no provider-SDK or direct-storage fallback.
#
# Run via systemd timer (Linux) or launchd LaunchAgent (macOS); the tick
# interval should be approximately one minute.

set -euo pipefail
UNITS_TO_WATCH="${WC_HEALTH_UNITS:-wisent-agent.service}"
HOST_SLUG=$(/bin/hostname -s 2>/dev/null | /usr/bin/tr '[:upper:]' '[:lower:]')

STADO_BIN="${STADO_BIN:-${HOME:-/home/ubuntu}/.stado/bin/stado}"
# Collecting and publishing are separable, and on one host they have to be.
# Only the disk-cleanup pass, the inference summary and the publish call need
# the Rust binary; the rest is hostname, df and systemctl. The fleet's single
# Linux machine has neither the binary nor a toolchain to build one, so it
# reported nothing at all -- while `stado host publish-beacon` exists precisely
# to hand in a beacon collected somewhere else. With WC_BEACON_COLLECT_ONLY
# set, this prints the beacon and publishes nothing.
collect_only="${WC_BEACON_COLLECT_ONLY:-}"
if [ ! -x "$STADO_BIN" ]; then
    # No binary means publishing is impossible, so collecting is the only
    # useful thing left to do -- and it is genuinely useful, because an
    # operator's stado can hand the result in on this host's behalf. Failing
    # here instead is why the fleet's one Linux machine reported nothing.
    collect_only=yes
    STADO_BIN=""
fi

# The same coordinates the macOS collector derives, for the same reason: the
# health API is the store this host already addresses, and Skarbiec's endpoint
# is in the service directory. A host that waits for a timer's environment
# publishes nothing when run any other way, and its silence reads as a dead
# machine.
PYTHON_BIN="${PYTHON_BIN:-$(command -v python3 || printf /usr/bin/python3)}"
READ_STORE_URL='import json,pathlib
p = pathlib.Path.home() / ".config" / "stado" / "config.json"
print(json.loads(p.read_text()).get("storage", {}).get("stado", {}).get("url", "") if p.is_file() else "")'
READ_SKARBIEC='import json,sys
host = sys.argv[1]
text = sys.stdin.read().strip()
doc = json.loads(text) if text else {}
service = doc.get("service_directory", {}).get("services", {}).get("skarbiec", {})
print(service.get("endpoints", {}).get(host, {}).get("url", ""))'
if [ -n "$STADO_BIN" ]; then
    export STADO_HOST_HEALTH_API_URL="${STADO_HOST_HEALTH_API_URL:-$("$PYTHON_BIN" -c "$READ_STORE_URL")}"
    declared_skarbiec=$("$STADO_BIN" registry pull 2>/dev/null \
        | "$PYTHON_BIN" -c "$READ_SKARBIEC" "$HOST_SLUG" || true)
    export STADO_HOST_HEALTH_SKARBIEC_URL="${declared_skarbiec:-${STADO_HOST_HEALTH_SKARBIEC_URL:-}}"
    export STADO_HOST_HEALTH_SKARBIEC_CONSUMER="${STADO_HOST_HEALTH_SKARBIEC_CONSUMER:-stado-host-health-beacon}"
    export STADO_HOST_HEALTH_SKARBIEC_TOKEN_FILE="${STADO_HOST_HEALTH_SKARBIEC_TOKEN_FILE:-$HOME/.stado/host-health-beacon-skarbiec-token}"
fi

# Use the existing health schedule for a bounded, registry-authorized pass.
WC_BIN="${WC_BIN:-$STADO_BIN}"
if [ -x "$WC_BIN" ]; then
    /usr/bin/timeout 40s "$WC_BIN" disk-cleanup --once >/dev/null 2>&1 || \
        echo "host_health_beacon: wc disk-cleanup did not complete; leaving disk state unchanged" >&2
else
    echo "host_health_beacon: wc disk-cleanup unavailable; leaving disk state unchanged" >&2
fi

reported_at=$(/bin/date -u +%Y-%m-%dT%H:%M:%SZ)

# Root fs usage
disk_line=$(/bin/df -k / 2>/dev/null | /usr/bin/awk 'NR==2 {print $3, $4, $5}')
read -r disk_used_kb disk_avail_kb disk_pct_str <<<"$disk_line"
disk_pct="${disk_pct_str%%%}"
# Avail in GB (rounded down).
disk_avail_gb=$(( ${disk_avail_kb:-0} / 1024 / 1024 ))


# systemctl unit states (one entry per UNITS_TO_WATCH item, comma-sep).
units_json=""
for unit in ${UNITS_TO_WATCH//,/ }; do
    case "$unit" in
        *weles*) echo "host_health_beacon: raw Weles unit lifecycle is forbidden"; false ;;
    esac
    if /usr/bin/systemctl is-active "$unit" >/dev/null 2>&1; then
        state="active"
    elif /usr/bin/systemctl is-failed "$unit" >/dev/null 2>&1; then
        state="failed"
    else
        state="inactive"
    fi
    # Restart counter: parse from `systemctl show -p NRestarts`.
    n_restarts=$(/usr/bin/systemctl show -p NRestarts --value "$unit" 2>/dev/null || echo "?")
    since=$(/usr/bin/systemctl show -p ActiveEnterTimestamp --value "$unit" 2>/dev/null || echo "?")
    if [ -n "$units_json" ]; then units_json="$units_json,"; fi
    units_json="$units_json\"$unit\":{\"state\":\"$state\",\"n_restarts\":\"$n_restarts\",\"active_since\":\"$since\"}"
done

if [ -n "$STADO_BIN" ] && inference_json=$("$STADO_BIN" inference beacon); then
    :
else
    inference_json='{}'
fi
case "$inference_json" in
    \{*\}) ;;
    *) inference_json='{}' ;;
esac


tmpfile=$(/usr/bin/mktemp)
trap 'rm -f "$tmpfile"' EXIT
cat > "$tmpfile" <<EOF
{
  "host": "${HOST_SLUG}",
  "reported_at": "${reported_at}",
  "disk_pct": ${disk_pct:-0},
  "disk_avail_gb": ${disk_avail_gb:-0},
  "units": {${units_json}},
  "inference": ${inference_json}
}
EOF

if [ -n "$collect_only" ]; then
    /bin/cat "$tmpfile"
else
    "$STADO_BIN" host publish-beacon "$tmpfile" >/dev/null
fi
