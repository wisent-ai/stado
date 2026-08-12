#!/bin/sh
set -eu

label=com.wisent.compute.coordinator.charless-control-plane
service="system/$label"
log="$HOME/.stado/logs/$label.log"

printf '%s\n' '=== service ==='
/bin/launchctl print "$service" | /usr/bin/sed -n '1,100p'
printf '%s\n' '=== recent log ==='
if [ -f "$log" ]; then
  /usr/bin/tail -n 120 "$log"
else
  printf '%s\n' "missing: $log"
fi
