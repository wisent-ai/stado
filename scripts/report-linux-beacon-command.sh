#!/bin/sh
# Show what this host's beacon unit runs and the publisher journal that records
# why a scheduled invocation failed.
#
# Read-only: this diagnostic never executes the publisher itself. Running it in
# the foreground used to turn a log reader into an unscheduled publication and
# still inspected the obsolete `wisent-host-health.service` identity instead of
# the unit the timer actually triggers.
set -eu

PATH="$HOME/.cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"
export PATH

UNIT=stado-host-beacon.service

printf '== unit definition ==\n'
systemctl cat "$UNIT" --no-pager 2>/dev/null | sed 's/^/  /'

systemctl show "$UNIT" --property=ExecStart --no-pager 2>/dev/null \
  | sed 's/^/  /'

printf '\n== current state ==\n'
systemctl status "$UNIT" --no-pager -n 0 2>&1 | sed -n '1,8p' | sed 's/^/  /' || true

printf '\n== publisher journal ==\n'
journalctl -u "$UNIT" -n 40 --no-pager 2>&1 | sed 's/^/  /'
