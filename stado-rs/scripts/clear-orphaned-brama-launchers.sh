#!/bin/sh
# Clear orphaned Brama launch wrappers on this host.
#
# Each start of the service runs through a `sudo -n -u charles -H env
# BRAMA_BIN=... start-with-skarbiec` wrapper. When a start races another
# definition of the same service, the wrapper survives its child and keeps the
# Skarbiec capability socket open, so the next start fails with "Skarbiec
# capability socket is owned by another process" and the port stays closed.
# This removes those wrappers and the entitlements-router children they hold,
# and leaves the capability database alone because that is state, not a lock.
set -eu

report() {
    printf '%s\n' "$1"
}

before=$(/bin/ps ax -o pid,command | /usr/bin/grep -c 'BRAMA_BIN=' || true)
report "wrappers before: $before"

for pattern in 'BRAMA_BIN=' 'start-with-skarbiec' 'skarbiec-entitlements-router' 'bin/brama serve'; do
    /usr/bin/sudo -n /usr/bin/pkill -TERM -f "$pattern" >/dev/null 2>&1 || true
done
/bin/sleep 2
for pattern in 'BRAMA_BIN=' 'start-with-skarbiec' 'skarbiec-entitlements-router' 'bin/brama serve'; do
    /usr/bin/sudo -n /usr/bin/pkill -KILL -f "$pattern" >/dev/null 2>&1 || true
done
/bin/sleep 1

after=$(/bin/ps ax -o pid,command | /usr/bin/grep -c 'BRAMA_BIN=' || true)
report "wrappers after: $after"

if /usr/sbin/lsof -nP -iTCP:8080 -sTCP:LISTEN >/dev/null 2>&1; then
    report "port 8080 still has a listener"
else
    report "port 8080 is free"
fi

/bin/rm -f "$0"
