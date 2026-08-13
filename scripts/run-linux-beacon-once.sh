#!/bin/sh
# Run this host's health beacon once and report what it cost.
#
# One bounded invocation of the host's own one-shot unit: it publishes a beacon
# if publishing works at all, and the journal then states the CPU it consumed.
# Nothing is enabled, so the timer's disabled state is left exactly as found —
# whoever disabled it in June may have done so for that cost.
set -eu

PATH="/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"
export PATH

UNIT=wisent-host-health.service

printf 'before: %s\n' "$(date -u +%H:%M:%SZ)"
systemctl start "$UNIT" 2>&1 | sed 's/^/  /' || printf '  start reported failure\n'

attempt=0
while [ "$attempt" -lt 12 ]; do
  state=$(systemctl is-active "$UNIT" 2>/dev/null || true)
  [ "$state" = "activating" ] || break
  sleep 5
  attempt=$((attempt + 1))
done

printf 'after: %s\n' "$(date -u +%H:%M:%SZ)"
printf '\n== journal for this run ==\n'
journalctl -u "$UNIT" -n 14 --no-pager 2>/dev/null | sed 's/^/  /'

printf '\n== timer left as found ==\n'
systemctl is-enabled wisent-host-health.timer 2>&1 | sed 's/^/  /' || true
systemctl is-active wisent-host-health.timer 2>&1 | sed 's/^/  /' || true
