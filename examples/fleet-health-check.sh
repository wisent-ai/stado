#!/bin/sh
# fleet-health-check.sh — fleet truth without ssh:
# who reported lately, who answers, what services run.
# Run: sh fleet-health-check.sh
set -eu

# every registry host and its last heartbeat, worst first
stado registry beacon-age

# reachability verdict per host (ssh check + beacon age); ping takes one
# TARGET, so walk every registry host. `|| true`: one down host must not
# stop the sweep — its verdict is the point.
stado registry pull \
  | python3 -c 'import json,sys; [print(t["name"]) for t in json.load(sys.stdin).get("targets", [])]' \
  | while IFS= read -r target; do
      stado host ping "$target" --json || true
    done

# managed services across the fleet, from beacons alone
stado service list
