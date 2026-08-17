#!/bin/sh
# Attach this host to the existing bobloo Cloudflare Tunnel.
#
# Cloudflare still holds the public hostname route for `bobloo.com` — that is
# why it answers 502 rather than NXDOMAIN — but no connector has been attached
# since the GCP estate went away. The connector token is already installed
# here, owner-only, so nothing secret needs to travel to run this.
#
# Installed as a LaunchDaemon, which is what durable services on this host use
# (`com.wisent.compute.service.wisent-backend-api` is one). The per-user domain
# rejected a LaunchAgent with `5: Input/output error` and `gui/<uid>` does not
# exist in a headless session, so `system` is the only domain that both accepts
# a bootstrap here and survives a reboot.
#
# The token is passed through the process environment, never through `argv`:
# a token on a command line is published to every `ps` reader on the host.
#
# Additive by design: it installs a connector and one new unit, and touches no
# existing service. Reverse it with
#   sudo launchctl bootout system/com.wisent.cloudflared
#   sudo rm /Library/LaunchDaemons/com.wisent.cloudflared.plist
set -eu

LABEL=com.wisent.cloudflared
TOKEN_FILE="$HOME/.stado/cloudflared-token"
PLIST="/Library/LaunchDaemons/$LABEL.plist"
RUNNER="$HOME/.stado/bin/cloudflared-run"
LOGS="$HOME/.stado/logs"
BREW=/opt/homebrew/bin/brew
METRICS=127.0.0.1:20241

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

/bin/mkdir -p "$LOGS" "$HOME/.stado/bin"

# The runner keeps the token out of argv and out of the unit file.
#
# Backticks are forbidden in the body below: this heredoc is unquoted so the
# host expands it, and a backticked word in a comment is executed as a command
# there. An earlier version explained the transport choice that way and the
# helper reported three "command not found" lines while still writing a correct
# runner, which is the worst combination to debug.
/bin/cat > "$RUNNER" <<RUNNER_EOF
#!/bin/sh
set -eu
TUNNEL_TOKEN=\$(/bin/cat "$TOKEN_FILE")
export TUNNEL_TOKEN
# Transport stays at the connector's default, but the edge address family does
# not: this host has no working IPv6 path. The connector log shows
# "sendmsg: no route to host" against every 2606:4700 edge address, and while it
# still reported four ready connections, cache-busted public requests answered
# 502 and the connector recorded no request for them at all - the edge was
# holding connections it could not use. Pinning IPv4 keeps every registration on
# a path that works.
exec "$BIN" tunnel --no-autoupdate --edge-ip-version 4 --metrics $METRICS run
RUNNER_EOF
/bin/chmod 0700 "$RUNNER"

owner=$(/usr/bin/id -un)
tmp_plist=$(/usr/bin/mktemp /tmp/cloudflared-plist.XXXXXX)
/bin/cat > "$tmp_plist" <<PLIST_EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key><string>$LABEL</string>
    <key>UserName</key><string>$owner</string>
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
/usr/bin/plutil -lint "$tmp_plist" >/dev/null
/usr/bin/sudo -n /usr/bin/install -o root -g wheel -m 0644 "$tmp_plist" "$PLIST"
/bin/rm -f "$tmp_plist"

# Restart in place when the unit is already loaded. `bootout` followed
# immediately by `bootstrap` raced a still-terminating job and answered
# `5: Input/output error`, which left the connector unloaded and the public
# hostname serving 502 — an unload that fails halfway leaves nothing running.
# `kickstart -k` restarts a loaded job with no window in which it does not
# exist, so it is the first choice and bootstrap is only for a cold start.
if /usr/bin/sudo -n /bin/launchctl print "system/$LABEL" >/dev/null 2>&1; then
    /usr/bin/sudo -n /bin/launchctl kickstart -k "system/$LABEL" >/dev/null
else
    bootstrapped=no
    for _ in 1 2 3 4 5 6; do
        if err=$(/usr/bin/sudo -n /bin/launchctl bootstrap system "$PLIST" 2>&1); then
            bootstrapped=yes
            break
        fi
        /bin/sleep 5
    done
    [ "$bootstrapped" = yes ] || {
        printf 'bootstrap system -> %s\n' "$err" >&2
        exit 1
    }
fi

# Readiness is an attached connector, not a created process.
attached=no
for _ in 1 2 3 4 5 6 7 8 9 10 11 12; do
    /bin/sleep 5
    if /usr/bin/curl -fsS --max-time 5 "http://$METRICS/ready" >/dev/null 2>&1; then
        attached=yes
        break
    fi
done
printf 'cloudflared=%s unit=%s connector_ready=%s\n' "$BIN" "$PLIST" "$attached"
/usr/bin/curl -fsS --max-time 5 "http://$METRICS/ready" 2>/dev/null || true
printf '\n'
/usr/bin/tail -6 "$LOGS/cloudflared.log" 2>/dev/null || true
[ "$attached" = yes ]
