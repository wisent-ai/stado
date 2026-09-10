#!/usr/bin/env bash
# Report why the stable release proxy did not start.
#
# The release agent binds `stable_bind` with a small proxy that fronts whichever
# candidate became active. Its failure is recorded as "stable release proxy failed
# to start", which names neither the bind nor the error, and the reason lands in a
# log file the agent opens for the child.
#
# Read-only: the proxy log tail, the declared bind, and who holds that port.
set -euo pipefail

product="${RELEASE_PRODUCT:-brama}"
logs_root="${RELEASE_LOGS_ROOT:-$HOME/.stado/logs}"
printf 'host %s\n' "$(hostname -s 2>/dev/null || hostname)"

for stream in err out; do
  log="$logs_root/$product-proxy.$stream"
  if [ -f "$log" ]; then
    printf -- '--- %s (%s bytes) ---\n' "$log" "$(/usr/bin/wc -c <"$log" | /usr/bin/tr -d ' ')"
    /usr/bin/tail -12 "$log" | /usr/bin/cut -c1-190
  else
    printf '%s absent\n' "$log"
  fi
done

# Whoever already owns the stable bind is the likeliest reason a second binder
# fails, and the legacy launchd service is the usual holder.
printf -- '--- port holders ---\n'
/usr/sbin/lsof -nP -iTCP -sTCP:LISTEN 2>/dev/null \
  | /usr/bin/awk '$9 ~ /:(8080|18080|18081)$/ {print "  " $1, "pid=" $2, $9}' | /usr/bin/head -8

printf -- '--- legacy service ---\n'
/bin/launchctl print system/com.wisent.always-on.brama 2>/dev/null \
  | /usr/bin/awk '/state|pid|program/ {print "  " $0}' | /usr/bin/head -6 \
  || printf '  legacy label not loaded in system domain\n'
