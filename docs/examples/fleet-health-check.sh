#!/bin/sh
# fleet-health-check.sh — fleet truth without ssh:
# who reported lately, who answers, what services run.
# Run: sh fleet-health-check.sh
set -eu

# every registry host and its last heartbeat, worst first
stado registry beacon-age

# reachability verdict per host (ssh check + beacon age)
stado host ping --json

# managed services across the fleet, from beacons alone
stado service list
