#!/bin/sh
# Find every place that defines the Weles agent model, and what the running
# process actually inherited.
#
# Two env files and two unit files were corrected and both units restarted, yet
# the browser task still refuses with "WELES_AGENT_MODEL must be the exact
# supported Brama alias weles/agent/primary". So the value it validates comes
# from somewhere else. Print names and values for this one variable only.
set -u

VAR=WELES_AGENT_MODEL

printf '== definitions on disk ==\n'
for path in \
    "$HOME/.weles/secrets.env" \
    "$HOME/.config/weles/worker.env" \
    "$HOME/weles/var/worker.env" \
    "$HOME/weles/.env" \
    "$HOME/weles/.env.local" \
    "$HOME/.zshrc" \
    "$HOME/.zprofile" \
    "$HOME/.profile" \
    /Library/LaunchDaemons/com.wisent.always-on.weles.plist \
    /Library/LaunchDaemons/com.wisent.always-on.weles-api.plist
do
    [ -f "$path" ] || continue
    line=$(/usr/bin/grep -m1 "$VAR" "$path" 2>/dev/null | /usr/bin/cut -c1-120)
    [ -n "$line" ] && printf '%s -> %s\n' "$path" "$line"
done

printf '== weles repo defaults ==\n'
if [ -d "$HOME/weles" ]; then
    /usr/bin/grep -rl "$VAR" "$HOME/weles" --include='*.mjs' --include='*.js' --include='*.json' \
        --include='*.ts' --include='*.sh' 2>/dev/null | /usr/bin/head -8
fi

printf '== running process environment ==\n'
for label in com.wisent.always-on.weles-api com.wisent.always-on.weles; do
    pid=$(/usr/bin/sudo -n /bin/launchctl print "system/$label" 2>/dev/null \
        | /usr/bin/awk '$1=="pid"{print $3; exit}')
    [ -n "$pid" ] || continue
    value=$(/usr/bin/sudo -n /bin/ps -Eww -o command -p "$pid" 2>/dev/null \
        | /usr/bin/tr ' ' '\n' | /usr/bin/grep "^$VAR=" | /usr/bin/head -1)
    printf '%s pid=%s %s\n' "$label" "$pid" "${value:-$VAR=not-in-process-env}"
done

printf '== brama aliases advertised ==\n'
/usr/bin/curl -s --max-time 8 http://127.0.0.1:8080/v1/models 2>/dev/null \
    | /usr/bin/tr ',' '\n' | /usr/bin/grep -i 'weles\|agent' | /usr/bin/head -8
