#!/bin/sh
# Report whether this host can run the bobloo Cloudflare Tunnel connector.
#
# `bobloo.com` answers 502: Cloudflare still holds the public hostname route,
# but no connector is attached, so the product API and every media URL served
# through that host are unreachable. Before changing anything on a live host,
# establish what is already here: the connector binary, an installer for it, an
# owner-only token, the declared service, and which local origin could serve.
#
# Read-only. It starts nothing, installs nothing and prints no secret value.
set -u

printf 'host=%s\n' "$(/bin/hostname -s)"

for candidate in \
    /opt/homebrew/bin/cloudflared \
    /usr/local/bin/cloudflared \
    "$HOME/.stado/bin/cloudflared" \
    "$HOME/.local/bin/cloudflared"
do
    if [ -x "$candidate" ]; then
        printf 'cloudflared=%s version=%s\n' "$candidate" \
            "$("$candidate" --version 2>/dev/null | /usr/bin/head -1)"
        found=yes
    fi
done
[ "${found:-no}" = yes ] || printf 'cloudflared=absent\n'

for installer in /opt/homebrew/bin/brew /usr/local/bin/brew; do
    [ -x "$installer" ] && printf 'brew=%s\n' "$installer"
done

token="$HOME/.stado/cloudflared-token"
if [ -f "$token" ]; then
    printf 'token_file=present bytes=%s mode=%s\n' \
        "$(/usr/bin/stat -f %z "$token")" "$(/usr/bin/stat -f %Lp "$token")"
else
    printf 'token_file=absent expected=%s\n' "$token"
fi

for plist in \
    "$HOME/Library/LaunchAgents/com.wisent.cloudflared.plist" \
    /Library/LaunchDaemons/com.wisent.cloudflared.plist
do
    [ -f "$plist" ] && printf 'service_plist=%s\n' "$plist"
done

printf 'listening_http_origins:\n'
/usr/sbin/lsof -nP -iTCP -sTCP:LISTEN 2>/dev/null \
    | /usr/bin/awk '{print $1, $9}' \
    | /usr/bin/grep -E '127\.0\.0\.1:(80|300[0-9]|400[0-9]|500[0-9]|8000|8001|8080|8081|8765|8787|8788|9000)$' \
    | /usr/bin/sort -u
