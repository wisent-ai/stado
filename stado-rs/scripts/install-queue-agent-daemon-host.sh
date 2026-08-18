#!/bin/sh
# Run this host's queue agent as a system LaunchDaemon.
#
# Why a daemon and not `stado service deploy`: that renderer bootstraps into the
# per-user domain, and over SSH there is no Aqua session, so launchd answers
# "Could not switch to audit session ... Operation not permitted". The same wall
# stopped the tunnel connector earlier today, and the same answer applies.
#
# Why this exists at all: the agent that claims fleet jobs was a hand-started,
# disowned process. It had outlived four days and several stado deliveries, so
# it kept executing the binary it started with while `stado --version` on disk
# read 0.7.4, and release builds queued behind it forever. A declared unit is
# restartable, survives reboots, and can be adopted into the registry.
set -u

LABEL=com.wisent.stado.queue-agent
PLIST="/Library/LaunchDaemons/$LABEL.plist"
BIN=/Users/charles/.stado/bin/stado
TARGET=control-host
LOG=/Users/charles/.stado/logs/queue-agent.log

[ -x "$BIN" ] || { printf 'missing stado binary: %s\n' "$BIN" >/dev/stderr; exit 1; }

tmp=$(/usr/bin/mktemp /tmp/queue-agent-plist.XXXXXX)
/bin/cat > "$tmp" <<PLIST_EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>$LABEL</string>
  <key>UserName</key><string>charles</string>
  <key>ProgramArguments</key>
  <array>
    <string>$BIN</string>
    <string>agent</string>
    <string>--target</string>
    <string>$TARGET</string>
  </array>
  <key>EnvironmentVariables</key>
  <dict>
    <key>HOME</key><string>/Users/charles</string>
    <key>PATH</key><string>/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin</string>
  </dict>
  <key>WorkingDirectory</key><string>/Users/charles</string>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>StandardOutPath</key><string>$LOG</string>
  <key>StandardErrorPath</key><string>$LOG</string>
</dict>
</plist>
PLIST_EOF

/usr/bin/sudo -n /usr/bin/install -m 0644 -o root -g wheel "$tmp" "$PLIST" || {
  printf 'could not install %s\n' "$PLIST" >/dev/stderr; /bin/rm -f "$tmp"; exit 1; }
/bin/rm -f "$tmp"
/usr/bin/plutil -lint "$PLIST" >/dev/null || { printf 'plist did not lint\n' >/dev/stderr; exit 1; }

# Restart in place when it is already loaded; bootstrap only when it is not.
# bootout-then-bootstrap raced a still-terminating job on this host before.
if /usr/bin/sudo -n /bin/launchctl print "system/$LABEL" >/dev/null 2>&1; then
  /usr/bin/sudo -n /bin/launchctl kickstart -k "system/$LABEL" >/dev/null 2>&1 || true
  printf 'action=restarted\n'
else
  err=$(/usr/bin/sudo -n /bin/launchctl bootstrap system "$PLIST" 2>&1) || {
    printf 'bootstrap failed: %s\n' "$err" >/dev/stderr; exit 1; }
  printf 'action=bootstrapped\n'
fi

/bin/sleep 12
pid=$(/usr/bin/sudo -n /bin/launchctl print "system/$LABEL" 2>/dev/null | /usr/bin/awk '$1=="pid"{print $3;exit}')
printf 'label=%s pid=%s\n' "$LABEL" "${pid:-none}"
/usr/bin/tail -4 "$LOG" 2>/dev/null | /usr/bin/cut -c1-160
[ -n "${pid:-}" ] || exit 1
