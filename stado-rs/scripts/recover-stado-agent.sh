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
    set -- "$HOME"/Library/LaunchAgents/com.wisent.compute.agent.*.plist
    [ "$#" -eq 1 ] && [ -f "$1" ]
    plist="$1"
    label="${plist##*/}"
    label="${label%.plist}"
    /bin/launchctl bootout "gui/$uid/$label" 2>/dev/null || true
    /bin/sleep 1
    /bin/launchctl bootstrap "gui/$uid" "$plist"
    /bin/launchctl print "gui/$uid/$label" >/dev/null
    printf '%s\n' active
    ;;
  *)
    printf 'unsupported operating system\n' >&2
    exit 1
    ;;
esac
