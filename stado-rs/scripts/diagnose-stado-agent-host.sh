#!/bin/sh
set -eu
case "$(uname -s)" in
  Linux)
    /bin/systemctl is-active wisent-agent.service
    /bin/systemctl show wisent-agent.service \
      --property=User --property=ExecStart --property=EnvironmentFiles
    "$HOME/.stado/bin/stado" --version
    /bin/journalctl -u wisent-agent.service --no-pager -n 300 \
      --grep 'Agent started|init:|iter-start|pre-drain|capacity|RAM gate|disk|claim|queue|reject|error|failed|cleanup|release|manifest|inference|yieldable|pause' \
      | /usr/bin/cut -c1-600
    ;;
  Darwin)
    printf '%s\n' "no loaded Stado agent service"
    ;;
  *)
    printf '%s\n' "unsupported operating system" >&2
    exit 1
    ;;
esac
