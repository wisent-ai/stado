#!/bin/sh
# Report this host's Stado forward markers: mode, age, name, value.
#
# Install:
#   stado host install-helper <target> \
#     stado-rs/scripts/report-forwards.sh report-forwards
#
# Several products resolve a service address from `~/.stado/forwards/<name>.local`
# rather than an environment variable. Those files were historically written by
# hand, so a fleet accumulates markers for services that were renamed, moved, or
# never existed -- one on this fleet names a port nothing has ever bound. A
# consumer holding an old name keeps resolving it forever, because writing a
# marker has a producer and removing one does not.
#
# Read-only by construction: it prints what is there and changes nothing. Knowing
# which markers are fossils has to come before regenerating them, because the
# directory is not automatically the better answer for a marker somebody wrote by
# hand precisely when the directory was wrong.
set -eu

dir="${STADO_FORWARDS_DIR:-$HOME/.stado/forwards}"
if [ ! -d "$dir" ]; then
  printf 'no forwards directory at %s\n' "$dir"
  exit 0
fi

cd "$dir"
found=0
for marker in *.local; do
  [ -e "$marker" ] || continue
  found=1
  mode=$(/usr/bin/stat -f '%Sp' "$marker" 2>/dev/null || /usr/bin/stat -c '%A' "$marker")
  when=$(/usr/bin/stat -f '%Sm' "$marker" 2>/dev/null || /usr/bin/stat -c '%y' "$marker")
  printf '%s\t%s\t%s\t%s\n' "$mode" "$marker" "$(cat "$marker")" "$when"
done
[ "$found" -eq 1 ] || printf 'no markers in %s\n' "$dir"
