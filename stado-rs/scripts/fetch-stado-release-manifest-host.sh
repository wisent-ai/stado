#!/bin/sh
set -eu
[ "$(uname -s)" = Linux ] || {
  printf '%s\n' "release fetch helper requires systemd" >&2
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
uri='stado://releases/image-video-router/0.1.0/linux-amd64/release.json'
environment_sha=$(
  /usr/bin/env -S "$environment" "$HOME/.stado/bin/stado" storage get "$uri" - \
    | /usr/bin/sha256sum | /usr/bin/cut -d' ' -f1
)
public_sha=$(
  /usr/bin/curl --fail --silent --show-error \
    --get --data-urlencode "uri=$uri" \
    'http://127.0.0.1:18765/api/release/object' \
    | /usr/bin/sha256sum | /usr/bin/cut -d' ' -f1
)
printf '%s\n' "{\"environment\":\"$environment_sha\",\"public\":\"$public_sha\"}"
