#!/bin/sh
# Read-only report of top-level /tmp usage on a managed host.
set -eu

printf 'filesystem:\n'
df -h /tmp
printf '\ntop-level usage (bytes):\n'
du -x -B1 --max-depth=1 /tmp 2>/dev/null | sort -nr
