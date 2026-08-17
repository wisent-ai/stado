#!/bin/sh
# add-remove-host.sh — add a device to the fleet and remove it again.
# Ends net-zero: the registry looks exactly like before.
# Usage: sh add-remove-host.sh <host> <ssh-destination> <release-platform>
set -eu

HOST=$1
DEST=$2
PLATFORM=$3

# onboard into the canonical registry (validated on write). This is the
# declaration on its own: `stado_fleet enroll` is the path that probes the
# machine for its hostname and platform before writing them.
stado registry host add "$HOST" --ssh "$DEST" --release-platform "$PLATFORM"

# the fleet sees it
stado registry beacon-age

# removal: pull the document, drop the host, validate, push back
stado registry pull > ~/.stado/registry-edit.json
jq --arg h "$HOST" '.targets |= map(select(.name != $h))' \
  ~/.stado/registry-edit.json > ~/.stado/registry-edit-out.json
mv ~/.stado/registry-edit-out.json ~/.stado/registry-edit.json
stado registry validate ~/.stado/registry-edit.json
stado registry push ~/.stado/registry-edit.json

# the fleet no longer sees it
stado registry beacon-age
