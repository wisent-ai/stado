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
# The units this host is asked about. `WC_HEALTH_UNITS` is the operator's own
# addition; the registry's declarations for this host are unioned onto it below,
# once the binary that can read them is resolved. A hand-typed list was the only
# source until 2026-09-03, and the registry had declared `stado-resolver` on
# ubuntu-server-rtx-pro-6000 while the beacon watched `wisent-agent.service`
# alone -- so `registry doctor` reported `missing-plist` for a unit that was
# active with a live pid. Two lists and nothing reconciling them.
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

# Union the registry's own declarations for this host onto the list above.
#
# Asked of the binary rather than derived here, for the same reason the
# inference summary is: the registry is the binary's to read, and a second
# reader in shell would be a second answer to "what does this host run".
# Failure is not fatal -- a host that cannot reach the registry still reports
# the units it was told about, which is strictly more than nothing.
if [ -n "$STADO_BIN" ] && declared_units=$("$STADO_BIN" host beacon-units 2>/dev/null); then
    for declared in $declared_units; do
        case ",${UNITS_TO_WATCH}," in
            *",${declared},"*) ;;
            *) UNITS_TO_WATCH="${UNITS_TO_WATCH},${declared}" ;;
        esac
    done
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
fi

if [ -n "$STADO_BIN" ] && [ -z "${STADO_HOST_HEALTH_API_TOKEN_FILE:-}" ]; then
    declared_skarbiec=$("$STADO_BIN" registry pull 2>/dev/null \
        | "$PYTHON_BIN" -c "$READ_SKARBIEC" "$HOST_SLUG" || true)
    export STADO_HOST_HEALTH_SKARBIEC_URL="${declared_skarbiec:-${STADO_HOST_HEALTH_SKARBIEC_URL:-}}"
    export STADO_HOST_HEALTH_SKARBIEC_CONSUMER="${STADO_HOST_HEALTH_SKARBIEC_CONSUMER:-stado-host-health-beacon}"
    export STADO_HOST_HEALTH_SKARBIEC_TOKEN_FILE="${STADO_HOST_HEALTH_SKARBIEC_TOKEN_FILE:-$HOME/.stado/host-health-beacon-skarbiec-token}"
fi

# Having the binary is not the same as being able to publish. This host reaches
# the fleet's registry but holds no Skarbiec grant of its own and has no local
# broker to mint one against, so the publish call fails on a missing token file
# and the beacon is lost -- while the relay on the always-on Mac is standing by
# to hand it in. Collect in that case rather than failing: an unpublished
# beacon printed on stdout is exactly what the relay consumes.
if [ -z "$collect_only" ] && [ -z "${STADO_HOST_HEALTH_API_TOKEN_FILE:-}" ]; then
    grant_file="${STADO_HOST_HEALTH_SKARBIEC_TOKEN_FILE:-}"
    if [ -z "${STADO_HOST_HEALTH_API_URL:-}" ] || [ -z "${STADO_HOST_HEALTH_SKARBIEC_URL:-}" ] \
        || [ ! -f "$grant_file" ]; then
        echo "host_health_beacon: no publishing coordinates here; collecting for a relay" >&2
        collect_only=yes
    fi
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


# `systemctl --user`, with the environment a login shell does not carry.
# Transcribed from the `systemctl_user()` helper the remote service scripts
# already use, because a state read that asks a different manager than the
# writer used is a state read about a different unit.
systemctl_user() {
    runtime="/run/user/$(/usr/bin/id -u)"
    /usr/bin/env \
        XDG_RUNTIME_DIR="$runtime" \
        DBUS_SESSION_BUS_ADDRESS="unix:path=$runtime/bus" \
        /usr/bin/systemctl --user "$@"
}

# Run one systemctl query against a named manager.
systemctl_in() {
    manager="$1"
    shift
    case "$manager" in
        system) /usr/bin/systemctl "$@" ;;
        user) systemctl_user "$@" ;;
        *) return 1 ;;
    esac
}

# Which manager has the unit loaded: `system`, `user`, or nothing.
#
# `LoadState` is the discriminator, because it separates the two facts this
# script used to merge: `not-found` means this manager has never heard of the
# unit, and `loaded` means it has and can be asked about its state. The fleet
# installs some units into `systemd --user` under `~/.config/systemd/user`, so
# asking the SYSTEM manager about one answered `not-found` and every such unit
# was recorded `inactive`.
unit_manager() {
    if [ "$(/usr/bin/systemctl show -p LoadState --value "$1" 2>/dev/null)" = loaded ]; then
        printf 'system\n'
    elif [ "$(systemctl_user show -p LoadState --value "$1" 2>/dev/null)" = loaded ]; then
        printf 'user\n'
    fi
}

# Which launchd domain holds the label: `system`, `gui/<uid>`, or nothing.
#
# `launchctl print` is read-only, needs no privilege on Darwin, and is the ONLY
# reader that can answer for the system domain -- `launchctl list` cannot print
# it at all. That gap is not theoretical: `com.wisent.always-on.brama` and
# `com.wisent.always-on.skarbiec` are declared as system LaunchDaemons on
# charless-mac-mini, and the beacon there reported both `inactive` while
# `brama serve` and `skarbiec serve` were listening. The system domain is asked
# first, in the order `service bootout` acts in.
launchd_domain() {
    for domain in "system" "gui/$(/usr/bin/id -u)"; do
        if /bin/launchctl print "$domain/$1" >/dev/null 2>&1; then
            printf '%s\n' "$domain"
            return 0
        fi
    done
    return 0
}

# One scalar out of `launchctl print`, or empty.
#
# Only `pid`, `state`, `last exit code` and `runs` are ever taken: the same
# fixed set `stado service label-print` restricts itself to, because
# `launchctl print` also dumps the job's whole environment and this fleet's
# units carry tokens there. A beacon that published those would put
# credentials into an object every host can read.
launchd_field() {
    printf '%s\n' "$2" | /usr/bin/awk -v key="$1" '
        $0 ~ "^[[:space:]]*" key " = " {
            sub("^[[:space:]]*" key " = ", "")
            gsub(/^"|"$/, "")
            print
            exit
        }' 2>/dev/null
}
# Per-unit state, from whichever manager on this host actually holds the unit.
#
# Branched on the OS because until 2026-09-03 it was not: the loop asked
# `/usr/bin/systemctl` on every host, so on the macOS boxes -- where every
# managed unit is a launchd job -- it asked a binary that does not exist and
# recorded `inactive` for all of them.
os=$(/usr/bin/uname -s 2>/dev/null || printf 'unknown')
units_json=""
for unit in ${UNITS_TO_WATCH//,/ }; do
    case "$unit" in
        *weles*) echo "host_health_beacon: raw Weles unit lifecycle is forbidden"; false ;;
    esac
    manager=""
    state="unknown"
    n_restarts="?"
    since="?"
    if [ "$os" = "Darwin" ]; then
        manager=$(launchd_domain "$unit")
        if [ -n "$manager" ]; then
            printed=$(/bin/launchctl print "$manager/$unit" 2>/dev/null || printf '')
            pid=$(launchd_field pid "$printed")
            last_exit=$(launchd_field 'last exit code' "$printed")
            runs=$(launchd_field runs "$printed")
            if [ -n "$pid" ]; then
                state="active"
            elif [ -n "$last_exit" ] && [ "$last_exit" != 0 ]; then
                state="failed"
            else
                state="inactive"
            fi
            n_restarts="${runs:-?}"
        fi
    else
        manager=$(unit_manager "$unit")
        if [ -n "$manager" ]; then
            if systemctl_in "$manager" is-active "$unit" >/dev/null 2>&1; then
                state="active"
            elif systemctl_in "$manager" is-failed "$unit" >/dev/null 2>&1; then
                state="failed"
            else
                state="inactive"
            fi
            n_restarts=$(systemctl_in "$manager" show -p NRestarts --value "$unit" 2>/dev/null || echo "?")
            since=$(systemctl_in "$manager" show -p ActiveEnterTimestamp --value "$unit" 2>/dev/null || echo "?")
        fi
    fi
    # An empty manager means no manager on this host has the unit loaded, so
    # nothing was observed. Reported as `unknown` and NOT as `inactive`: "I
    # could not see it" and "it is stopped" are different facts, and merging
    # them is what let running units be reported as ones that do not exist.
    if [ -n "$units_json" ]; then units_json="$units_json,"; fi
    units_json="$units_json\"$unit\":{\"state\":\"$state\",\"manager\":\"${manager:-none}\",\"n_restarts\":\"$n_restarts\",\"active_since\":\"$since\"}"
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


payload_dir="${HOME}/.stado/work/host-health-beacon"
/usr/bin/install -d -m 700 "$payload_dir"
tmpfile=$(/usr/bin/mktemp "$payload_dir/beacon.XXXXXXXXXX")
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
