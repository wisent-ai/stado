#!/bin/sh
# Put the local control plane under launchd supervision on this host.
#
# `stado local-control-plane` was running as an orphan started on 5 August: its
# LaunchAgent plist exists but the label is in no launchd domain, because nobody
# is logged in graphically here and `gui/501` does not exist. Nothing restarts
# it, and it cannot be reloaded to pick up config changes. The other always-on
# units on this host are system LaunchDaemons running as charles; this makes the
# control plane one of them.
set -eu

label=com.wisent.compute.coordinator.charless-control-plane
plist=/Library/LaunchDaemons/$label.plist
program=/Users/charles/.stado/bin/stado
sudo="/usr/bin/sudo -n"

if [ ! -x "$program" ]; then
    printf '%s\n' "no stado binary at $program" >&2
    exit 1
fi

/usr/bin/sudo -n /usr/bin/tee "$plist" >/dev/null <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>$label</string>
  <key>UserName</key><string>charles</string>
  <key>ProgramArguments</key>
  <array>
    <string>$program</string>
    <string>local-control-plane</string>
  </array>
  <key>EnvironmentVariables</key>
  <dict>
    <key>HOME</key><string>/Users/charles</string>
    <key>PATH</key><string>/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin</string>
  </dict>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>ProcessType</key><string>Background</string>
  <key>SoftResourceLimits</key><dict><key>NumberOfFiles</key><integer>65536</integer></dict>
  <key>HardResourceLimits</key><dict><key>NumberOfFiles</key><integer>65536</integer></dict>
  <key>StandardOutPath</key><string>/Users/charles/.stado/logs/$label.log</string>
  <key>StandardErrorPath</key><string>/Users/charles/.stado/logs/$label.log</string>
</dict>
</plist>
PLIST

$sudo /usr/bin/plutil -lint "$plist" >/dev/null
$sudo /usr/sbin/chown root:wheel "$plist"
$sudo /bin/chmod 644 "$plist"

$sudo /usr/bin/pkill -TERM -f 'stado local-control-plane' >/dev/null 2>&1 || true
/bin/sleep 2
$sudo /usr/bin/pkill -KILL -f 'stado local-control-plane' >/dev/null 2>&1 || true

$sudo /bin/launchctl bootout "system/$label" >/dev/null 2>&1 || true
$sudo /bin/launchctl enable "system/$label" >/dev/null 2>&1 || true
$sudo /bin/launchctl bootstrap system "$plist"

n=0
while [ "$n" -lt 40 ]; do
    if /usr/bin/curl -s -o /dev/null --max-time 3 http://127.0.0.1:8765/ ; then
        break
    fi
    n=$((n + 1))
    /bin/sleep 1
done

printf 'control plane http: %s\n' "$(/usr/bin/curl -s -o /dev/null -w '%{http_code}' --max-time 5 http://127.0.0.1:8765/ || echo none)"
$sudo /bin/launchctl print "system/$label" 2>/dev/null | /usr/bin/grep -E 'state = |pid = ' | head -2
/bin/rm -f "$0"
