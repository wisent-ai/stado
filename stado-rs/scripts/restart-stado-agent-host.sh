#!/bin/sh
set -eu
case "$(uname -s)" in
  Linux)
    /bin/systemctl restart wisent-agent.service
    /bin/systemctl is-active wisent-agent.service
    ;;
  Darwin)
    printf '%s\n' "no loaded Stado agent service; binary installation is complete"
    ;;
  *)
    printf '%s\n' "unsupported operating system" >&2
    exit 1
    ;;
esac
"$HOME/.stado/bin/stado" --version
