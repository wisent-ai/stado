#!/bin/sh
set -eu

label=com.wisent.always-on.weles
plist=/Library/LaunchDaemons/com.wisent.always-on.weles.plist
if [ ! -f "$plist" ] || [ -L "$plist" ]; then
  printf '%s\n' "missing regular Weles LaunchDaemon: $plist" >&2
  exit 1
fi

/usr/bin/sudo -n /bin/launchctl bootstrap system "$plist"
/usr/bin/sudo -n /bin/launchctl enable "system/$label"
/usr/bin/sudo -n /bin/launchctl kickstart -k "system/$label"
printf '%s\n' 'Weles system service recovered'
