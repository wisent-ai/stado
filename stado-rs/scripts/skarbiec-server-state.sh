#!/bin/sh
# Is the running Skarbiec older than the vault it is supposed to serve?
#
# `skarbiec token-verify` reads the vault file and answers allowed=true while
# the server answers 403 for the same read. That gap is what this prints: the
# process actually holding the configured port, when it started, and when the
# vault file it serves was last written. A server started before the last mint
# is serving a grant table that no longer exists on disk.
set -eu

config=${STADO_CONFIG:-$HOME/.config/stado/config.json}
vault=${SKARBIEC_VAULT_FILE:-$HOME/.stado/skarbiec.vault.json}

url=$(/usr/bin/jq -r '.secrets.skarbiec.url' "$config")
port=${url##*:}

printf 'configured url : %s\n' "$url"
printf 'vault written  : %s\n' "$(/usr/bin/stat -f '%Sm' -t '%Y-%m-%dT%H:%M:%S' "$vault")"

pids=$(/usr/sbin/lsof -nP -iTCP:"$port" -sTCP:LISTEN -t || true)
if [ -z "$pids" ]; then
    printf 'listener       : none on port %s\n' "$port"
else
    for pid in $pids; do
        printf 'listener pid   : %s started %s\n' "$pid" \
            "$(/bin/ps -o lstart= -p "$pid" | /usr/bin/sed 's/^ *//')"
        /bin/ps -o command= -p "$pid" | /usr/bin/cut -c-"$(printf %s "----------------------------------------------------------------------------------------------------" | /usr/bin/wc -c)"
    done
fi

printf 'launchd unit   : %s\n' "$(/bin/launchctl list com.wisent.skarbiec | /usr/bin/grep -w PID || printf 'not loaded')"
