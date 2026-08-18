#!/bin/sh
# Set the required Brama alias wherever the Weles API unit actually reads it.
#
# Repairing `~/.weles/secrets.env` was not enough: the process that serves
# `/run` is a different unit from the worker, and a launchd unit's own
# `EnvironmentVariables` override anything an env file says. So report every
# place the variable is set, fix the unit files that carry a wrong value, and
# restart the API in place.
set -eu

REQUIRED=weles/agent/primary
VAR=WELES_AGENT_MODEL
PB=/usr/libexec/PlistBuddy

for label in com.wisent.always-on.weles-api com.wisent.always-on.weles com.wisent.weles-echo-api; do
    for plist in "/Library/LaunchDaemons/$label.plist" "$HOME/Library/LaunchAgents/$label.plist"; do
        [ -f "$plist" ] || continue
        current=$("$PB" -c "Print :EnvironmentVariables:$VAR" "$plist" 2>/dev/null || true)
        printf 'plist=%s %s=%s\n' "$plist" "$VAR" "${current:-absent}"
        [ -n "$current" ] || continue
        [ "$current" = "$REQUIRED" ] && continue
        stamp=$(/bin/date -u +%Y%m%dT%H%M%SZ)
        /usr/bin/sudo -n /bin/cp -p "$plist" "$plist.bak-$stamp"
        /usr/bin/sudo -n "$PB" -c "Set :EnvironmentVariables:$VAR $REQUIRED" "$plist"
        /usr/bin/sudo -n /usr/bin/plutil -lint "$plist" >/dev/null
        printf 'plist_updated=%s backup=%s\n' "$plist" "$plist.bak-$stamp"
    done
done

# Env files the units may source, worker and API alike.
for env_file in "$HOME/.weles/secrets.env" "$HOME/weles/var/worker.env" "$HOME/.config/weles/worker.env"; do
    [ -f "$env_file" ] || continue
    current=$(/usr/bin/grep -m1 "^$VAR=" "$env_file" | /usr/bin/cut -d= -f2- || true)
    printf 'env=%s %s=%s\n' "$env_file" "$VAR" "${current:-absent}"
    [ "$current" = "$REQUIRED" ] && continue
    stamp=$(/bin/date -u +%Y%m%dT%H%M%SZ)
    /bin/cp -p "$env_file" "$env_file.bak-$stamp"
    tmp=$(/usr/bin/mktemp "${env_file%/*}/.env.XXXXXX")
    /usr/bin/grep -v "^$VAR=" "$env_file" > "$tmp" || true
    printf '%s=%s\n' "$VAR" "$REQUIRED" >> "$tmp"
    /bin/chmod 600 "$tmp"
    /bin/mv "$tmp" "$env_file"
    printf 'env_updated=%s\n' "$env_file"
done

for label in com.wisent.always-on.weles-api com.wisent.always-on.weles; do
    for domain in "system/$label" "user/$(/usr/bin/id -u)/$label"; do
        if /usr/bin/sudo -n /bin/launchctl print "$domain" >/dev/null 2>&1; then
            /usr/bin/sudo -n /bin/launchctl kickstart -k "$domain" >/dev/null 2>&1 \
                && printf 'restarted=%s\n' "$domain"
            break
        fi
    done
done

/bin/sleep 15
printf 'weles_api=%s\n' "$(/usr/bin/curl -s -o /dev/null -w '%{http_code}' --max-time 8 http://127.0.0.1:8788/healthz || true)"
