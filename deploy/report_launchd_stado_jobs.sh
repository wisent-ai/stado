#!/usr/bin/env bash
# Report every loaded launchd job whose program is Stado, in both domains.
#
# A `stado release agent` has run since 2026-08-14 with ppid 1, and the label whose
# plist mentions it is loaded in neither `gui/<uid>` nor `system`. Either the label
# differs from the file name or the job was unloaded and the process was adopted by
# launchd -- an unsupervised reconciler writing release state every fifteen seconds
# from a four-day-old binary image. Which of those it is decides whether it can be
# restarted or has to be stopped.
#
# Written without mid-pipeline `head`: under `pipefail` that closes the pipe, the
# upstream takes SIGPIPE, and two earlier versions of this probe died one line after
# their first heading with no diagnosis at all.
#
# Read-only.
set -euo pipefail

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

printf 'host %s\n' "$(hostname -s 2>/dev/null || hostname)"
printf 'uid %s\n' "$(id -u)"

/bin/launchctl list > "$work/list" 2>/dev/null || true
/usr/bin/grep -i 'stado\|wisent' "$work/list" > "$work/labels" 2>/dev/null || true
printf 'matching_labels %s\n' "$(/usr/bin/wc -l < "$work/labels" | /usr/bin/tr -d ' ')"

while IFS= read -r line; do
  label=$(printf '%s\n' "$line" | /usr/bin/awk '{print $3}')
  [ -n "$label" ] || continue
  for domain in "gui/$(id -u)" system; do
    if /bin/launchctl print "$domain/$label" > "$work/print" 2>/dev/null; then
      state=$(/usr/bin/grep -m1 'state = ' "$work/print" | /usr/bin/sed 's/^[[:space:]]*//' || true)
      if /usr/bin/grep -q 'release' "$work/print"; then
        printf '  %s in %s %s (mentions release)\n' "$label" "$domain" "$state"
      else
        printf '  %s in %s %s\n' "$label" "$domain" "$state"
      fi
    fi
  done
done < "$work/labels"

printf -- '--- agent process ---\n'
/bin/ps -eo pid=,ppid=,lstart=,command= > "$work/ps" 2>/dev/null || true
/usr/bin/grep 'stado release agent' "$work/ps" | /usr/bin/cut -c1-150 || printf '  none\n'
