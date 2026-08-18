#!/bin/sh
# Can this host's OWN agent grant read the Vast key, or only the control plane's?
#
# `stado agent` reads `stado-vast/api_key` through the control-plane channel
# (consumer `stado-control-plane`, token file `~/.stado/control-plane-skarbiec-token`),
# which a worker host does not hold, so its renter gate is permanently blind. The
# host does hold its own grant -- consumer `stado-local-agent`, bearer installed --
# and whether that grant carries the field decides whether routing the read
# through it is a code change or a vault change.
#
# Read-only, and it prints HTTP status codes only: never a token, never a value.
# The bearer is passed to curl through a file, not on the command line, so it
# never appears in this host's process table.
set -eu

env_file=/root/.stado/files/stado-agent-grant.env
[ -r "$env_file" ] || { printf 'ERROR\t%s unreadable\n' "$env_file" >&2; exit 1; }
# shellcheck disable=SC1090
. "$env_file"

url=${WC_AGENT_SKARBIEC_URL:-}
consumer=${WC_AGENT_SKARBIEC_CONSUMER:-}
token_file=${WC_AGENT_SKARBIEC_TOKEN_FILE:-}
printf 'CHANNEL\t%s consumer=%s\n' "$url" "$consumer"
printf 'BEARER\t'
if [ -r "$token_file" ]; then printf 'present (%s bytes)\n' "$(stat -c %s "$token_file")"; else printf 'absent at %s\n' "$token_file"; exit 1; fi

probe() {
  item=$1
  field=$2
  status=$(curl -sS -o /dev/null -w '%{http_code}' \
    --max-time 20 \
    -H "X-Consumer: $consumer" \
    -H "@/dev/stdin" \
    -H 'Content-Type: application/json' \
    -X POST "$url/v1/items/read" \
    --data "{\"id\":\"$item\",\"field\":\"$field\"}" <<EOF || printf '000'
Authorization: Bearer $(cat "$token_file")
EOF
)
  printf 'READ\t%s#%s\tHTTP %s\n' "$item" "$field" "$status"
}

# A field the grant is known to carry, as the control: if this one fails too, the
# answer is about the channel, not about the missing capability.
probe stado-huggingface token
probe stado-vast api_key
