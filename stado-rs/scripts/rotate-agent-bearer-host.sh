#!/bin/sh
# Put the worker-agent grant back on a bearer its holders actually hold.
#
# `stado-local-agent` has two holders and the vault agreed with neither pair of
# them consistently: the RTX host's bearer authenticates (HTTP 200 on a field it
# holds), while the copy beside the vault -- which the mac mini's own agent reads
# through `agent.skarbiec.token_file` in `~/.config/stado/config.json` -- did not
# hash to what the vault recorded, so that host's credential reads were failing.
# Nothing in the fleet can move a bearer from a worker back to the vault machine,
# so the way to make one bearer true everywhere is to rotate onto a new one and
# deliver it to both holders in the same operation.
#
# The capability list is the existing union and nothing more: the renter gate's
# `read:stado-vast#api_key` cannot be minted while this vault holds no
# `stado-vast` item, and `token-mint` says so ("capability names a missing item").
#
# Delivery is the caller's half, with `stado host install-secret` to each holder.
# Takes no operator words: consumer, bearer path and holder count are checked in.
set -eu

script="$HOME/.stado/files/rotate-consumer-bearer.py"
bearer="$HOME/.stado/local-agent-skarbiec-token"
[ -r "$script" ] || {
  printf 'ERROR\t%s absent; deliver it with `stado host install-file`\n' "$script" >&2
  exit 1
}
[ -r "$bearer" ] || {
  printf 'ERROR\t%s absent; deliver it with `stado host install-secret` first\n' "$bearer" >&2
  exit 1
}

exec /usr/bin/python3 "$script" stado-local-agent "$bearer" 2
