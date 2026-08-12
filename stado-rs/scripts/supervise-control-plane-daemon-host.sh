#!/bin/sh
# Split the canonical object API from the fleet coordinator and supervise both
# in launchd's system domain. The dashboard owns the host-local canonical store;
# the coordinator consumes that authenticated API through the host config.
set -eu

object_label=com.wisent.always-on.stado-object-api
coordinator_label=com.wisent.compute.coordinator.charless-control-plane
object_plist="/Library/LaunchDaemons/$object_label.plist"
coordinator_plist="/Library/LaunchDaemons/$coordinator_label.plist"
legacy_system_label=com.wisent.compute.coordinator
legacy_system_plist="/Library/LaunchDaemons/$legacy_system_label.plist"
legacy_user_plist="$HOME/Library/LaunchAgents/$legacy_system_label.plist"
legacy_user_charless_plist="$HOME/Library/LaunchAgents/$coordinator_label.plist"
owner=$(/usr/bin/id -un)
home=$HOME
binary="$home/.stado/bin/stado"
logs="$home/.stado/logs"
skarbiec_url=http://127.0.0.1:8895
port=8765

[ -x "$binary" ] || { printf '%s\n' "missing $binary" >&2; exit 1; }
/bin/mkdir -p "$logs"

/usr/bin/sudo -n /usr/bin/tee "$object_plist" > /dev/null <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>$object_label</string>
    <key>UserName</key>
    <string>$owner</string>
    <key>ProgramArguments</key>
    <array>
        <string>$binary</string>
        <string>dashboard</string>
        <string>--bind</string>
        <string>127.0.0.1</string>
        <string>--port</string>
        <string>$port</string>
    </array>
    <key>EnvironmentVariables</key>
    <dict>
        <key>HOME</key>
        <string>$home</string>
        <key>WC_SKARBIEC_URL</key>
        <string>$skarbiec_url</string>
        <key>WC_OBJECT_SKARBIEC_URL</key>
        <string>$skarbiec_url</string>
        <key>STADO_CONFIG</key>
        <string>$home/.config/stado/config.json</string>
        <key>WC_STORAGE_BACKEND</key>
        <string>local</string>
        <key>PATH</key>
        <string>/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin</string>
    </dict>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>$logs/$object_label.log</string>
    <key>StandardErrorPath</key>
    <string>$logs/$object_label.log</string>
</dict>
</plist>
PLIST

/usr/bin/sudo -n /usr/bin/tee "$coordinator_plist" > /dev/null <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>$coordinator_label</string>
    <key>UserName</key>
    <string>$owner</string>
    <key>ProgramArguments</key>
    <array>
        <string>$binary</string>
        <string>coordinator</string>
        <string>--target</string>
        <string>always-on</string>
    </array>
    <key>EnvironmentVariables</key>
    <dict>
        <key>HOME</key>
        <string>$home</string>
        <key>WC_SKARBIEC_URL</key>
        <string>$skarbiec_url</string>
        <key>STADO_CONFIG</key>
        <string>$home/.config/stado/config.json</string>
        <key>PATH</key>
        <string>/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin</string>
    </dict>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>$logs/$coordinator_label.log</string>
    <key>StandardErrorPath</key>
    <string>$logs/$coordinator_label.log</string>
</dict>
</plist>
PLIST

for plist in "$object_plist" "$coordinator_plist"; do
  /usr/bin/sudo -n /usr/sbin/chown root:wheel "$plist"
  /usr/bin/sudo -n /bin/chmod 644 "$plist"
done
/usr/bin/sudo -n /bin/launchctl bootout "system/$legacy_system_label" >/dev/null 2>&1 || true
/usr/bin/sudo -n /bin/rm -f "$legacy_system_plist"
/bin/rm -f "$legacy_user_plist" "$legacy_user_charless_plist"

/usr/bin/sudo -n /bin/launchctl bootout "system/$coordinator_label" >/dev/null 2>&1 || true
/usr/bin/sudo -n /bin/launchctl bootout "system/$object_label" >/dev/null 2>&1 || true
for pid in $(/bin/ps axww -o pid= -o command= | /usr/bin/awk -v binary="$binary" \
  '$2 == binary && $3 == "coordinator" && $4 == "--target" && $5 == "always-on" {print $1}'); do
  /bin/kill -TERM "$pid" 2>/dev/null || true
done

/usr/bin/sudo -n /bin/launchctl bootstrap system "$object_plist"
waited=0
while [ "$waited" -lt 60 ]; do
  if /usr/sbin/lsof -nP -iTCP:"$port" -sTCP:LISTEN -Fc 2>/dev/null \
    | /usr/bin/grep -qx cstado; then
    break
  fi
  /bin/sleep 2
  waited=$((waited + 2))
done
if [ "$waited" -ge 60 ]; then
  printf '%s\n' "object API did not bind port $port" >&2
  /usr/bin/tail -n 30 "$logs/$object_label.log" >&2 || true
  exit 1
fi

/usr/bin/sudo -n /bin/launchctl bootstrap system "$coordinator_plist"
/bin/sleep 2
/bin/launchctl print "system/$coordinator_label" \
  | /usr/bin/grep -q 'active count = 1'
printf '{"object_api":"%s","coordinator":"%s","port":%s,"state":"supervised"}\n' \
  "$object_label" "$coordinator_label" "$port"
