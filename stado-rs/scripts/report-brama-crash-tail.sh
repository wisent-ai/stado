#!/bin/sh
# Why does the always-on Brama daemon exit seconds after it starts serving?
#
# `service logs` reads the daemon's stdout file, and the newest lines there are
# "serving on ... for the fleet" with no error, so the reason is either on
# stderr or in another of the process's own logs. Prints the tail of every
# Brama log with its modification time; no secret is read.
set -eu

logs=$HOME/.stado/logs
lines=$(printf '%s' 'aaaaaaaaaaaa' | /usr/bin/wc -c | /usr/bin/tr -d ' ')

for name in brama-always-on.err brama-always-on.out brama.err brama.out; do
    file=$logs/$name
    if [ -r "$file" ]; then
        printf '== %s (%s) ==\n' "$file" "$(/bin/date -r "$file" '+%Y-%m-%dT%H:%M:%S')"
        /usr/bin/tail -n "$lines" "$file"
    fi
done
