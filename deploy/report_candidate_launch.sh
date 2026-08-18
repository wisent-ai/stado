#!/usr/bin/env bash
# Report the newest candidate launch log for a product on this host.
#
# The release agent quarantined brama's digest with "candidate did not become ready
# before deadline", and the same reason is recorded for a digest from twelve days
# earlier: the candidate has never passed readiness on this host, so the release
# reaching the target is not the same as the release working there. The reason is
# in the launch log, which the agent keeps under its logs root.
#
# Read-only. Prints the tail of the newest log and the ports the policy uses, so a
# readiness probe pointed at the wrong port looks different from a process that
# exited.
set -euo pipefail

product="${RELEASE_PRODUCT:-brama}"
logs_root="${RELEASE_LOGS_ROOT:-$HOME/.stado/logs}"
printf 'host %s\n' "$(hostname -s 2>/dev/null || hostname)"
printf 'logs_root %s\n' "$logs_root"

if [ ! -d "$logs_root" ]; then
  printf 'logs_root absent\n'
  exit 0
fi

printf -- '--- candidate logs, newest first ---\n'
/usr/bin/find "$logs_root" -type f -name "*${product}*" -print0 2>/dev/null \
  | /usr/bin/xargs -0 /bin/ls -t 2>/dev/null | /usr/bin/head -5 | while IFS= read -r log; do
  printf '  %s  %s bytes  %s\n' "$log" \
    "$(/usr/bin/wc -c <"$log" | /usr/bin/tr -d ' ')" \
    "$(/usr/bin/stat -f '%Sm' -t '%Y-%m-%dT%H:%M:%SZ' "$log" 2>/dev/null || true)"
done

# A candidate's log is named for its version; `brama-always-on.err` is the running
# service's, and it is 382 MB, so tailing "the newest brama log" reads the wrong
# process and shows a perfectly healthy gateway while the candidate that failed
# sits in a 2 KB file next to it. Prefer a version-named log, and say which was
# chosen.
newest=$(/usr/bin/find "$logs_root" -type f -name "${product}-[0-9]*" -print0 2>/dev/null \
  | /usr/bin/xargs -0 /bin/ls -t 2>/dev/null | /usr/bin/head -1 || true)
if [ -z "$newest" ]; then
  printf 'no version-named %s candidate log; falling back to newest match\n' "$product"
  newest=$(/usr/bin/find "$logs_root" -type f -name "*${product}*" -print0 2>/dev/null \
    | /usr/bin/xargs -0 /bin/ls -t 2>/dev/null | /usr/bin/head -1 || true)
fi
if [ -z "$newest" ]; then
  printf 'no %s log found\n' "$product"
  exit 0
fi

printf -- '--- tail of %s ---\n' "$newest"
/usr/bin/tail -25 "$newest" | /usr/bin/cut -c1-200

# Which ports the candidate was told to use, so "nothing listening" can be told
# apart from "listening somewhere else".
printf -- '--- listeners ---\n'
/usr/sbin/lsof -nP -iTCP -sTCP:LISTEN 2>/dev/null \
  | /usr/bin/awk '$9 ~ /:(8080|18080|18081)$/ {print "  " $1, $2, $9}' | /usr/bin/head -6
