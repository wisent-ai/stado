#!/bin/sh
# fleet-health-check.sh — fleet truth without ssh:
# who reported lately, who answers, what services run.
# Run: sh fleet-health-check.sh
set -eu

# every registry host and its last heartbeat, worst first
stado host beacon list

# reachability verdict per host (ssh check + beacon age); ping takes one
# TARGET, so walk every registry host. One down host must not stop the
# sweep — its verdict is the point — so a refused ping is named and the
# walk goes on.
stado registry pull \
  | jq -r '.targets[].name' \
  | while IFS= read -r target; do
      if ! stado host ping "$target" --json; then
        printf '%s\n' "host $target: ping refused (its verdict is above)" >&2
      fi
    done

# managed services across the fleet, from beacons alone
stado service list
