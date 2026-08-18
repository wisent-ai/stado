#!/usr/bin/env bash
# Which session packages this host can actually install, and what the last
# `stream apply` left behind.
#
# `apt-get install` answered exit 100 with "[no choices]", which is a dependency
# resolution failure and says nothing about which package caused it. This asks
# per package instead of guessing, and it also shows the mount `--provision-library`
# created: the search picked a docker overlay, which is a container's filesystem
# and disappears with the container.
#
# Read-only.
set -euo pipefail

printf 'RELEASE\t%s\n' "$(. /etc/os-release && printf '%s %s' "$ID" "$VERSION_ID")"

printf '\nCANDIDATES\n'
for package in xserver-xorg-core xserver-xorg-input-libinput xserver-xorg-video-nvidia-580 xinit x11-xserver-utils openbox pulseaudio pipewire wireplumber xdotool steam-installer; do
  candidate=$(apt-cache policy "$package" 2>/dev/null | awk '/Candidate:/ { print $2 }')
  printf '%s\t%s\n' "$package" "${candidate:-not in any source}"
done

printf '\nLIBRARY_MOUNT\n'
awk '$2 == "/mnt/wisent-games" { print $1, $2, $3 }' /proc/self/mounts || printf 'not mounted\n'
grep -n 'wisent-games' /etc/fstab || printf 'no fstab line\n'

printf '\nREAL_FILESYSTEMS\n'
awk '$3 ~ /^(ext4|xfs|btrfs|zfs|f2fs|ext3)$/ { print $2, $3 }' /proc/self/mounts |
  while read -r point type; do
    printf '%s\t%s\t%s KiB free\n' "$point" "$type" "$(df -Pk "$point" | awk 'NR==2 { print $4 }')"
  done
