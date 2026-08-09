#!/bin/sh
set -eu

case "$(uname -s)" in
  Linux)
    state="$(/bin/systemctl is-active wisent-agent.service || true)"
    if [ "$state" = active ]; then
      pid="$(/bin/systemctl show wisent-agent.service --property=MainPID --value)"
      [ "$pid" -gt 1 ]
      /bin/kill -TERM "$pid"
      /bin/sleep 2
    else
      /bin/systemctl start wisent-agent.service
    fi
    state="$(/bin/systemctl is-active wisent-agent.service)"
    [ "$state" = active ]
    printf '%s\n' "$state"
    ;;
  Darwin)
    uid="$(/usr/bin/id -u)"
    label="$(/bin/launchctl list | /usr/bin/awk '$3 ~ /^com[.]wisent[.]compute[.]agent[.]/ {print $3}')"
    [ -n "$label" ]
    /bin/launchctl kickstart -k "gui/$uid/$label"
    /bin/launchctl print "gui/$uid/$label" >/dev/null
    printf '%s\n' active
    ;;
  *)
    printf 'unsupported operating system\n' >&2
    exit 1
    ;;
esac
