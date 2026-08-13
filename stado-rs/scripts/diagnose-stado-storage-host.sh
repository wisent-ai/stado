#!/bin/sh
set -eu
[ "$(uname -s)" = Linux ] || {
  printf '%s\n' "storage fetch diagnostic helper requires systemd" >&2
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
cat_uri='registry.json'
get_uri='probierz/registry.json'
cat_output=$(/usr/bin/mktemp)
get_output=$(/usr/bin/mktemp)
trap '/bin/rm -f "$cat_output" "$get_output"' EXIT HUP INT TERM
/usr/bin/env -S "$environment" "$HOME/.stado/bin/stado" storage cat "$cat_uri" >"$cat_output"
/usr/bin/env -S "$environment" "$HOME/.stado/bin/stado" storage get "$get_uri" "$get_output"
cat_sha=$(/usr/bin/sha256sum "$cat_output" | /usr/bin/cut -d' ' -f1)
get_sha=$(/usr/bin/sha256sum "$get_output" | /usr/bin/cut -d' ' -f1)
printf '%s\n' "{\"cat\":\"$cat_sha\",\"get\":\"$get_sha\"}"
