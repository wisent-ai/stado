#!/bin/sh
set -eu

case "$(uname -s)" in
  Linux)
    /bin/systemctl restart wisent-agent.service
    state="$(/bin/systemctl is-active wisent-agent.service)"
    [ "$state" = active ]
    printf '%s\n' "$state"
    ;;
  Darwin)
    uid="$(/usr/bin/id -u)"
    label="com.wisent.compute.agent"
    /bin/launchctl kickstart -k "gui/$uid/$label"
    /bin/launchctl print "gui/$uid/$label" >/dev/null
    printf '%s\n' active
    ;;
  *)
    printf 'unsupported operating system\n' >&2
    exit 1
    ;;
esac
