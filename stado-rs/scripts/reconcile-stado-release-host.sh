#!/bin/sh
set -eu
[ "$(uname -s)" = Linux ] || {
  printf '%s\n' "release reconciliation helper requires systemd" >&2
  exit 1
}
/bin/systemctl is-active --quiet wisent-agent.service
environment=$(/bin/systemctl show wisent-agent.service --property=Environment --value)
exec_start=$(/bin/systemctl show wisent-agent.service --property=ExecStart --value)
target=$(printf '%s\n' "$exec_start" | /bin/sed -n 's/.*--target[ =]\([^ ;}]*\).*/\1/p')
[ -n "$target" ] || {
  printf '%s\n' "wisent-agent.service has no --target" >&2
  exit 1
}
exec /usr/bin/env -S "$environment" "$HOME/.stado/bin/stado" \
  release agent --target "$target" --once --json
