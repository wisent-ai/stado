#!/bin/sh
set -eu
case "$(uname -s)" in
  Linux)
    /bin/systemctl is-active wisent-agent.service
    /bin/systemctl show wisent-agent.service --property=EnvironmentFiles --value
    /bin/systemctl show wisent-agent.service \
      --property=ExecStart,After,Wants,Requires,Restart,MainPID,ActiveState,SubState
    "$HOME/.stado/bin/stado" --version
    /bin/journalctl -u wisent-agent.service --no-pager --all --output=cat -n 300 \
      --grep 'RAM gate|disk|claim|queue|reject|error|failed|cleanup|release|manifest|inference|yieldable|pause'
    ;;
  Darwin)
    printf '%s\n' "no loaded Stado agent service"
    ;;
  *)
    printf '%s\n' "unsupported operating system" >&2
    exit 1
    ;;
esac
