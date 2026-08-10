#!/bin/sh
set -eu

nvidia_smi=/usr/bin/nvidia-smi
if [ ! -x "$nvidia_smi" ]; then
  printf '%s\n' "nvidia-smi is unavailable" >&2
  exit 1
fi

printf '%s\n' 'GPU,index,name,total_mib,used_mib,free_mib,utilization_percent'
"$nvidia_smi" \
  --query-gpu=index,name,memory.total,memory.used,memory.free,utilization.gpu \
  --format=csv,noheader,nounits

printf '%s\n' 'PROCESS,pid,name,used_mib'
"$nvidia_smi" \
  --query-compute-apps=pid,process_name,used_memory \
  --format=csv,noheader,nounits
