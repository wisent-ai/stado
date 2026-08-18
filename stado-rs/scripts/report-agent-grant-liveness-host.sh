#!/bin/sh
# Does this host's agent grant still authenticate, and for which fields?
#
# Portable across the fleet's macOS and Linux hosts: the agent grant lives in an
# env file under $HOME/.stado, and the answer worth having is one HTTP status per
# probed field. On 2026-08-17 this separated two things that look identical from
# the outside -- a bearer that works but lacks a capability (403 on the field,
# 200 on another) from a bearer the vault no longer recognises (401 on both).
#
# Read-only. Statuses only: never a token, never a value. The bearer reaches curl
# through stdin, so it never appears in this host's process table.
set -eu

env_file=""
for candidate in "$HOME/.stado/files/stado-agent-grant.env" "$HOME/.stado/stado-agent-grant.env" "$HOME/.stado/stado-agent.env"; do
  if [ -r "$candidate" ]; then env_file=$candidate; break; fi
done
[ -n "$env_file" ] || { printf 'ERROR\tno agent grant env file under %s/.stado\n' "$HOME" >&2; exit 1; }
printf 'ENV_FILE\t%s\n' "$env_file"
# shellcheck disable=SC1090
. "$env_file"

url=${WC_AGENT_SKARBIEC_URL:-}
consumer=${WC_AGENT_SKARBIEC_CONSUMER:-}
token_file=$(printf '%s' "${WC_AGENT_SKARBIEC_TOKEN_FILE:-}" | sed "s|^\$HOME|$HOME|")
printf 'CHANNEL\t%s consumer=%s\n' "$url" "$consumer"
printf 'BEARER\t'
if [ -r "$token_file" ]; then printf 'present at %s\n' "$token_file"; else printf 'absent at %s\n' "$token_file"; exit 1; fi

probe() {
  status=$(curl -sS -o /dev/null -w '%{http_code}' --max-time 20 \
    -H "X-Consumer: $consumer" \
    -H 'Content-Type: application/json' \
    -H "@/dev/stdin" \
    -X POST "$url/v1/items/read" \
    --data "{\"id\":\"$1\",\"field\":\"$2\"}" <<EOF || printf '000'
Authorization: Bearer $(cat "$token_file")
EOF
)
  printf 'READ\t%s#%s\tHTTP %s\n' "$1" "$2" "$status"
}

probe stado-huggingface token
probe stado-vast api_key
