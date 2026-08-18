#!/bin/sh
# Give the Weles API unit the Brama alias its browser agent requires.
#
# The worker unit inherited the corrected value, but the API unit - the process
# that serves /run and launches the browser agent - carried no
# WELES_AGENT_MODEL at all, and an earlier repair only replaced keys that were
# already present. So every browser task kept failing with
# "must be the exact supported Brama alias weles/agent/primary" while both env
# files on disk were already correct.
#
# Adds the key to the unit file, keeps a timestamped backup, restarts in place.
set -eu

REQUIRED=weles/agent/primary
VAR=WELES_AGENT_MODEL
LABEL=com.wisent.always-on.weles-api
PLIST="/Library/LaunchDaemons/$LABEL.plist"
PB=/usr/libexec/PlistBuddy

[ -f "$PLIST" ] || { printf '%s\n' "missing $PLIST" >&2; exit 1; }

stamp=$(/bin/date -u +%Y%m%dT%H%M%SZ)
/usr/bin/sudo -n /bin/cp -p "$PLIST" "$PLIST.bak-$stamp"

/usr/bin/sudo -n "$PB" -c "Add :EnvironmentVariables dict" "$PLIST" >/dev/null 2>&1 || true
if /usr/bin/sudo -n "$PB" -c "Print :EnvironmentVariables:$VAR" "$PLIST" >/dev/null 2>&1; then
    /usr/bin/sudo -n "$PB" -c "Set :EnvironmentVariables:$VAR $REQUIRED" "$PLIST"
else
    /usr/bin/sudo -n "$PB" -c "Add :EnvironmentVariables:$VAR string $REQUIRED" "$PLIST"
fi
/usr/bin/sudo -n /usr/bin/plutil -lint "$PLIST" >/dev/null
printf 'plist=%s %s=%s backup=%s\n' "$PLIST" "$VAR" \
    "$(/usr/bin/sudo -n "$PB" -c "Print :EnvironmentVariables:$VAR" "$PLIST")" "$PLIST.bak-$stamp"

/usr/bin/sudo -n /bin/launchctl kickstart -k "system/$LABEL" >/dev/null 2>&1 || true
/bin/sleep 15
pid=$(/usr/bin/sudo -n /bin/launchctl print "system/$LABEL" 2>/dev/null \
    | /usr/bin/awk '$1=="pid"{print $3; exit}')
printf 'pid=%s in_process=%s api=%s\n' "${pid:-none}" \
    "$(/usr/bin/sudo -n /bin/ps -Eww -o command -p "${pid:-0}" 2>/dev/null | /usr/bin/tr ' ' '\n' | /usr/bin/grep -c "^$VAR=$REQUIRED")" \
    "$(/usr/bin/curl -s -o /dev/null -w '%{http_code}' --max-time 8 http://127.0.0.1:8788/healthz || true)"
