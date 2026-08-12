#!/bin/sh
# Report who serves the control-plane port on this host, and under what.
#
# Read-only. An endpoint the whole fleet depends on turned out to be an
# unmanaged process on a build old enough to write host beacons where current
# readers no longer look, and nothing in the registry said so. This answers
# three questions the registry cannot: which process listens, whether launchd
# owns it, and which binary it actually runs.
set -eu
port=${1:-8765}

managed=$(/bin/launchctl list 2>/dev/null | /usr/bin/awk '$1 ~ /^[0-9]+$/ {print $1}')

/usr/sbin/lsof -nP -iTCP:"$port" -sTCP:LISTEN -Fpcn 2>/dev/null \
  | /usr/bin/awk '
      /^p/ {pid=substr($0,2)}
      /^c/ {comm=substr($0,2)}
      /^n/ {printf "%s\t%s\t%s\n", pid, comm, substr($0,2)}
    ' \
  | while IFS="$(printf '\t')" read -r pid comm addr; do
      if printf '%s\n' "$managed" | /usr/bin/grep -qx "$pid"; then
        owner=launchd
      else
        owner=unmanaged
      fi
      command=$(/bin/ps -p "$pid" -o command= 2>/dev/null | /usr/bin/cut -c1-160)
      printf 'listener\t%s\tpid=%s\towner=%s\taddr=%s\n' "$comm" "$pid" "$owner" "$addr"
      printf '  command\t%s\n' "$command"
      case "$command" in
        */stado*)
          binary=$(printf '%s\n' "$command" | /usr/bin/awk '{print $1}')
          if [ -x "$binary" ]; then
            printf '  version\t%s\n' "$("$binary" --version 2>/dev/null || echo unknown)"
          fi
          ;;
      esac
    done
