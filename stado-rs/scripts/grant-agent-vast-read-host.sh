#!/bin/sh
# Let the worker agents read the Vast key through their own grant.
#
# `stado agent` pauses fleet claims while a Vast.ai renter holds the card, and on
# the machine that is actually rented -- the RTX host -- that gate could not
# evaluate: the key is read as `stado-control-plane`, whose bearer only this
# control-plane host holds. Its own consumer, `stado-local-agent`, answered
# HTTP 200 for a field it holds and HTTP 403 for `stado-vast#api_key`.
#
# So the capability is added to the consumer the worker already authenticates
# as, which is least privilege in the direction that matters: the worker gains
# one field, and no worker ever needs a control-plane bearer.
#
# The addition itself is `scripts/grant-consumer-field-read.py`, delivered beside
# this helper: it reads the live grant, refuses unless the recorded bearer can be
# reproduced from the owner-only file, takes the union with what is requested,
# preserves the remaining TTL, and re-mints with `--token-file` so the bearer is
# written back unchanged. Running it twice changes nothing.
#
# Takes no operator words: the consumer, item and field are checked in here.
set -eu

script="$HOME/.stado/files/grant-consumer-field-read.py"
[ -r "$script" ] || {
  printf 'ERROR\t%s absent; deliver it with `stado host install-file`\n' "$script" >&2
  exit 1
}

exec /usr/bin/python3 "$script" stado-local-agent stado-vast api_key
