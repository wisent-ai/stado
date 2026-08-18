#!/bin/sh
# What the vastai-owned processes on this host actually are.
#
# I read `vastai_*` processes plus a python3 holding 60 GiB on card 0 as "a
# paying renter is on this machine" and wrote that into a commit message and an
# incident note. The operator says nothing is rented here. This is the probe I
# should have run before asserting it: which containers exist, what state they
# are in, and whether anything of theirs is on a GPU right now.
#
# Read-only.
set -eu

printf 'VASTAI_PROCESSES\n'
ps ax -o user= -o pid= -o etime= -o comm= | awk '$1 ~ /^vastai/ { print }' | head -20 || printf 'none\n'

printf '\nKAALIA\n'
ps ax -o user= -o pid= -o etime= -o comm= | grep -i kaalia | head -5 || printf 'not running\n'

printf '\nDOCKER_CONTAINERS\n'
if command -v docker >/dev/null 2>&1; then
  docker ps -a --format '{{.Names}}\t{{.Image}}\t{{.Status}}' 2>&1 | head -20
else
  printf 'docker unavailable\n'
fi

printf '\nGPU_COMPUTE_APPS\n'
if command -v nvidia-smi >/dev/null 2>&1; then
  apps=$(nvidia-smi --query-compute-apps=pid,process_name,used_memory --format=csv,noheader 2>&1)
  if [ -n "$apps" ]; then printf '%s\n' "$apps"; else printf 'none\n'; fi
  printf 'PER_GPU\t'
  nvidia-smi --query-gpu=index,memory.used,utilization.gpu --format=csv,noheader
else
  printf 'nvidia-smi unavailable\n'
fi
