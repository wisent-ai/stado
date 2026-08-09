#!/bin/sh
# Declare the canonical Skarbiec endpoint for the credential subsystem.
#
# `credential status|adopt|rotate` read one owner-owned forward file and refuse
# outright when it is missing, which is what stopped the Microsoft rotation on
# the host that holds the vault: the service was listening the whole time and
# nothing named it. On the host that IS the canonical Skarbiec the hop is
# loopback, so the file names the port the running service already holds -
# discovered, not hardcoded, so a moved port cannot leave a stale declaration.
#
# Idempotent: an existing file is left alone and reported.
set -eu

forward_dir=$HOME/.stado/forwards
forward=$forward_dir/skarbiec.local
label=${SKARBIEC_LABEL:-com.wisent.always-on.skarbiec}

if [ -f "$forward" ]; then
    printf 'already declared: %s\n' "$(/bin/cat "$forward")"
else
    pid=$(/bin/launchctl print "system/$label" 2>/dev/null | /usr/bin/awk '/^\tpid = /{print $3}')
    if [ -z "${pid:-}" ]; then
        pid=$(/usr/bin/pgrep -x skarbiec | /usr/bin/head -n "$(printf '%s' a | /usr/bin/wc -c | /usr/bin/tr -d ' ')")
    fi
    if [ -z "${pid:-}" ]; then
        printf 'no running skarbiec to point at\n'
        exit
    fi
    port=$(/usr/sbin/lsof -nP -iTCP -sTCP:LISTEN -a -p "$pid" |
        /usr/bin/awk '/LISTEN/{split($9,a,":"); print a[length(a)]; exit}')
    if [ -z "${port:-}" ]; then
        printf 'skarbiec pid %s holds no listening port\n' "$pid"
        exit
    fi
    /bin/mkdir -p "$forward_dir"
    /bin/chmod u=rwx,go= "$forward_dir"
    printf 'http://127.0.0.1:%s\n' "$port" > "$forward"
    /bin/chmod u=rw,go= "$forward"
    printf 'declared: %s\n' "$(/bin/cat "$forward")"
fi

/usr/bin/curl -sS -o /dev/null -w 'endpoint answers -> %{http_code}\n' --max-time 5 \
    "$(/bin/cat "$forward")/v1/items/list" || printf 'endpoint answers -> unreachable\n'
