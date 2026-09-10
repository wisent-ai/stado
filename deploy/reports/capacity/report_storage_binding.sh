#!/usr/bin/env bash
# Report the object store this host is configured to read, beside the port its
# own dashboard actually serves.
#
# The resolver's snapshot runs `stado resolver snapshot` over ssh on the
# authority host, in a non-login shell, so it takes the store address from that
# host's configuration and nothing else. When the configuration names a port
# the dashboard no longer binds, the failure surfaces on the calling machine as
# an unreachable control plane and says nothing about which host disagreed.
#
# Read-only. The process pattern is bracketed so this script does not report
# its own pipeline as the service it is looking for.
set -u

config="$HOME/.config/stado/config.json"

echo "=== configured store ==="
if [ -r "$config" ]; then
  echo "file: $config"
  /usr/bin/sed -n '/"url"/p' "$config" | head -4
else
  echo "(no readable $config)"
fi

echo
echo "=== dashboard process ==="
/bin/ps -axo pid=,command= 2>/dev/null | /usr/bin/grep '[s]tado dashboard' | head -2

echo
echo "=== object api reachability from here ==="
for candidate in $(/bin/ps -axo command= 2>/dev/null | /usr/bin/grep '[s]tado dashboard' | /usr/bin/awk '{for (i=1;i<=NF;i++) if ($i=="--port") print $(i+1)}'); do
  code=$(/usr/bin/curl -s -m 5 -o /dev/null -w '%{http_code}' "http://127.0.0.1:${candidate}/" 2>/dev/null || echo "000")
  echo "port ${candidate}: HTTP ${code}"
done
