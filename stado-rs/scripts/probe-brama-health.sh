#!/bin/sh
# Does this host's Brama answer its own port, and how long has it been up?
#
# From another machine the port refused while `lsof` showed it bound, which is
# what a KeepAlive restart loop looks like from outside. Asking the host itself
# separates "not serving" from "not reachable". Status codes only.
set -eu

port=${BRAMA_PORT:-$(/usr/bin/sed -n 's/^PORT=//p' "$HOME/.config/brama/service.env")}
printf 'port %s\n' "$port"

/usr/bin/pgrep -f 'services/brama/.*/bin/brama' |
    while read -r pid; do
        /bin/ps -p "$pid" -o pid=,etime=,comm=
    done

/usr/bin/curl -sS -o /dev/null -w 'loopback -> %{http_code}\n' --max-time "$(printf '%s' 'aaaaa' | /usr/bin/wc -c | /usr/bin/tr -d ' ')" \
    "http://127.0.0.1:$port/v1/models" || printf 'loopback -> unreachable\n'
