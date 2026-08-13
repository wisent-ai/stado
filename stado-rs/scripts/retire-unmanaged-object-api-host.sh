#!/bin/sh
# Stop the unmanaged object-API process so the managed unit can bind its port.
#
# Matches only a Stado dashboard listening on the control-plane port, never an
# agent, a coordinator or any other Stado process on this host.
#
# A process launchd supervises must be retired through launchctl, not killed
# underneath it. Parentage cannot tell the two apart -- an orphaned ad-hoc
# process is reparented to pid 1 exactly like a launchd job -- so the check
# asks launchd which pids it owns.
set -eu
port=8765

listeners=$(
  /usr/sbin/lsof -nP -iTCP:"$port" -sTCP:LISTEN -Fpc 2>/dev/null \
    | /usr/bin/awk '/^p/ {pid=substr($0,2)} /^cstado$/ {print pid}'
)
[ -n "$listeners" ] || {
  printf '%s\n' '{"stopped":[],"detail":"no stado listener on the control-plane port"}'
  exit 0
}

managed=$(/bin/launchctl list | /usr/bin/awk '$1 ~ /^[0-9]+$/ {print $1}')

stopped=""
skipped=""
for pid in $listeners; do
  if printf '%s\n' "$managed" | /usr/bin/grep -qx "$pid"; then
    [ -z "$skipped" ] || skipped="$skipped,"
    skipped="$skipped$pid"
    continue
  fi
  /bin/kill -TERM "$pid" 2>/dev/null || continue
  [ -z "$stopped" ] || stopped="$stopped,"
  stopped="$stopped$pid"
done
printf '{"stopped":[%s],"left_to_launchd":[%s]}\n' "$stopped" "$skipped"
