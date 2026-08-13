#!/bin/sh
set -eu
[ "$(uname -s)" = Linux ] || {
  printf '%s\n' "release state reset helper requires systemd" >&2
  exit 1
}
/bin/systemctl is-active --quiet wisent-agent.service
exec_start=$(/bin/systemctl show wisent-agent.service --property=ExecStart --value)
target=$(printf '%s\n' "$exec_start" | /bin/sed -n 's/.*--target[ =]\([^ ;}]*\).*/\1/p')
[ -n "$target" ] || {
  printf '%s\n' "wisent-agent.service has no --target" >&2
  exit 1
}
case "$target" in
  *[!a-z0-9-]*|'') {
    printf '%s\n' "refusing unsafe release target $target" >&2
    exit 1
  } ;;
esac
state_dir="$HOME/.stado/release-state"
state="$state_dir/image-video-router.json"
proxy="$state_dir/image-video-router.proxy.json"
removed=false
for path in "$state" "$proxy"; do
  if [ -f "$path" ]; then
    rm -f "$path"
    removed=true
  fi
done
printf '%s\n' "{\"removed\":$removed}"
