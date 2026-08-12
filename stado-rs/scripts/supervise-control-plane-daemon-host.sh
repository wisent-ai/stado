#!/bin/sh
# Supervise this host's control plane as a system LaunchDaemon.
#
# Agent domains are unreachable from SSH on this Mac -- `user/<uid>` answers
# "Input/output error" and `gui/<uid>` answers "Domain does not support
# specified action", because neither a user nor an Aqua session exists for a
# remote shell. The system domain does bootstrap over SSH, which is why every
# other always-on unit on this host lives there.
#
# The job keeps the original command and the original user: this exists to put
# a supervisor under a process that had none, not to change what the host runs.
# Root would resolve $HOME to /var/root and take the config, the store and the
# grants with it.
#
# If the supervised job does not bind the port, the plist is removed and the
# previous process is relaunched, so a failed attempt leaves the fleet's
# endpoint exactly as it found it.
set -eu

label=com.wisent.compute.coordinator.charless-control-plane
plist="/Library/LaunchDaemons/$label.plist"
owner=$(/usr/bin/id -un)
home=$HOME
binary="$home/.stado/bin/stado"
logs="$home/.stado/logs"
skarbiec_url=http://127.0.0.1:8895
# This is the fleet coordinator, not the single-device onboarding profile.
# Storage, provider selection, and credentials come from the host's Stado
# config; forcing WC_STORAGE_BACKEND=local would fork canonical queue state.
port=8765

[ -x "$binary" ] || { printf '%s\n' "missing $binary" >&2; exit 1; }
/bin/mkdir -p "$logs"

bound() {
  /usr/sbin/lsof -nP -iTCP:"$port" -sTCP:LISTEN -Fc 2>/dev/null | /usr/bin/grep -qx cstado
}

relaunch_detached() {
  WC_SKARBIEC_URL="$skarbiec_url" \
    /usr/bin/nohup "$binary" cloud-control-plane \
      --bind 127.0.0.1 --port "$port" --interval 30 \
    < /dev/null >> "$logs/stado-cloud-control-plane.log" 2>&1 &
}

/usr/bin/sudo -n /usr/bin/tee "$plist" > /dev/null <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>$label</string>
    <key>UserName</key>
    <string>$owner</string>
    <key>ProgramArguments</key>
    <array>
        <string>$binary</string>
        <string>cloud-control-plane</string>
        <string>--bind</string>
        <string>127.0.0.1</string>
        <string>--port</string>
        <string>$port</string>
        <string>--interval</string>
        <string>30</string>
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
    <string>$logs/$label.log</string>
    <key>StandardErrorPath</key>
    <string>$logs/$label.log</string>
</dict>
</plist>
PLIST
/usr/bin/sudo -n /usr/sbin/chown root:wheel "$plist"
/usr/bin/sudo -n /bin/chmod 644 "$plist"

previous=$(
  /usr/sbin/lsof -nP -iTCP:"$port" -sTCP:LISTEN -Fpc 2>/dev/null \
    | /usr/bin/awk '/^p/ {pid=substr($0,2)} /^cstado$/ {print pid}'
)
/usr/bin/sudo -n /bin/launchctl bootout "system/$label" >/dev/null 2>&1 || true
for pid in $previous; do
  /bin/kill -TERM "$pid" 2>/dev/null || true
done

if ! error=$(/usr/bin/sudo -n /bin/launchctl bootstrap system "$plist" 2>&1); then
  /usr/bin/sudo -n /bin/rm -f "$plist"
  relaunch_detached
  printf 'bootstrap system failed: %s; previous process relaunched\n' "$error" >&2
  exit 1
fi

waited=0
while [ "$waited" -lt 150 ]; do
  if bound; then
    printf '{"label":"%s","domain":"system","port":%s,"state":"supervised","waited_seconds":%s}\n' \
      "$label" "$port" "$waited"
    exit 0
  fi
  /bin/sleep 2
  waited=$((waited + 2))
done

/usr/bin/sudo -n /bin/launchctl bootout "system/$label" >/dev/null 2>&1 || true
/usr/bin/sudo -n /bin/rm -f "$plist"
relaunch_detached
printf '%s\n' "supervised job did not bind $port; previous process relaunched" >&2
/usr/bin/tail -n 20 "$logs/$label.log" >&2 || true
exit 1
