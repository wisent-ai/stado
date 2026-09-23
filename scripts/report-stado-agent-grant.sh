#!/bin/sh
# Report the non-secret routing and allowlist fields consumed by the local agent.
set -eu

file="$HOME/.stado/files/stado-agent-grant.env"
[ -r "$file" ] || {
  printf 'missing=%s\n' "$file" >&2
  exit 1
}
for name in \
  WC_AGENT_SKARBIEC_URL \
  WC_AGENT_SKARBIEC_CONSUMER \
  WC_AGENT_SKARBIEC_TOKEN_FILE \
  WC_AGENT_SKARBIEC_ITEMS \
  WC_AGENT_SKARBIEC_SECRET_FIELDS
do
  value=$(sed -n "s/^${name}=//p" "$file")
  [ -n "$value" ] || {
    printf 'missing_field=%s\n' "$name" >&2
    exit 1
  }
  printf '%s=%s\n' "$name" "$value"
done
