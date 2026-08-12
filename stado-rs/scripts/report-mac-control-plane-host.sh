#!/bin/sh
set -eu

label=com.wisent.compute.coordinator.charless-control-plane
service="system/$label"
log="$HOME/.stado/logs/$label.log"

printf '%s\n' '=== service ==='
/bin/launchctl print "$service" | /usr/bin/sed -n '1,100p'
printf '%s\n' '=== stado processes ==='
/bin/ps axww -o pid= -o ppid= -o etime= -o command= \
  | /usr/bin/awk '/[s]tado/ {print}'
printf '%s\n' '=== stado launch labels ==='
for pid in $(/bin/ps axww -o pid= -o command= | /usr/bin/awk '$2 ~ /\/stado$/ {print $1}'); do
  /bin/launchctl list | /usr/bin/awk -v pid="$pid" '$1 == pid {print}'
done
printf '%s\n' '=== object api service ==='
/bin/launchctl print system/com.wisent.always-on.stado-object-api \
  | /usr/bin/sed -n '1,100p'
printf '%s\n' '=== skarbiec weles service ==='
/bin/launchctl print system/com.wisent.always-on.skarbiec-weles \
  | /usr/bin/sed -n '1,120p'
printf '%s\n' '=== credential listeners ==='
/usr/sbin/lsof -nP -iTCP:8895 -iTCP:19095 -sTCP:LISTEN
printf '%s\n' '=== skarbiec weles log ==='
skarbiec_weles_log="$HOME/.stado/logs/com.wisent.always-on.skarbiec-weles.log"
if [ -f "$skarbiec_weles_log" ]; then
  /usr/bin/tail -n 80 "$skarbiec_weles_log"
else
  printf '%s\n' "missing: $skarbiec_weles_log"
fi
printf '%s\n' '=== recent log ==='
if [ -f "$log" ]; then
  /usr/bin/tail -n 120 "$log"
else
  printf '%s\n' "missing: $log"
fi
