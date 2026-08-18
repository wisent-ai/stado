#!/usr/bin/env bash
# Remove the fstab line that bound a session library onto a container's overlay.
#
# The first `stado stream apply --provision-library` searched for the largest
# filesystem with `df --output`, which GNU df refuses beside `-P`; the search
# came back empty, fell through to the next candidate and bound
# /mnt/wisent-games to /var/lib/docker/overlay2/<id>/merged/wisent-games — one
# running container's filesystem, which disappears with the container. The mount
# is already gone; this removes the boot-time line it left behind.
#
# The code no longer produces such a line: the search now reads /proc/self/mounts
# and accepts only ext4/xfs/btrfs/zfs/f2fs, and every line it writes carries a
# `# stado-stream` tag so `stream stop --purge` can remove exactly its own.
#
# Idempotent, backs up /etc/fstab, and touches only lines whose mount point is
# the session library and whose source sits under docker's overlay tree.
set -euo pipefail

point=/mnt/wisent-games
fstab=/etc/fstab

if ! awk -v point="$point" '$2 == point && $1 ~ /overlay2/ { found = 1 } END { exit !found }' "$fstab"; then
  printf 'SETTLED\tno overlay-backed line for %s\n' "$point"
  exit 0
fi

cp -p "$fstab" "$fstab.before-stream-overlay-removal-$(date -u +%Y%m%d)"
awk -v point="$point" '!($2 == point && $1 ~ /overlay2/)' "$fstab" >"$fstab.stream-new"
mv "$fstab.stream-new" "$fstab"
printf 'REMOVED\tthe overlay-backed bind line for %s\n' "$point"

printf 'MOUNTED\t'
if awk -v point="$point" '$2 == point { found = 1 } END { exit !found }' /proc/self/mounts; then
  printf 'still mounted; unmounting\n'
  umount "$point"
else
  printf 'not mounted\n'
fi

printf 'FSTAB_LINES_FOR_POINT\t'
grep -c "$point" "$fstab" || true
