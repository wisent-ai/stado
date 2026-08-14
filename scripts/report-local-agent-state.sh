#!/bin/sh
# Report the local Stado agent and its recent journal without changing service state.
# Run through `stado host install-helper` + `run-helper`; it accepts no input.
set -eu

PATH="/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"
export PATH

printf 'now: %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
printf '\n== user unit ==\n'
systemctl --user status stado-local-agent --no-pager -n 0 2>&1 | sed -n '1,16p' || true
printf '\n== recent user journal ==\n'
journalctl --user -u stado-local-agent -n 80 --no-pager 2>&1 | sed 's/^/  /' || true
printf '\n== system unit ==\n'
systemctl status stado-local-agent --no-pager -n 0 2>&1 | sed -n '1,16p' || true
printf '\n== recent system journal ==\n'
journalctl -u stado-local-agent -n 80 --no-pager 2>&1 | sed 's/^/  /' || true
printf '\n== matching processes ==\n'
ps ax -o user= -o pid= -o ppid= -o etime= -o command= 2>/dev/null \
  | grep -iE '[s]tado( |$).*agent|[s]tado-local-agent' \
  | sed 's/^/  /' || printf '  (none)\n'
