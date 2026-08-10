#!/bin/sh
# Report why this Linux host's Stado health beacon stopped publishing.
#
# Read-only: unit and timer state plus the tail of the publisher's own journal.
# Run through `stado host install-helper` + `run-helper`; it takes no arguments,
# so it names nothing host-specific and is safe to leave installed.
set -eu

PATH="/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"
export PATH

printf 'now: %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)"

printf '\n== units mentioning health or beacon ==\n'
systemctl list-units --all --no-legend --no-pager 2>/dev/null \
  | grep -iE 'health|beacon' | sed 's/^/  /' || printf '  (none)\n'

printf '\n== timers ==\n'
systemctl list-timers --all --no-legend --no-pager 2>/dev/null \
  | grep -iE 'health|beacon' | sed 's/^/  /' || printf '  (none)\n'

for unit in $(systemctl list-unit-files --no-legend --no-pager 2>/dev/null \
  | grep -iE 'health|beacon' | awk '{print $1}'); do
  printf '\n== %s ==\n' "$unit"
  systemctl status "$unit" --no-pager -n 0 2>/dev/null | sed -n '1,6p' | sed 's/^/  /'
  journalctl -u "$unit" -n 12 --no-pager 2>/dev/null | sed 's/^/  /' || true
done

printf '\n== beacon script on disk ==\n'
for candidate in "$HOME/.stado/bin/host_health_beacon_linux.sh" \
                 "$HOME/.stado/bin/host_health_beacon.sh" \
                 /usr/local/bin/host_health_beacon.sh; do
  [ -f "$candidate" ] && printf '  %s (mtime %s)\n' "$candidate" "$(date -u -r "$candidate" +%H:%M:%SZ 2>/dev/null)"
done

printf '\n== installed stado ==\n'
[ -x "$HOME/.stado/bin/stado" ] && "$HOME/.stado/bin/stado" --version 2>/dev/null | sed 's/^/  /' || printf '  (no stado binary)\n'
