#!/bin/sh
# Whether this host's resolver accepts the current service directory.
#
# `cli/resolver.rs::refresh` refuses a directory that changed while
# `service_directory.generation` stayed put, and a running resolver that has
# already cached the old content stays refused until the generation advances or
# the process restarts. From the outside both look the same as a dead adapter, so
# the log line is the only thing that separates them:
#
#   service directory changed without advancing generation N
#   service directory cache is stale (store generation <hash>)
#
# Read-only: unit state, bound adapter ports, and the resolver's own recent log.
set -eu

unit_linux=com.wisent.compute.service.stado-resolver.service
label_macos=com.wisent.stado-resolver

printf 'UNIT\n'
if command -v systemctl >/dev/null 2>&1; then
  printf '%s\t%s\n' "$unit_linux" "$(systemctl is-active "$unit_linux" 2>&1 || true)"
  printf 'RESTARTS\t%s\n' "$(systemctl show "$unit_linux" --property=NRestarts --value 2>&1 || true)"
else
  printf '%s\t%s\n' "$label_macos" "$(launchctl print "gui/$(id -u)/$label_macos" 2>/dev/null | awk '/state =/ { print $3; exit }' || printf 'unknown')"
  printf 'LAST_EXIT\t%s\n' "$(launchctl print "gui/$(id -u)/$label_macos" 2>/dev/null | awk '/last exit code/ { print $5; exit }' || printf 'unknown')"
fi

printf '\nADAPTERS_LISTENING\n'
if command -v ss >/dev/null 2>&1; then
  ss -ltnp 2>/dev/null | grep -E '176[0-9][0-9]|187[0-9][0-9]' || printf 'none in the resolver port range\n'
else
  lsof -nP -iTCP -sTCP:LISTEN 2>/dev/null | grep -E '176[0-9][0-9]|187[0-9][0-9]' || printf 'none in the resolver port range\n'
fi

printf '\nRESOLVER_LOG_TAIL\n'
if command -v journalctl >/dev/null 2>&1; then
  journalctl -u "$unit_linux" --no-pager -n 12 -o cat 2>&1 | tail -n 12 || true
else
  for log in "$HOME/.stado/logs/stado-resolver.err" "$HOME/.stado/logs/stado-resolver.out"; do
    [ -r "$log" ] || continue
    printf '%s\n' "$log"
    tail -n 8 "$log"
  done
fi
