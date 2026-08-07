#!/usr/bin/env bash
# Report which pieces of the host-health beacon this machine already has.
#
# The registry declares the beacon on hosts that have never had it installed,
# and `registry doctor` then reports a divergence that says only "no beacon
# exists". Which of the five pieces is missing is the whole repair, and it is
# knowable without changing anything.
#
# Read-only. Prints paths and presence, never a credential.
set -eu

report() {
  if [ -e "$1" ]; then
    printf 'present  %s\n' "$1"
  else
    printf 'MISSING  %s\n' "$1"
  fi
}

home_dir=${HOME:-/home/ubuntu}

report "$home_dir/wisent-compute-deploy/deploy/host_health_beacon.sh"
report /etc/stado/host-health.env
report "$home_dir/.stado/host-health-beacon-skarbiec-token"
report "$home_dir/.stado/bin/stado"
report /etc/systemd/system/host-health-beacon.service
report /etc/systemd/system/host-health-beacon.timer
