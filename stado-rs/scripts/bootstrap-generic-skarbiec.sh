#!/bin/sh
set -eu

label=com.wisent.always-on.skarbiec
plist=/Library/LaunchDaemons/$label.plist
skarbiec="$HOME/.stado/bin/skarbiec"

if [ ! -x "$skarbiec" ] || [ ! -f "$plist" ]; then
  printf '%s\n' 'generic Skarbiec binary and managed plist are required' >&2
  exit 1
fi

arguments=$(/usr/libexec/PlistBuddy -c 'Print :ProgramArguments' "$plist")
printf '%s\n' "$arguments" | /usr/bin/grep -F "$skarbiec" >/dev/null
printf '%s\n' "$arguments" | /usr/bin/grep -F '    serve' >/dev/null
printf '%s\n' "$arguments" | /usr/bin/grep -F '    --port' >/dev/null
printf '%s\n' "$arguments" | /usr/bin/grep -F '    8895' >/dev/null

if /bin/launchctl print "system/$label" >/dev/null 2>&1; then
  printf '%s\n' 'generic Skarbiec already loaded'
  exit 0
fi

/usr/bin/sudo -n /bin/launchctl bootstrap system "$plist"
/bin/launchctl print "system/$label" | /usr/bin/sed -n '1,80p'
