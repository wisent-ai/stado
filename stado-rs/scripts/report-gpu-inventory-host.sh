#!/bin/sh
# Report every accelerator this host actually has, from the driver.
#
# `stado host exec` carries no `nvidia-smi` -- its allowlist is macOS-shaped --
# and the registry's `gpu_type` / `vram_gb` are one string and one number per
# target, so a host with two cards is indistinguishable there from a host with
# one. That ambiguity is not academic: admission sizes jobs from live per-GPU
# VRAM (`providers/local/agent.rs`), while placement reads the single declared
# number.
#
# Read-only, and it takes no operator words: a helper that took them would be a
# remote shell.
set -eu

if ! command -v nvidia-smi >/dev/null 2>&1; then
  printf 'ERROR\tnvidia-smi unavailable\n' >&2
  exit 1
fi

printf 'DRIVER\t'
nvidia-smi --query-gpu=driver_version --format=csv,noheader | head -n 1

printf '\nGPUS\n'
nvidia-smi --query-gpu=index,name,uuid,memory.total,memory.used,utilization.gpu,power.limit,display_mode,display_active \
  --format=csv,noheader

printf '\nCOMPUTE_APPS\n'
nvidia-smi --query-compute-apps=gpu_uuid,pid,process_name,used_memory --format=csv,noheader || true

printf '\nPCI_DISPLAY_DEVICES\n'
if command -v lspci >/dev/null 2>&1; then
  lspci -nn | grep -iE 'vga|3d controller|display controller' || printf 'none\n'
else
  printf 'lspci unavailable\n'
fi

printf '\nDRM_NODES\n'
ls -1 /dev/dri 2>/dev/null || printf 'none\n'
