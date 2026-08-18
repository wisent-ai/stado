#!/bin/sh
# Stop the Weles API launcher from overwriting the agent model with "best".
#
# Root cause of every failing browser job: `~/.stado/bin/weles-api-launcher`
# sources the three env files and then, further down, hardcodes
#   export WELES_AGENT_MODEL=best
# so nothing set in `~/.weles/secrets.env`, `~/.config/weles/worker.env` or the
# launchd unit could ever survive. Weles then rejects the run with
# "WELES_AGENT_MODEL must be the exact supported Brama alias weles/agent/primary",
# and Brama does serve that alias - it is a key in
# `~/.config/brama/inference-routes.json`.
#
# The line becomes a default instead of an override, so an operator or unit can
# still pin a different alias. The launcher itself is generated outside this
# repository (Weles), so the same change belongs there; this repairs the host.
set -eu

LAUNCHER="$HOME/.stado/bin/weles-api-launcher"
REQUIRED=weles/agent/primary
LABEL=com.wisent.always-on.weles-api

[ -f "$LAUNCHER" ] || { printf '%s\n' "missing $LAUNCHER" >&2; exit 1; }
before=$(/usr/bin/grep -n 'WELES_AGENT_MODEL=' "$LAUNCHER" | /usr/bin/head -3)
printf 'before:\n%s\n' "$before"

if /usr/bin/grep -q '^export WELES_AGENT_MODEL=best$' "$LAUNCHER"; then
    stamp=$(/bin/date -u +%Y%m%dT%H%M%SZ)
    /bin/cp -p "$LAUNCHER" "$LAUNCHER.bak-$stamp"
    tmp=$(/usr/bin/mktemp "$HOME/.stado/bin/.weles-api-launcher.XXXXXX")
    /usr/bin/sed 's|^export WELES_AGENT_MODEL=best$|export WELES_AGENT_MODEL="${WELES_AGENT_MODEL:-'"$REQUIRED"'}"|' \
        "$LAUNCHER" > "$tmp"
    /bin/chmod 0700 "$tmp"
    /bin/mv "$tmp" "$LAUNCHER"
    printf 'launcher_updated=yes backup=%s\n' "$LAUNCHER.bak-$stamp"
else
    printf 'launcher_updated=no reason=pattern-absent\n'
fi

printf 'after:\n%s\n' "$(/usr/bin/grep -n 'WELES_AGENT_MODEL=' "$LAUNCHER" | /usr/bin/head -3)"

/usr/bin/sudo -n /bin/launchctl kickstart -k "system/$LABEL" >/dev/null 2>&1 || true
/bin/sleep 15
printf 'weles_api=%s\n' "$(/usr/bin/curl -s -o /dev/null -w '%{http_code}' --max-time 8 http://127.0.0.1:8788/healthz || true)"
