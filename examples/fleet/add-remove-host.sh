#!/bin/sh
# add-remove-host.sh — add a device to the fleet and remove it again.
# Ends net-zero: the registry looks exactly like before.
# Usage: sh add-remove-host.sh <host> <ssh-destination> <release-platform>
set -eu

HOST=$1
DEST=$2
PLATFORM=$3

# onboard into the canonical registry (validated on write). This is the
# declaration on its own: `stado fleet enroll` is the path that probes the
# machine for its hostname and platform before writing them.
stado registry host add "$HOST" --ssh "$DEST" --release-platform "$PLATFORM"

# the fleet sees it
stado registry beacon-age

# removal: read the document AND the generation it is at in ONE call, drop the
# host, validate, then write conditionally on that same generation. Two pulls
# would pair a token with a document it does not describe; without
# --if-generation the push replaces whatever is there now, so anything
# published between the read and the write is erased by this edit.
stado registry pull --with-generation > ~/.stado/registry-pull.json
GENERATION=$(jq -r '.generation' ~/.stado/registry-pull.json)
jq --arg h "$HOST" '.document | .targets |= map(select(.name != $h))' \
  ~/.stado/registry-pull.json > ~/.stado/registry-edit.json
stado registry validate ~/.stado/registry-edit.json

# exit 75 means "somebody wrote first": re-read, re-apply, push again. It never
# means the store is broken, and --force would not help - forcing waves past
# the deleted-key guard, not past a generation that has moved on. `--json` adds
# a stado.registry-push-receipt.v1 object naming both generations.
set +e
stado registry push ~/.stado/registry-edit.json --if-generation "$GENERATION"
STATUS=$?
set -e
if [ "$STATUS" -eq 75 ]; then
  echo "the registry moved while this edit was being made; re-run to re-apply" >&2
fi
[ "$STATUS" -eq 0 ] || exit "$STATUS"

# the fleet no longer sees it
stado registry beacon-age
