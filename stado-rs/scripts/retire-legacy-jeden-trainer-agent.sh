#!/bin/sh
set -eu

[ "$(uname -s)" = Darwin ] || {
  printf '%s\n' "legacy Jeden trainer agent retirement requires launchd" >&2
  exit 1
}

uid=$(/usr/bin/id -u)
label=com.wisent.compute.service.jeden-trainer-agent
plist="$HOME/Library/LaunchAgents/$label.plist"
launcher="$HOME/.stado/bin/stado-local-agent"

/bin/launchctl bootout "gui/$uid/$label" 2>/dev/null || true
/bin/rm -f "$plist" "$launcher"
if /bin/launchctl print "gui/$uid/$label" >/dev/null 2>&1
then
  printf '%s\n' "$label is still loaded" >&2
  exit 1
fi
printf '%s\n' "$label retired"
