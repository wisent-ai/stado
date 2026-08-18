#!/usr/bin/env bash
# Report what supervises the resident release agent on this host.
#
# A `stado release agent --interval-seconds 15` process reconciles every fifteen
# seconds from the binary image it started with, so installing a new Stado does not
# change its behaviour and its verdicts keep overwriting the state file. It is not a
# registry-managed service, so the label that owns it has to be found before it can
# be restarted the sanctioned way rather than killed.
#
# Read-only: the process, its start time, and any launchd label whose program is
# Stado. Nothing is stopped or started here.
set -euo pipefail

printf 'host %s\n' "$(hostname -s 2>/dev/null || hostname)"
printf -- '--- agent process ---\n'
/bin/ps -eo pid=,ppid=,lstart=,command= 2>/dev/null \
  | /usr/bin/awk '/stado release agent/{print "  " $0}' | /usr/bin/cut -c1-160 | /usr/bin/head -3

printf -- '--- launchd labels mentioning stado ---\n'
/bin/launchctl list 2>/dev/null | /usr/bin/awk 'NR>1 && $3 ~ /stado|wisent/ {print "  " $1, $2, $3}' \
  | /usr/bin/head -12

printf -- '--- plists whose program is stado ---\n'
for dir in "$HOME/Library/LaunchAgents" /Library/LaunchAgents /Library/LaunchDaemons; do
  [ -d "$dir" ] || continue
  /usr/bin/grep -rl 'stado' "$dir" 2>/dev/null | /usr/bin/head -6 | while IFS= read -r plist; do
    label=$(/usr/bin/basename "$plist" .plist)
    if /usr/bin/grep -q 'release[[:space:]]*<\|release</string>\|<string>agent</string>' "$plist" 2>/dev/null; then
      printf '  %s (mentions release/agent)\n' "$label"
    else
      printf '  %s\n' "$label"
    fi
  done
done
