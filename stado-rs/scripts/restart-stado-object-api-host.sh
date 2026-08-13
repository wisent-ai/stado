#!/bin/sh
set -eu

[ "$(uname -s)" = Darwin ] || {
  printf '%s\n' "Stado object API restart requires launchd" >&2
  exit 1
}

uid=$(/usr/bin/id -u)
label=com.wisent.compute.service.stado-object-api
plist="$HOME/Library/LaunchAgents/$label.plist"
binary="$HOME/.stado/bin/stado"

[ -f "$plist" ]
/bin/launchctl bootout "gui/$uid/$label" 2>/dev/null || true
for pid in $(/usr/sbin/lsof -t -nP -iTCP:18765 -sTCP:LISTEN 2>/dev/null || true)
do
  command=$(/bin/ps -p "$pid" -o comm= | /usr/bin/xargs)
  [ "$command" = "$binary" ] || {
    printf 'refusing to stop unexpected port 18765 listener: %s (%s)\n' "$pid" "$command" >&2
    exit 1
  }
  /bin/kill -TERM "$pid"
done
/bin/sleep 1
/bin/launchctl bootstrap "gui/$uid" "$plist"
/bin/sleep 1
/bin/launchctl print "gui/$uid/$label" >/dev/null
/usr/sbin/lsof -nP -iTCP:18765 -sTCP:LISTEN
