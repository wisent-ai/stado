#!/bin/sh
set -eu
case "$(uname -s)" in
  Linux)
    /bin/systemctl is-active wisent-agent.service
    /bin/journalctl -u wisent-agent.service --no-pager -n 300 \
      --grep 'RAM gate|disk|claim|queue|reject|error|failed|cleanup|release'
    ;;
  Darwin)
    printf '%s\n' "no loaded Stado agent service"
    ;;
  *)
    printf '%s\n' "unsupported operating system" >&2
    exit 1
    ;;
esac
