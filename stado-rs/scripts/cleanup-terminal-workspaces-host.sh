#!/bin/sh
# Reclaim terminal Stado/model workspaces after dispatch has been paused.
set -eu

[ "$(uname -s)" = Linux ] || {
  printf '%s\n' "terminal workspace cleanup requires Linux" >&2
  exit 1
}

stado="$HOME/.stado/bin/stado"
[ -x "$stado" ]
environment=$(/bin/systemctl show wisent-agent.service --property=Environment --value)
queue_state=$(/usr/bin/env -S "$environment" "$stado" queue status)
printf '%s\n' "$queue_state" | /usr/bin/grep -q 'paused[[:space:]]*true' || {
  printf '%s\n' "refusing cleanup while Stado dispatch is active" >&2
  exit 1
}

/bin/df -h /tmp
removed=0
for path in \
  /tmp/wc-* \
  /tmp/oko-lifecycle-model-* \
  /tmp/echo-humanizer-* \
  /tmp/jeden-goal-model \
  /tmp/jeden-goal-qualified-* \
  /tmp/jeden-goal-artifact-verify \
  /tmp/stado-machine-source
 do
  [ -e "$path" ] || continue
  /bin/rm -rf -- "$path"
  removed=$((removed + 1))
 done
printf 'removed terminal workspace roots: %s\n' "$removed"
/bin/df -h /tmp
