#!/bin/sh
# Put this host's agent back on storage roots that exist.
#
# Two roots pointed into a 16 TB disk that has been removed from the machine
# (fstab still carries it `nofail`, so the host boots and the mount point is an
# empty directory on the 100 GiB root volume):
#
#   ~/.stado/local-backup -> /mnt/wd16tb/stado-local-backup
#       The configured read-failover store. `JobStorage::new()` builds it with
#       `fs::create_dir_all`, and mkdir answers EEXIST for a path that already
#       exists as a dangling symlink, so every agent tick died with
#       `agent loop failed: File exists (os error 17)` -- ten seconds apart,
#       for days, while `systemctl is-active` answered `active` because the
#       binary restarts its own loop. No capacity object was ever published, so
#       the fleet's only GPU host was invisible to placement.
#
#   TMPDIR=/mnt/wd16tb/wisent-staging in the unit
#       `disk_staging` keeps an explicit TMPDIR verbatim, so multi-GB activation
#       staging was aimed at the volume with 12 GiB free and a janitor enforcing
#       an 8 GiB floor.
#
# Both move to the only large filesystem this host has (the 3.5 TiB docker LV,
# ~3.2 TiB free): the backup store to a real directory behind the same symlink,
# and staging to `/mnt/wisent-staging`, the bind the agent already created there.
#
# Idempotent, and it changes nothing it does not have to: the symlink is
# replaced only while it fails to resolve, the drop-in is written only when its
# content differs, and the unit is restarted only when something changed.
# Takes no operator words: a helper that took them would be a remote shell.
set -eu

BACKUP_LINK=/root/.stado/local-backup
BACKUP_STORE=/var/lib/docker/wisent-stado-local-backup
STAGING=/mnt/wisent-staging
UNIT=wisent-agent.service
DROPIN_DIR="/etc/systemd/system/$UNIT.d"
DROPIN="$DROPIN_DIR/zz-staging-path.conf"
DROPIN_BODY="[Service]
Environment=TMPDIR=$STAGING
"

if [ "$(id -u)" -ne 0 ]; then
  printf 'ERROR\tthis helper writes /root and a systemd drop-in; it must run as root\n' >&2
  exit 1
fi

changed=0

mkdir -p "$BACKUP_STORE"
chmod 0700 "$BACKUP_STORE"

if [ -L "$BACKUP_LINK" ] && [ ! -e "$BACKUP_LINK" ]; then
  printf 'BACKUP\tdangling -> %s; repointing to %s\n' "$(readlink "$BACKUP_LINK")" "$BACKUP_STORE"
  rm -f "$BACKUP_LINK"
  ln -s "$BACKUP_STORE" "$BACKUP_LINK"
  changed=1
elif [ -L "$BACKUP_LINK" ]; then
  printf 'BACKUP\talready resolves -> %s\n' "$(readlink "$BACKUP_LINK")"
elif [ -d "$BACKUP_LINK" ]; then
  printf 'BACKUP\talready a directory\n'
else
  printf 'BACKUP\tabsent; creating symlink to %s\n' "$BACKUP_STORE"
  ln -s "$BACKUP_STORE" "$BACKUP_LINK"
  changed=1
fi

if [ ! -d "$STAGING" ]; then
  printf 'ERROR\t%s does not exist; run install-training-artifact-root first\n' "$STAGING" >&2
  exit 1
fi

mkdir -p "$DROPIN_DIR"
if [ -f "$DROPIN" ] && [ "$(cat "$DROPIN")" = "$DROPIN_BODY" ]; then
  printf 'STAGING\tdrop-in already declares TMPDIR=%s\n' "$STAGING"
else
  printf '%s' "$DROPIN_BODY" >"$DROPIN"
  printf 'STAGING\twrote %s with TMPDIR=%s\n' "$DROPIN" "$STAGING"
  changed=1
fi

if [ "$changed" -eq 1 ]; then
  systemctl daemon-reload
  systemctl restart "$UNIT"
  printf 'UNIT\tdaemon-reload and restart issued\n'
  sleep 12
else
  printf 'UNIT\tnothing changed; left running\n'
fi

printf '\nVERIFY\n'
printf 'BACKUP_TARGET\t'
if [ -d "$BACKUP_LINK" ]; then
  df -Ph "$BACKUP_LINK" | awk 'NR==2 { print $1, $4 " available" }'
else
  printf 'still unusable\n'
fi
printf 'TMPDIR_EFFECTIVE\t'
systemctl show "$UNIT" --property=Environment --value | tr ' ' '\n' | grep '^TMPDIR=' | tail -n 1 || printf 'unset\n'
printf 'UNIT_STATE\t'
systemctl is-active "$UNIT" || true

printf '\nAGENT_LOG_TAIL\n'
journalctl -u "$UNIT" --no-pager -n 25 -o cat 2>&1 | tail -n 25 || true
