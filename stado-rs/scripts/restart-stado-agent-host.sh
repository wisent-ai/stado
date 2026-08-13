#!/bin/sh
set -eu
case "$(uname -s)" in
  Linux)
    /bin/systemctl restart wisent-agent.service
    /bin/systemctl is-active wisent-agent.service
    ;;
  Darwin)
    uid=$(/usr/bin/id -u)
    found=false
    for plist in "$HOME"/Library/LaunchAgents/com.wisent.compute.agent.*.plist
    do
      [ -f "$plist" ] || continue
      found=true
      label=$(/usr/bin/basename "$plist" .plist)
      /bin/launchctl bootout "gui/$uid/$label" 2>/dev/null || true
      /bin/sleep 1
      /bin/launchctl bootstrap "gui/$uid" "$plist"
      /bin/launchctl kickstart -k "gui/$uid/$label"
      /bin/launchctl print "gui/$uid/$label" >/dev/null
      printf '%s\n' "$label active"
    done
    "$found" || {
      printf '%s\n' "no Stado agent launch unit found" >&2
      exit 1
    }
    ;;
  *)
    printf '%s\n' "unsupported operating system" >&2
    exit 1
    ;;
esac
"$HOME/.stado/bin/stado" --version
