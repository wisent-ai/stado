#!/bin/sh
# Rotate the worker-agent grant onto the bearer just delivered here, and add the
# one capability its renter gate needs.
#
# `grant-agent-vast-read` is the operation to prefer, and it refused for the right
# reason: the bearer file beside this vault is a stale copy that does not hash to
# what the vault recorded for `stado-local-agent`. The live bearer is on the
# worker, and nothing in the fleet can move a bearer back to the vault machine.
#
# So the grant is re-minted onto a bearer the operator generated and delivered to
# both machines with `stado host install-secret`, with `read:stado-vast#api_key`
# added to the existing capability list. Verified before running: exactly one host
# holds this consumer's bearer -- neither Mac carries an agent grant env file, and
# the worker's bearer answered HTTP 200 for a field it holds.
#
# Takes no operator words: the consumer, the bearer path, the holder count and
# the capability are checked in here.
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

exec /usr/bin/python3 "$script" stado-local-agent "$bearer" 1 read:stado-vast#api_key
