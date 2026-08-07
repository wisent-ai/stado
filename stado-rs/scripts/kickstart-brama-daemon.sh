#!/bin/sh
# Restart the Brama LaunchDaemon and wait for it to answer.
#
# `stado service restart` cannot drive a unit in the `system` domain on this
# host: its remote body addresses launchctl without sudo, so the call returns an
# empty status. Until that is fixed in the CLI, this helper performs the same
# operation the daemon needs, then proves the port came back.
set -eu

label=com.wisent.always-on.brama
sudo="/usr/bin/sudo -n"

$sudo /bin/launchctl kickstart -k "system/$label"

n=0
while [ "$n" -lt 45 ]; do
    if /usr/bin/curl -s -o /dev/null --max-time 3 http://127.0.0.1:8080/v1/models; then
        break
    fi
    n=$((n + 1))
    /bin/sleep 1
done

printf 'listening: %s\n' "$(/usr/bin/curl -s -o /dev/null -w '%{http_code}' --max-time 5 http://127.0.0.1:8080/v1/models || echo none)"
/bin/ps ax -o pid,command \
    | /usr/bin/grep -E 'bin/brama serve|entitlements-router cap' \
    | /usr/bin/grep -v grep \
    | /usr/bin/sed 's|/Users/charles/.stado/services/brama/||'
/bin/rm -f "$0"
