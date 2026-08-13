#!/bin/sh
set -eu

[ "$(uname -s)" = Linux ] || {
  printf '%s\n' "Stado agent grant configuration requires systemd" >&2
  exit 1
}

environment_file="$HOME/.stado/files/stado-agent-grant.env"
[ -f "$environment_file" ] || {
  printf '%s\n' "missing $environment_file" >&2
  exit 1
}

for name in \
  WC_AGENT_SKARBIEC_URL \
  WC_AGENT_SKARBIEC_CONSUMER \
  WC_AGENT_SKARBIEC_TOKEN_FILE \
  WC_AGENT_SKARBIEC_ITEMS \
  WC_AGENT_SKARBIEC_SECRET_FIELDS
do
  /usr/bin/grep -q "^${name}=" "$environment_file" || {
    printf '%s\n' "missing $name in $environment_file" >&2
    exit 1
  }
done

unit=wisent-agent.service
dropin_directory="/etc/systemd/system/${unit}.d"
dropin="$dropin_directory/agent-skarbiec.conf"
temporary="${dropin}.tmp.$$"
/bin/mkdir -p "$dropin_directory"
printf '[Service]\nEnvironmentFile=%s\n' "$environment_file" >"$temporary"
/bin/chmod 0644 "$temporary"
/bin/mv "$temporary" "$dropin"
/bin/systemctl daemon-reload
/bin/systemctl restart "$unit"
/bin/systemctl is-active "$unit"
