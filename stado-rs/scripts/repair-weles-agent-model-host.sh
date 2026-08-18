#!/bin/sh
# Point the Weles browser agent at the Brama alias it is required to use.
#
# Every Weles browser job currently dies before it opens a page:
#   Jeden/Brama error after retries:
#   WELES_AGENT_MODEL must be the exact supported Brama alias weles/agent/primary
# So browser automation on the dedicated host has been unavailable, which is why
# a Cloudflare dashboard read looked impossible from here.
#
# Only this one variable is touched, and only when it is absent or different. The
# previous file is kept beside it with a `.bak-` timestamp suffix, and no other
# value is read, printed or changed.
set -eu

REQUIRED=weles/agent/primary
ENV_FILE=${WELES_ENV_FILE:-$HOME/.weles/secrets.env}
LABEL=com.wisent.always-on.weles

[ -f "$ENV_FILE" ] || { printf '%s\n' "missing $ENV_FILE" >&2; exit 1; }

current=$(/usr/bin/grep -m1 '^WELES_AGENT_MODEL=' "$ENV_FILE" | /usr/bin/cut -d= -f2- || true)
printf 'current=%s required=%s\n' "${current:-absent}" "$REQUIRED"

if [ "$current" = "$REQUIRED" ]; then
    printf 'env_change=none\n'
else
    stamp=$(/bin/date -u +%Y%m%dT%H%M%SZ)
    /bin/cp -p "$ENV_FILE" "$ENV_FILE.bak-$stamp"
    tmp=$(/usr/bin/mktemp "$HOME/.weles/.secrets.env.XXXXXX")
    /usr/bin/grep -v '^WELES_AGENT_MODEL=' "$ENV_FILE" > "$tmp"
    printf 'WELES_AGENT_MODEL=%s\n' "$REQUIRED" >> "$tmp"
    /bin/chmod 600 "$tmp"
    /bin/mv "$tmp" "$ENV_FILE"
    printf 'env_change=applied backup=%s\n' "$ENV_FILE.bak-$stamp"
fi

# Restart in place: the unit exists, so kickstart avoids any window in which the
# always-on worker is unloaded.
uid=$(/usr/bin/id -u)
restarted=no
for domain in "system/$LABEL" "user/$uid/$LABEL" "gui/$uid/$LABEL"; do
    if /usr/bin/sudo -n /bin/launchctl print "$domain" >/dev/null 2>&1; then
        /usr/bin/sudo -n /bin/launchctl kickstart -k "$domain" >/dev/null 2>&1 && restarted="$domain"
        break
    fi
    if /bin/launchctl print "$domain" >/dev/null 2>&1; then
        /bin/launchctl kickstart -k "$domain" >/dev/null 2>&1 && restarted="$domain"
        break
    fi
done
printf 'weles_restarted=%s\n' "$restarted"
/bin/sleep 15
printf 'weles_api=%s\n' "$(/usr/bin/curl -s -o /dev/null -w '%{http_code}' --max-time 8 http://127.0.0.1:8788/healthz || true)"
