#!/bin/sh
# Give this host a real, stable artifact root for label-model training.
#
# The registry declared `training.models_dir = /mnt/wd16tb/stado/training` on
# the RTX host, and on 2026-08-17 `lsblk` showed that disk is not there: fstab
# carries `UUID=7c4525d1-... /mnt/wd16tb xfs defaults,nofail 0 2`, the `nofail`
# let the host boot without it, and `/mnt/wd16tb` is now an empty directory on
# the 100 GiB root volume with 12 GiB free. A trainer honouring that
# declaration would write model artifacts onto the volume the disk janitor is
# already enforcing a low-water mark on.
#
# So the declared path becomes a mount point this host can actually keep:
# `/mnt/wisent-training`, bound to `wisent-training` on the only large
# filesystem here (the 3.5 TiB docker LV, ~3.2 TiB free). That is the same
# shape the host already uses for agent staging -- `/var/lib/docker/wisent-staging`
# bound to `/mnt/wisent-staging` -- and it keeps the registry declaration true
# through a disk change: when a data disk is attached again, mount it at
# `/mnt/wisent-training` and the declaration does not move.
#
# Idempotent, and it never touches an existing mount: it creates the source
# directory, creates the mount point, binds it if nothing is mounted there, and
# appends one fstab line only when that exact line is absent. Takes no operator
# words: a helper that took them would be a remote shell.
set -eu

SOURCE=/var/lib/docker/wisent-training
POINT=/mnt/wisent-training
ARTIFACTS="$POINT/stado/training"
FSTAB_LINE="$SOURCE $POINT none bind 0 0"

if [ "$(id -u)" -ne 0 ]; then
  printf 'ERROR\tthis helper mounts and writes /etc/fstab; it must run as root\n' >&2
  exit 1
fi

mkdir -p "$SOURCE"
chmod 0755 "$SOURCE"
mkdir -p "$POINT"

if awk -v point="$POINT" '$2 == point { found = 1 } END { exit !found }' /proc/self/mounts; then
  printf 'MOUNT\talready mounted at %s\n' "$POINT"
else
  mount --bind "$SOURCE" "$POINT"
  printf 'MOUNT\tbound %s to %s\n' "$SOURCE" "$POINT"
fi

# Persist across reboots. Written after the bind succeeded, so a line that
# cannot be satisfied never reaches the boot path.
if grep -Fxq "$FSTAB_LINE" /etc/fstab; then
  printf 'FSTAB\talready declared\n'
else
  cp -p /etc/fstab "/etc/fstab.before-wisent-training-$(date -u +%Y%m%d)"
  printf '%s\n' "$FSTAB_LINE" >>/etc/fstab
  printf 'FSTAB\tappended: %s\n' "$FSTAB_LINE"
fi

mkdir -p "$ARTIFACTS"
printf 'ARTIFACTS\t%s\n' "$ARTIFACTS"
printf 'DEVICE\t'
df -Ph "$ARTIFACTS" | awk 'NR==2 { print $1, $2 " size", $4 " available" }'
printf 'WRITABLE\t'
probe="$ARTIFACTS/.write-probe.$$"
if : >"$probe" 2>/dev/null; then
  rm -f "$probe"
  printf 'yes\n'
else
  printf 'no\n'
  exit 1
fi
