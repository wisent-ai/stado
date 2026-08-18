#!/bin/sh
# Stop queue agents that no launchd unit owns.
#
# Two `stado agent` processes had been running for four days with no unit
# behind them: nothing restarted them, nothing updated them, and they kept
# executing the binary they were launched with. Now that the agent is a declared
# daemon, an unowned copy is a second claimant racing the managed one for the
# same jobs.
#
# Only processes that are NOT the managed daemon's own tree are stopped, and
# each pid is reported with the command it was running, so a stop is auditable.
set -u

LABEL=com.wisent.stado.queue-agent
managed=$(/usr/bin/sudo -n /bin/launchctl print "system/$LABEL" 2>/dev/null | /usr/bin/awk '$1=="pid"{print $3;exit}')
printf 'managed_pid=%s\n' "${managed:-none}"
[ -n "${managed:-}" ] || { printf 'refusing to stop anything without a managed agent\n' >/dev/stderr; exit 1; }

/bin/ps ax -o pid=,ppid=,command= \
| /usr/bin/grep -E '/\.stado/bin/stado agent( |$)' \
| /usr/bin/grep -v grep \
| while read -r pid ppid rest; do
    [ "$pid" = "$managed" ] && continue
    [ "$ppid" = "$managed" ] && continue
    printf 'stopping pid=%s ppid=%s cmd=%s\n' "$pid" "$ppid" "$(printf '%s' "$rest" | /usr/bin/cut -c1-70)"
    /usr/bin/sudo -n /bin/kill -TERM "$pid" 2>/dev/null || true
  done

/bin/sleep 8
printf '== remaining agents ==\n'
/bin/ps ax -o pid=,etime=,command= | /usr/bin/grep -E '/\.stado/bin/stado agent( |$)' \
  | /usr/bin/grep -v grep | /usr/bin/cut -c1-100
