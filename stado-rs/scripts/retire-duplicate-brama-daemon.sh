#!/bin/sh
# Retire the second Brama definition on this host.
#
# Two launchd units were serving the same process: the Stado-registry unit
# com.wisent.compute.service.com.wisent.always-on.brama (user agent, the one
# the canonical registry declares and Stado controls) and an unmanaged system
# LaunchDaemon com.wisent.always-on.brama. Both carry KeepAlive, so they race
# for 127.0.0.1:8080 and for the Skarbiec capability socket, and the loser
# restarts into the same collision. The registry unit is the one that stays.
set -eu

label=com.wisent.always-on.brama
plist=/Library/LaunchDaemons/$label.plist

/usr/bin/sudo -n /bin/launchctl bootout "system/$label" >/dev/null 2>&1 || true
/usr/bin/sudo -n /bin/launchctl disable "system/$label" >/dev/null 2>&1 || true

if [ -f "$plist" ]; then
    /usr/bin/sudo -n /bin/mv "$plist" "$plist.retired"
    printf '%s\n' "retired $plist -> $plist.retired"
else
    printf '%s\n' "no plist at $plist"
fi

if /usr/bin/sudo -n /bin/launchctl print "system/$label" >/dev/null 2>&1; then
    printf '%s\n' "still loaded: system/$label" >&2
    exit 1
fi

printf '%s\n' "system/$label is not loaded"
/bin/rm -f "$0"
