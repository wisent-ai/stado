#!/bin/sh
# Report which block devices this host has, which are mounted, and what
# /etc/fstab intends to mount -- the question `stado host exec` cannot ask,
# because its allowlist carries `df -h` only and a filesystem that is absent
# from `df` is indistinguishable there from one that was never declared.
#
# Written for the 2026-08-17 finding: the registry declares
# `training.models_dir = /mnt/wd16tb/stado/training` on the RTX host while
# `df -h` shows no such mount, so a declared path resolves onto a root volume
# with 12 GiB free. Answering "is the disk gone, or merely unmounted" needs the
# block-device list, and that list is read-only.
#
# Takes no operator words: a helper that took them would be a remote shell.
set -eu

printf 'BLOCK_DEVICES\n'
if command -v lsblk >/dev/null 2>&1; then
  lsblk -o NAME,SIZE,TYPE,FSTYPE,UUID,MOUNTPOINT 2>&1 || true
else
  printf 'lsblk unavailable\n'
fi

printf '\nFSTAB_NON_COMMENT\n'
if [ -r /etc/fstab ]; then
  grep -v '^[[:space:]]*#' /etc/fstab | grep -v '^[[:space:]]*$' || true
else
  printf '/etc/fstab unreadable\n'
fi

printf '\nMOUNTS_UNDER_MNT\n'
awk '$2 ~ /^\/mnt/ { print $1, $2, $3, $4 }' /proc/self/mounts || true

printf '\nDECLARED_PATHS\n'
for path in /mnt/wd16tb /mnt/wd16tb/stado/training /mnt/wd16tb/stado/inference /mnt/wd16tb/wisent-cache; do
  if [ -d "$path" ]; then
    device=$(df -P "$path" 2>/dev/null | awk 'NR==2 { print $1 }')
    avail=$(df -Pk "$path" 2>/dev/null | awk 'NR==2 { print $4 }')
    printf '%s\tdirectory\t%s\t%s KiB available\n' "$path" "${device:-unknown}" "${avail:-unknown}"
  else
    printf '%s\tabsent\n' "$path"
  fi
done

printf '\nROOT_FILESYSTEM\n'
df -Ph / || true
