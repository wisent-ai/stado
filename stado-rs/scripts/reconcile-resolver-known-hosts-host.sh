#!/bin/sh
set -eu
umask 077

source_file=${STADO_RESOLVER_KNOWN_HOSTS_SOURCE:-$HOME/.stado/files/stado-resolver-known-hosts}
ssh_directory=$HOME/.ssh
destination=$ssh_directory/known_hosts

[ -s "$source_file" ] || {
  printf '%s\n' "resolver known-host source is missing or empty: $source_file" >&2
  exit 1
}
/usr/bin/ssh-keygen -l -f "$source_file" >/dev/null

/bin/mkdir -p "$ssh_directory"
/bin/chmod 700 "$ssh_directory"
existing=/dev/null
[ ! -f "$destination" ] || existing=$destination
temporary=$(/usr/bin/mktemp "$ssh_directory/.known_hosts.XXXXXX")
trap '/bin/rm -f "$temporary"' EXIT HUP INT TERM
/usr/bin/awk 'NF && !seen[$0]++' "$existing" "$source_file" >"$temporary"
/usr/bin/ssh-keygen -l -f "$temporary" >/dev/null
/bin/chmod 600 "$temporary"
/bin/mv "$temporary" "$destination"
trap - EXIT HUP INT TERM
printf '%s\n' "reconciled resolver peer keys -> $destination"
