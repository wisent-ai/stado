#!/bin/sh
# Install the periodic host-health beacon on a Linux registry host.
#
# The macOS hosts have had a launchd beacon for months; Linux had none, so
# `stado host ping` reported a machine that was serving releases as down, and a
# beacon published by hand went stale within the hour. A timer is what makes
# the signal mean "this host is alive now" instead of "someone ran a command
# once".
#
# The publisher reads its bearer from an owner-only file and trusts the tailnet
# CA from a file; neither value is placed in the unit, and neither is a secret
# in the environment.
set -eu

service=/etc/systemd/system/stado-host-beacon.service
timer=/etc/systemd/system/stado-host-beacon.timer
helper="$HOME/.stado/bin/publish-host-beacon"
token="$HOME/.stado/host-health-api-beacon-token"
# The anchor this host verifies the fleet store with. It is a copy: the authority
# itself is Skarbiec item `stado-tailnet-ca` in charless-mac-mini's vault, and
# `scripts/install-tailnet-anchor.py` is what replaces this file. Re-issue from
# that item rather than minting a new authority, which would re-anchor every host.
ca="$HOME/.stado/stado-tailnet-ca.crt"
# One endpoint for every beacon: the supervised service that owns the
# fleet store. The always-on host owns a different disk, so a beacon
# posted there is stored and never read.
endpoint=https://lukaszs-macbook-pro-4007-2.tail6443b3.ts.net

for required in "$helper" "$token" "$ca"; do
  [ -e "$required" ] || { printf '%s\n' "missing $required" >&2; exit 1; }
done

/bin/cat > "$service" <<UNIT
[Unit]
Description=Publish this host's Stado health beacon
After=network-online.target
Wants=network-online.target

[Service]
Type=oneshot
Environment=STADO_HOST_HEALTH_API_URL=$endpoint
Environment=STADO_HOST_HEALTH_API_TOKEN_FILE=$token
Environment=WC_STADO_STORAGE_CA_FILE=$ca
Environment=HOME=$HOME
ExecStart=$helper
UNIT

/bin/cat > "$timer" <<UNIT
[Unit]
Description=Publish this host's Stado health beacon every five minutes

[Timer]
OnBootSec=2min
OnUnitActiveSec=5min
AccuracySec=30s
Unit=stado-host-beacon.service

[Install]
WantedBy=timers.target
UNIT

/bin/systemctl daemon-reload
/bin/systemctl enable --now stado-host-beacon.timer >/dev/null 2>&1
/bin/systemctl start stado-host-beacon.service

printf '{"timer":"%s","state":"%s","last":"%s"}\n' \
  stado-host-beacon.timer \
  "$(/bin/systemctl is-active stado-host-beacon.timer 2>/dev/null || echo unknown)" \
  "$(/bin/systemctl show stado-host-beacon.service -p ExecMainStatus --value 2>/dev/null || echo unknown)"
