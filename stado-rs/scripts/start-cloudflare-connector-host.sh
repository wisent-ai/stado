#!/bin/sh
# Attach this host to the existing bobloo Cloudflare Tunnel.
#
# Cloudflare still holds the public hostname route for `bobloo.com` — that is
# why it answers 502 rather than NXDOMAIN — but no connector has been attached
# since the GCP estate went away. The connector token is already installed
# here, owner-only, so nothing secret needs to travel to run this.
#
# The token is passed through the process environment, never through `argv`:
# a token on a command line is published to every `ps` reader on the host.
#
# Additive by design: it installs a connector and a LaunchAgent, and touches no
# existing service. Reverse it with
#   launchctl bootout gui/$(id -u)/com.wisent.cloudflared
#   rm ~/Library/LaunchAgents/com.wisent.cloudflared.plist
set -eu

LABEL=com.wisent.cloudflared
TOKEN_FILE="$HOME/.stado/cloudflared-token"
PLIST="$HOME/Library/LaunchAgents/$LABEL.plist"
RUNNER="$HOME/.stado/bin/cloudflared-run"
LOGS="$HOME/.stado/logs"
BREW=/opt/homebrew/bin/brew

[ -s "$TOKEN_FILE" ] || { printf '%s\n' "missing connector token: $TOKEN_FILE" >&2; exit 1; }

BIN=""
for candidate in /opt/homebrew/bin/cloudflared /usr/local/bin/cloudflared "$HOME/.stado/bin/cloudflared"; do
    [ -x "$candidate" ] && BIN="$candidate" && break
done
if [ -z "$BIN" ]; then
    [ -x "$BREW" ] || { printf '%s\n' "no cloudflared and no brew to install it" >&2; exit 1; }
    "$BREW" install cloudflared >/dev/null 2>&1 || true
    for candidate in /opt/homebrew/bin/cloudflared /usr/local/bin/cloudflared; do
        [ -x "$candidate" ] && BIN="$candidate" && break
    done
fi
[ -n "$BIN" ] || { printf '%s\n' "cloudflared install failed" >&2; exit 1; }

/bin/mkdir -p "$LOGS" "$HOME/.stado/bin" "$HOME/Library/LaunchAgents"

# The runner keeps the token out of argv and out of the plist.
/bin/cat > "$RUNNER" <<RUNNER_EOF
#!/bin/sh
set -eu
TUNNEL_TOKEN=\$(/bin/cat "$TOKEN_FILE")
export TUNNEL_TOKEN
exec "$BIN" tunnel --no-autoupdate --metrics 127.0.0.1:20241 run
RUNNER_EOF
/bin/chmod 0700 "$RUNNER"

/bin/cat > "$PLIST" <<PLIST_EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key><string>$LABEL</string>
    <key>ProgramArguments</key>
    <array><string>$RUNNER</string></array>
    <key>RunAtLoad</key><true/>
    <key>KeepAlive</key><true/>
    <key>ProcessType</key><string>Background</string>
    <key>StandardOutPath</key><string>$LOGS/cloudflared.log</string>
    <key>StandardErrorPath</key><string>$LOGS/cloudflared.log</string>
</dict>
</plist>
PLIST_EOF
/bin/chmod 0644 "$PLIST"

uid=$(/usr/bin/id -u)
# A helper runs without an Aqua session, where `gui/<uid>` answers
# "Domain does not support specified action" (125). The per-user domain is the
# one that exists headless, so try it first and keep `gui` as the fallback for
# an interactive operator session.
# Never swallow the reason: the first version discarded launchctl's stderr and
# reported only "no domain accepted", which says nothing about why.
domain=""
for candidate in "user/$uid" "gui/$uid"; do
    /bin/launchctl bootout "$candidate/$LABEL" >/dev/null 2>&1 || true
    err=$(/bin/launchctl bootstrap "$candidate" "$PLIST" 2>&1) && domain="$candidate" && break
    printf 'bootstrap %s -> %s\n' "$candidate" "$err" >&2
done
[ -n "$domain" ] || { printf '%s\n' "no launchd domain accepted $PLIST" >&2; exit 1; }
/bin/launchctl kickstart -k "$domain/$LABEL" >/dev/null 2>&1 || true
printf 'launchd_domain=%s\n' "$domain"

# Readiness is an attached connector, not a created process.
attached=no
for _ in 1 2 3 4 5 6 7 8 9 10 11 12; do
    /bin/sleep 5
    if /usr/bin/curl -fsS --max-time 5 http://127.0.0.1:20241/ready >/dev/null 2>&1; then
        attached=yes
        break
    fi
done
printf 'cloudflared=%s connector_ready=%s\n' "$BIN" "$attached"
/usr/bin/curl -fsS --max-time 5 http://127.0.0.1:20241/ready 2>/dev/null || true
printf '\n'
/usr/bin/tail -5 "$LOGS/cloudflared.log" 2>/dev/null || true
[ "$attached" = yes ]
