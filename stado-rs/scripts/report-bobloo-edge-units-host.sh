#!/bin/sh
# Retire the abandoned LaunchAgent plists and report what actually runs.
#
# The first attempt at the tunnel connector wrote a LaunchAgent, which the
# per-user domain refused with `5: Input/output error`; the working unit is a
# LaunchDaemon. Leaving the agent file behind gives the registry a second,
# false answer to "where does this service live", so it goes.
set -u

for label in com.wisent.cloudflared com.wisent.bobloo-gateway; do
    agent="$HOME/Library/LaunchAgents/$label.plist"
    if [ -f "$agent" ]; then
        /bin/rm -f "$agent" && printf 'removed_stale_agent=%s\n' "$agent"
    fi
    daemon="/Library/LaunchDaemons/$label.plist"
    if [ -f "$daemon" ]; then
        pid=$(/usr/bin/sudo -n /bin/launchctl print "system/$label" 2>/dev/null \
            | /usr/bin/awk '$1=="pid"{print $3; exit}')
        state=$(/usr/bin/sudo -n /bin/launchctl print "system/$label" 2>/dev/null \
            | /usr/bin/awk '$1=="state"{print $3; exit}')
        printf 'unit=%s domain=system pid=%s state=%s\n' "$daemon" "${pid:-none}" "${state:-unknown}"
    else
        printf 'unit=%s missing\n' "$daemon"
    fi
done

printf 'listening:\n'
/usr/sbin/lsof -nP -iTCP -sTCP:LISTEN 2>/dev/null \
    | /usr/bin/awk '{print $1, $9}' \
    | /usr/bin/grep -E ':(3000|20241)$' | /usr/bin/sort -u
printf 'processes:\n'
/bin/ps ax -o pid -o etime -o comm | /usr/bin/grep -E 'cloudflared|caddy' | /usr/bin/grep -v grep
