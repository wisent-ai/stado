#!/bin/sh
# Show what this host's beacon unit runs, then run that command in the
# foreground so its own stdout and stderr are visible.
#
# The unit reports "Finished" even when the publish inside it fails, so the exit
# status of the systemd job says nothing about whether a beacon was delivered.
set -eu

PATH="$HOME/.cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"
export PATH

UNIT=wisent-host-health.service

printf '== unit definition ==\n'
systemctl cat "$UNIT" --no-pager 2>/dev/null | sed 's/^/  /'

command_line=$(systemctl show "$UNIT" --property=ExecStart --no-pager 2>/dev/null \
  | sed -n 's/.*argv\[\]=\([^;]*\);.*/\1/p')
printf '\n== resolved ExecStart ==\n  %s\n' "${command_line:-(none)}"

if [ -n "$command_line" ]; then
  printf '\n== running it in the foreground ==\n'
  # shellcheck disable=SC2086
  sh -c "$command_line" 2>&1 | tail -25 | sed 's/^/  /'
  printf '  exit: %s\n' "$?"
fi
