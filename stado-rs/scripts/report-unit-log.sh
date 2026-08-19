#!/bin/sh
# The tail of one managed unit's own log, read from the paths its unit file
# declares.
#
# Why this exists: when a unit crash-loops, the only thing that says why is the
# log it writes, and until now nothing in Stado could read it. `host health`
# reports a unit as `failed` and carries an empty `last_log`; `service status`
# reports the state; `host exec` is a read-only allowlist that cannot cat a
# file. So the operator's fastest route to the sentence that names the fault was
# an ssh session — the one thing the fleet forbids. A brama restart that
# answered on one poll and was gone on the next cost half an hour of guessing
# for want of these twenty lines.
#
# The caller prepends `unit` and `lines` as shell-quoted bindings. Reports the
# declared paths, then the tail of each, prefixed so two files never blur into
# one. Read-only: nothing is written, nothing is installed.
set -eu
PATH="/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin"
export PATH

plist=""
for candidate in \
  "/Library/LaunchDaemons/$unit.plist" \
  "$HOME/Library/LaunchAgents/$unit.plist" \
  "/Library/LaunchAgents/$unit.plist"; do
  if [ -f "$candidate" ]; then
    plist="$candidate"
    break
  fi
done
if [ -z "$plist" ]; then
  printf 'no unit file for %s in the daemon or agent directories\n' "$unit" > /dev/stderr
  exit 1
fi
printf 'STADO_UNITLOG\tplist\t%s\n' "$plist"

# One reader for both keys: a unit that sends stdout and stderr to the same file
# must not be tailed twice, and a unit that separates them must not have half of
# its account silently dropped.
paths=$(/usr/libexec/PlistBuddy -c 'Print :StandardOutPath' "$plist" 2>/dev/null || true)
errs=$(/usr/libexec/PlistBuddy -c 'Print :StandardErrorPath' "$plist" 2>/dev/null || true)
if [ -n "$errs" ] && [ "$errs" != "$paths" ]; then
  paths="$paths
$errs"
fi
if [ -z "$paths" ]; then
  printf 'STADO_UNITLOG\tdeclared\tnone\n'
  printf '%s declares no log path\n' "$unit" > /dev/stderr
  exit 1
fi

printf '%s\n' "$paths" | while IFS= read -r log; do
  [ -n "$log" ] || continue
  if [ -f "$log" ]; then
    printf 'STADO_UNITLOG\tfile\t%s\n' "$log"
    printf '=== %s (last %s lines)\n' "$log" "$lines"
    tail -n "$lines" -- "$log" 2>/dev/null || printf '    unreadable\n'
  else
    printf 'STADO_UNITLOG\tabsent\t%s\n' "$log"
    printf '=== %s (absent)\n' "$log"
  fi
done
