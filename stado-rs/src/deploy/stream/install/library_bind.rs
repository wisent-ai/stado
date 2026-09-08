//! The half of the storage question that reshapes a host: only reached when
//! the operator asked for it, and spliced into the reconcile program where the
//! library's free space has just been read.

pub(super) const LIBRARY_BIND: &str = r#"if [ "$library_device" = "$root_device" ] && [ "$library_free_kib" -lt "$minimum_kib" ]; then
  # Only a real block-backed filesystem, and never a container's overlay: the
  # first version of this search picked
  # /var/lib/docker/overlay2/<id>/merged, which is one running container's
  # filesystem and vanishes with it.
  source_mount=$(awk '$3 ~ /^(ext4|ext3|xfs|btrfs|zfs|f2fs)$/ && $2 != "/" && $2 != "/boot" { print $2 }' /proc/self/mounts |
    while read -r point; do
      printf '%s %s\n' "$(df -Pk "$point" | awk 'NR==2 { print $4 }')" "$point"
    done | sort -n -r | sed -n 1p | cut -d' ' -f2)
  if [ -z "$source_mount" ]; then
    printf 'ERROR\tno disk-backed filesystem here has room for a session library\n' >&2
    exit 1
  fi
  backing="$source_mount/wisent-games"
  mkdir -p "$backing"
  if ! awk -v point='LIBRARY_DIR' '$2 == point { found = 1 } END { exit !found }' /proc/self/mounts; then
    mount --bind "$backing" "LIBRARY_DIR"
  fi
  line="$backing LIBRARY_DIR none bind 0 0 # stado-stream"
  if ! grep -Fxq "$line" /etc/fstab; then
    cp -p /etc/fstab "/etc/fstab.before-stream-library-$(date -u +%Y%m%d)"
    printf '%s\n' "$line" >>/etc/fstab
  fi
  library_device=$(df -P "LIBRARY_DIR" | awk 'NR==2 { print $1 }')
  library_free_kib=$(df -Pk "LIBRARY_DIR" | awk 'NR==2 { print $4 }')
  printf 'LIBRARY_PROVISIONED\t%s bound to LIBRARY_DIR\n' "$backing"
fi
"#;
