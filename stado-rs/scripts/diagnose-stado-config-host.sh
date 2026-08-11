#!/bin/sh
set -eu
[ "$(uname -s)" = Linux ] || {
  printf '%s\n' "Stado config diagnostic helper requires systemd" >&2
  exit 1
}
/bin/systemctl is-active --quiet wisent-agent.service
environment=$(/bin/systemctl show wisent-agent.service --property=Environment --value)
/usr/bin/env -S "$environment" "$HOME/.stado/bin/stado" config show \
  | /usr/bin/grep -E 'storage|stado_storage|bucket|local_storage|api|agent_skarbiec'
