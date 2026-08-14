#!/bin/sh
# Report top-level temporary storage consumers without reading file contents.
set -eu

[ "$(uname -s)" = Linux ] || {
  printf '%s\n' "temporary storage report requires Linux" >&2
  exit 1
}

/bin/df -h /tmp
/usr/bin/du -x -k --max-depth=1 /tmp 2>/dev/null | /usr/bin/sort -n
