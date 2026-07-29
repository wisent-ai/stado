#!/bin/bash
# macOS host health writer for the configured Stado backend.
#
# Unit state is collected through launchctl and publication is delegated to
# the authenticated `stado host publish-beacon` control route. No provider
# SDK, cloud CLI, application-default credential, or direct-storage path is
# available to this LaunchAgent.

set -euo pipefail

STADO_BIN="${STADO_BIN:-$HOME/.stado/bin/stado}"
HOST_SLUG=$(/bin/hostname -s 2>/dev/null | /usr/bin/tr '[:upper:]' '[:lower:]')
if [ ! -x "$STADO_BIN" ]; then
    echo "host_health_beacon: Rust stado binary unavailable at $STADO_BIN" > /dev/stderr
    false
fi
PYTHON_BIN="${PYTHON_BIN:-/usr/bin/python3}"

# Preserve the bounded, best-effort disk recovery pass.
WC_BIN="${WC_BIN:-$STADO_BIN}"
if [ -x "$WC_BIN" ]; then
    if ! "$PYTHON_BIN" - "$WC_BIN" <<'CLEANUPPY' &>/dev/null
import subprocess
import sys

try:
    subprocess.run(
        [sys.argv[1], "disk-cleanup", "--once"],
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        timeout=40,
        check=True,
    )
except (OSError, subprocess.SubprocessError):
    raise SystemExit(1)
CLEANUPPY
    then
        echo "host_health_beacon: wc disk-cleanup did not complete; leaving disk state unchanged" >&2
    fi
else
    echo "host_health_beacon: wc disk-cleanup unavailable; leaving disk state unchanged" >&2
fi

reported_at=$(/bin/date -u +%Y-%m-%dT%H:%M:%SZ)

# Root fs: BSD df. KB blocks via `df -k`.
disk_line=$(/bin/df -k / 2>/dev/null | /usr/bin/awk 'NR==2 {print $3, $4, $5}')
read -r disk_used_kb disk_avail_kb disk_pct_str <<<"$disk_line"
disk_pct="${disk_pct_str%%%}"
disk_avail_gb=$(( ${disk_avail_kb:-0} / 1024 / 1024 ))

# Weles lifecycle is observable only through the authenticated Stado service
# API; raw launchd labels are forbidden here.
LABELS="${WC_HEALTH_UNITS:-com.wisent.compute.dashboard com.wisent.hf-refresh}"
GUI_DOMAIN="gui/$(/usr/bin/id -u)"

units_json=""
for lbl in $LABELS; do
    case "$lbl" in
        *weles*) echo "host_health_beacon: raw Weles launchd lifecycle is forbidden"; false ;;
    esac
    if /bin/launchctl print "${GUI_DOMAIN}/${lbl}" >/dev/null 2>&1; then
        # Pull "last exit code" + "state" + "pid" from the print
        # output. State "running" with last_exit=0 = active.
        info=$(/bin/launchctl print "${GUI_DOMAIN}/${lbl}" 2>/dev/null)
        state="active"
        last_exit=$(echo "$info" | /usr/bin/awk -F'=' '/last exit code/ {gsub(/[ \t]/,""); print $2; exit}')
        n_restarts="?"
        active_since=$(echo "$info" | /usr/bin/awk -F'=' '/spawn type/ {print "?"; exit}')
        if [ -n "$last_exit" ] && [ "$last_exit" != "0" ]; then
            state="failed"
        fi
    else
        state="inactive"
        last_exit="?"
        n_restarts="?"
        active_since="?"
    fi
    if [ -n "$units_json" ]; then units_json="$units_json,"; fi
    units_json="$units_json\"$lbl\":{\"state\":\"$state\",\"n_restarts\":\"$n_restarts\",\"active_since\":\"$active_since\"}"
done


tmpfile=$(/usr/bin/mktemp)
trap '/bin/rm -f "$tmpfile"' EXIT
cat > "$tmpfile" <<EOF
{
  "host": "${HOST_SLUG}",
  "reported_at": "${reported_at}",
  "disk_pct": ${disk_pct:-0},
  "disk_avail_gb": ${disk_avail_gb:-0},
  "units": {${units_json}}
}
EOF

"$STADO_BIN" host publish-beacon "$tmpfile" >/dev/null
