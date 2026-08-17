#!/bin/sh
# Put this host's relocated caches back on a disk it still has.
#
# `relocate-rust-cache-host.sh` moved `~/.cargo`, `~/.rustup` and
# `~/.cache/huggingface` onto the 16 TB volume and left symlinks behind. That
# volume has been removed, so all three links resolve to nothing -- and because
# `mkdir` answers EEXIST for a dangling symlink, every consumer failed with a
# message about a file existing and none of them named the link:
#
#   rustup:  could not create home directory: '/root/.rustup': File exists
#   agent:   agent loop failed: File exists (os error 17)
#
# The cache root moves to `/mnt/wisent-cache`, a mount point bound to the only
# large filesystem here, in the same shape as `/mnt/wisent-staging` and
# `/mnt/wisent-training`. Contents are not recovered -- they were on the disk
# that left -- so each cache starts empty and refills.
#
# Idempotent: a link that already resolves is left alone, the fstab line is
# appended only when absent, and the bind happens only if nothing is mounted at
# the point. Takes no operator words: a helper that took them would be a remote
# shell.
set -eu

SOURCE=/var/lib/docker/wisent-cache
POINT=/mnt/wisent-cache
FSTAB_LINE="$SOURCE $POINT none bind 0 0"

if [ "$(id -u)" -ne 0 ]; then
  printf 'ERROR\tthis helper mounts and rewrites root-owned links; it must run as root\n' >&2
  exit 1
fi

mkdir -p "$SOURCE"
chmod 0700 "$SOURCE"
mkdir -p "$POINT"

if awk -v point="$POINT" '$2 == point { found = 1 } END { exit !found }' /proc/self/mounts; then
  printf 'MOUNT\talready mounted at %s\n' "$POINT"
else
  mount --bind "$SOURCE" "$POINT"
  printf 'MOUNT\tbound %s to %s\n' "$SOURCE" "$POINT"
fi

if grep -Fxq "$FSTAB_LINE" /etc/fstab; then
  printf 'FSTAB\talready declared\n'
else
  cp -p /etc/fstab "/etc/fstab.before-wisent-cache-$(date -u +%Y%m%d)"
  printf '%s\n' "$FSTAB_LINE" >>/etc/fstab
  printf 'FSTAB\tappended: %s\n' "$FSTAB_LINE"
fi

relink() {
  link=$1
  destination=$2
  mkdir -p "$destination"
  if [ -L "$link" ] && [ ! -e "$link" ]; then
    printf 'LINK\t%s was dangling -> %s; now -> %s\n' "$link" "$(readlink "$link")" "$destination"
    rm -f "$link"
    ln -s "$destination" "$link"
  elif [ -L "$link" ]; then
    printf 'LINK\t%s already resolves -> %s\n' "$link" "$(readlink "$link")"
  elif [ -e "$link" ]; then
    printf 'LINK\t%s is real content; left untouched\n' "$link"
  else
    mkdir -p "$(dirname "$link")"
    ln -s "$destination" "$link"
    printf 'LINK\t%s created -> %s\n' "$link" "$destination"
  fi
}

relink /root/.cargo "$POINT/.cargo"
relink /root/.rustup "$POINT/.rustup"
relink /root/.cache/huggingface "$POINT/.cache/huggingface"

printf '\nVERIFY\n'
for link in /root/.cargo /root/.rustup /root/.cache/huggingface; do
  if [ -d "$link" ]; then
    df -Ph "$link" | awk -v l="$link" 'NR==2 { print l "\t" $1 "\t" $4 " available" }'
  else
    printf '%s\tstill unusable\n' "$link"
    exit 1
  fi
done
