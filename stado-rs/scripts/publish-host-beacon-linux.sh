#!/bin/sh
# Collect this Linux host's state and publish it as a Stado health beacon.
#
# The macOS fleet has had `host_health_beacon_macos.sh` for months; Linux had
# nothing, so `stado host ping` called a machine that was serving releases
# "down" -- the loudest possible way to be wrong about a healthy host. The
# bearer is read from an owner-only file because Skarbiec binds to loopback on
# the control plane and no authenticated broker path reaches this host.
set -eu
umask 077

STADO_BIN="${STADO_BIN:-$HOME/.stado/bin/stado}"
UNITS="${WC_HEALTH_UNITS:-wisent-agent.service image-video-router-release.service}"

reported_at=$(/bin/date -u +%Y-%m-%dT%H:%M:%SZ)
disk_line=$(/bin/df -h / 2>/dev/null | /usr/bin/awk 'NR==2 {print}' || true)

units_json=""
for unit in $UNITS; do
  if /bin/systemctl is-active --quiet "$unit" 2>/dev/null; then
    state=active
  elif /bin/systemctl is-enabled --quiet "$unit" 2>/dev/null; then
    state=inactive
  else
    state=missing
  fi
  [ -z "$units_json" ] || units_json="$units_json,"
  units_json="$units_json\"$unit\":{\"state\":\"$state\"}"
done

host_slug=$(/bin/hostname -s | /usr/bin/tr '[:upper:]' '[:lower:]')
payload_file=$(/bin/mktemp)
trap '/bin/rm -f "$payload_file"' EXIT HUP INT TERM
printf '{"host":"%s","reported_at":"%s","disk":"%s","units":{%s}}\n' \
  "$host_slug" "$reported_at" "$disk_line" "$units_json" > "$payload_file"

exec "$STADO_BIN" host publish-beacon "$payload_file"
