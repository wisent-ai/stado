#!/bin/sh
# Reconcile Skarbiec's short-lived acquisition state after a service-user cutover.
# Invoked by `stado host recover-skarbiec-acquisition-state`.
set -eu
PATH="/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
export PATH

vault="${SKARBIEC_VAULT_FILE:-$HOME/.stado/skarbiec.vault.json}"
state="${SKARBIEC_ACQUISITION_FILE:-${vault}.acquisitions.json}"
legacy_lock="${state}.lock"
lock="${state}.advisory.lock"
uid=$(/usr/bin/id -u)
gid=$(/usr/bin/id -g)
chown_bin=$(command -v chown)
platform=$(/usr/bin/uname -s)
changed=

owner_id() {
  case "$platform" in
    Darwin) /usr/bin/stat -f '%u' "$1" ;;
    *) /usr/bin/stat -c '%u' "$1" ;;
  esac
}

inode_mtime() {
  case "$platform" in
    Darwin) /usr/bin/stat -f '%i:%m' "$1" ;;
    *) /usr/bin/stat -c '%i:%Y' "$1" ;;
  esac
}

mtime() {
  case "$platform" in
    Darwin) /usr/bin/stat -f '%m' "$1" ;;
    *) /usr/bin/stat -c '%Y' "$1" ;;
  esac
}

mode_bits() {
  case "$platform" in
    Darwin) /usr/bin/stat -f '%Lp' "$1" ;;
    *) /usr/bin/stat -c '%a' "$1" ;;
  esac
}

reconcile_owner() {
  path=$1
  mode=$2
  kind=$3
  if [ ! -e "$path" ]; then
    return
  fi
  if [ -L "$path" ]; then
    printf '%s\n' "refusing recovery: $kind is a symbolic link: $path" >&2
    exit 1
  fi
  case "$kind" in
    state) [ -f "$path" ] || { printf '%s\n' "refusing recovery: acquisition state is not a regular file: $path" >&2; exit 1; } ;;
    lock) [ -f "$path" ] || { printf '%s\n' "refusing recovery: advisory lock is not a regular file: $path" >&2; exit 1; } ;;
  esac
  if [ "$(owner_id "$path")" != "$uid" ]; then
    /usr/bin/sudo -n "$chown_bin" "$uid:$gid" "$path"
    changed="${changed}${changed:+, }owner of $kind"
  fi
  if [ "$(mode_bits "$path")" != "$mode" ]; then
    /bin/chmod "$mode" "$path"
    changed="${changed}${changed:+, }mode of $kind"
  fi
}

reconcile_owner "$state" 600 state

if [ -d "$legacy_lock" ]; then
  first=$(inode_mtime "$legacy_lock")
  /bin/sleep 6
  second=$(inode_mtime "$legacy_lock")
  if [ "$first" != "$second" ]; then
    printf '%s\n' 'refusing recovery: the legacy acquisition lock changed while observed' >&2
    exit 1
  fi
  age=$(( $(/bin/date +%s) - $(mtime "$legacy_lock") ))
  if [ "$age" -lt 10 ]; then
    printf '%s\n' 'refusing recovery: the legacy acquisition lock is still recent' >&2
    exit 1
  fi
  /usr/bin/sudo -n /bin/rmdir "$legacy_lock"
  changed="${changed}${changed:+, }stale legacy lock"
elif [ -e "$legacy_lock" ]; then
  printf '%s\n' "refusing recovery: legacy acquisition lock is not a directory: $legacy_lock" >&2
  exit 1
fi

reconcile_owner "$lock" 600 lock

if [ -n "$changed" ]; then
  printf '%s\n' "recovered Skarbiec acquisition state: $changed"
else
  printf '%s\n' 'Skarbiec acquisition state ownership is healthy; no recovery needed'
fi
