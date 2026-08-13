#!/bin/sh
# Put this host's control plane under launchd, without ever dropping the port.
#
# It runs as an unmanaged process: nothing restarts it and nothing survives a
# reboot. An earlier attempt killed it first and could not bootstrap a
# replacement, which took the endpoint down; the order here is reversed, so the
# running process is only retired once a supervised one has bound the port.
#
# `launchctl bootstrap gui/<uid>` refuses over SSH ("could not switch to audit
# session"), which is why `stado service deploy` cannot install this one. Both
# the `user/<uid>` and `gui/<uid>` domains are attempted and their real errors
# reported, because "it failed" is not a diagnosis.
#
# The job binds a staging port first. Only after it is proven up is the
# unmanaged process retired and the job moved onto the real port.
set -eu
umask 077

label=com.wisent.compute.service.stado-local-control-plane
plist="$HOME/Library/LaunchAgents/$label.plist"
logs="$HOME/.stado/logs"
binary="$HOME/.stado/bin/stado"
skarbiec_url=http://127.0.0.1:8895
staging_port=18766
uid=$(/usr/bin/id -u)

[ -x "$binary" ] || { printf '%s\n' "missing $binary" >&2; exit 1; }
/bin/mkdir -p "$logs" "$HOME/Library/LaunchAgents"

write_plist() {
  /bin/cat > "$plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>$label</string>
    <key>ProgramArguments</key>
    <array>
        <string>$binary</string>
        <string>dashboard</string>
        <string>--bind</string>
        <string>127.0.0.1</string>
        <string>--port</string>
        <string>$1</string>
    </array>
    <key>EnvironmentVariables</key>
    <dict>
        <key>WC_SKARBIEC_URL</key>
        <string>$skarbiec_url</string>
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
    <string>$logs/$label.log</string>
    <key>StandardErrorPath</key>
    <string>$logs/$label.log</string>
</dict>
</plist>
PLIST
}

bound() {
  /usr/sbin/lsof -nP -iTCP:"$1" -sTCP:LISTEN -Fc 2>/dev/null | /usr/bin/grep -qx cstado
}

write_plist "$staging_port"
/bin/launchctl bootout "user/$uid/$label" >/dev/null 2>&1 || true
/bin/launchctl bootout "gui/$uid/$label" >/dev/null 2>&1 || true

domain=
for candidate in "user/$uid" "gui/$uid"; do
  error=$(/bin/launchctl bootstrap "$candidate" "$plist" 2>&1) && { domain=$candidate; break; }
  printf 'bootstrap %s failed: %s\n' "$candidate" "$error" >&2
done
[ -n "$domain" ] || { printf '%s\n' "no launchd domain accepted the job; nothing was changed" >&2; exit 1; }

waited=0
while [ "$waited" -lt 150 ] && ! bound "$staging_port"; do
  /bin/sleep 2
  waited=$((waited + 2))
done
if ! bound "$staging_port"; then
  /bin/launchctl bootout "$domain/$label" >/dev/null 2>&1 || true
  printf '%s\n' "supervised job never bound the staging port; nothing was changed" >&2
  /usr/bin/tail -n 20 "$logs/$label.log" >&2 || true
  exit 1
fi

printf '{"label":"%s","domain":"%s","staging_port":%s,"state":"supervised-and-proven"}\n' \
  "$label" "$domain" "$staging_port"
