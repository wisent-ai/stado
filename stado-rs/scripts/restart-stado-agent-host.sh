#!/bin/sh
set -eu
case "$(uname -s)" in
  Linux)
    /bin/systemctl restart wisent-agent.service
    /bin/systemctl is-active wisent-agent.service
    ;;
  Darwin)
    /bin/launchctl kickstart -k system/com.wisent.compute.service.stado-agent
    /bin/launchctl print system/com.wisent.compute.service.stado-agent >/dev/null
    ;;
  *)
    printf '%s\n' "unsupported operating system" >&2
    exit 1
    ;;
esac
"$HOME/.stado/bin/stado" --version
