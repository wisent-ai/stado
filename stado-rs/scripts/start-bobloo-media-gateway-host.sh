#!/bin/sh
# Serve `bobloo.com` from this host: product media from Azure, everything else
# from the local Wisent API.
#
# The tunnel's remote ingress sends `bobloo.com` to `http://localhost:3000`, and
# nothing has listened there since the GCP estate went away, which is the real
# reason the host answers 502. The connector cannot rewrite paths and we hold no
# Cloudflare API token, so the mapping from the public path to the Azure
# container lives here, at the origin:
#
#   /images/*   and /profiles/*  ->  wisentprodstado / media-public
#   everything else              ->  127.0.0.1:8000 (Wisent API)
#
# This is what lets the product database name a Wisent host instead of a storage
# provider: the container, the account and even the cloud can change here
# without touching a single row.
#
# Additive: one new unit on a free port. Reverse it with
#   sudo launchctl bootout system/com.wisent.bobloo-gateway
#   sudo rm /Library/LaunchDaemons/com.wisent.bobloo-gateway.plist
set -eu

LABEL=com.wisent.bobloo-gateway
PORT=3000
ACCOUNT=wisentprodstado
CONTAINER=media-public
API=127.0.0.1:8000
PLIST="/Library/LaunchDaemons/$LABEL.plist"
CONF="$HOME/.stado/bobloo-gateway.Caddyfile"
LOGS="$HOME/.stado/logs"
BREW=/opt/homebrew/bin/brew

BIN=""
for candidate in /opt/homebrew/bin/caddy /usr/local/bin/caddy; do
    [ -x "$candidate" ] && BIN="$candidate" && break
done
if [ -z "$BIN" ]; then
    [ -x "$BREW" ] || { printf '%s\n' "no caddy and no brew to install it" >&2; exit 1; }
    "$BREW" install caddy >/dev/null 2>&1 || true
    for candidate in /opt/homebrew/bin/caddy /usr/local/bin/caddy; do
        [ -x "$candidate" ] && BIN="$candidate" && break
    done
fi
[ -n "$BIN" ] || { printf '%s\n' "caddy install failed" >&2; exit 1; }

/bin/mkdir -p "$LOGS"
/bin/cat > "$CONF" <<CONF_EOF
{
	admin off
	auto_https off
}

:$PORT {
	# The connector reaches this origin over loopback, so nothing else should.
	# A bare `:$PORT` listened on every interface, publishing the unauthenticated
	# origin to the LAN and the tailnet. Both loopback families are required:
	# the connector dials `http://localhost:3000`, which resolves to `::1` first,
	# so an IPv4-only bind turned every public request into a 502.
	bind 127.0.0.1 ::1
	@media path /images/* /profiles/*
	handle @media {
		rewrite * /$CONTAINER{uri}
		reverse_proxy https://$ACCOUNT.blob.core.windows.net {
			header_up Host $ACCOUNT.blob.core.windows.net
			header_down Cache-Control "public, max-age=86400"
		}
	}
	handle {
		reverse_proxy $API
	}
}
CONF_EOF
"$BIN" validate --config "$CONF" --adapter caddyfile >/dev/null 2>&1 \
    || { "$BIN" validate --config "$CONF" --adapter caddyfile; exit 1; }

owner=$(/usr/bin/id -un)
tmp_plist=$(/usr/bin/mktemp /tmp/bobloo-gateway.XXXXXX)
/bin/cat > "$tmp_plist" <<PLIST_EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key><string>$LABEL</string>
    <key>UserName</key><string>$owner</string>
    <key>ProgramArguments</key>
    <array>
        <string>$BIN</string>
        <string>run</string>
        <string>--config</string>
        <string>$CONF</string>
        <string>--adapter</string>
        <string>caddyfile</string>
    </array>
    <key>RunAtLoad</key><true/>
    <key>KeepAlive</key><true/>
    <key>ProcessType</key><string>Background</string>
    <key>StandardOutPath</key><string>$LOGS/bobloo-gateway.log</string>
    <key>StandardErrorPath</key><string>$LOGS/bobloo-gateway.log</string>
</dict>
</plist>
PLIST_EOF
/usr/bin/plutil -lint "$tmp_plist" >/dev/null
/usr/bin/sudo -n /usr/bin/install -o root -g wheel -m 0644 "$tmp_plist" "$PLIST"
/bin/rm -f "$tmp_plist"

/usr/bin/sudo -n /bin/launchctl bootout "system/$LABEL" >/dev/null 2>&1 || true
err=$(/usr/bin/sudo -n /bin/launchctl bootstrap system "$PLIST" 2>&1) || {
    printf 'bootstrap system -> %s\n' "$err" >&2
    exit 1
}
/usr/bin/sudo -n /bin/launchctl kickstart -k "system/$LABEL" >/dev/null 2>&1 || true

media=0
api=0
for _ in 1 2 3 4 5 6 7 8 9 10; do
    /bin/sleep 3
    media=$(/usr/bin/curl -s -o /dev/null -w '%{http_code}' --max-time 8 \
        "http://127.0.0.1:$PORT/images/characters/8808.webp" || true)
    [ "$media" = 200 ] && break
done
api=$(/usr/bin/curl -s -o /dev/null -w '%{http_code}' --max-time 8 "http://127.0.0.1:$PORT/health" || true)
printf 'caddy=%s local_media_status=%s local_api_status=%s\n' "$BIN" "$media" "$api"
/usr/bin/tail -4 "$LOGS/bobloo-gateway.log" 2>/dev/null || true
[ "$media" = 200 ]
