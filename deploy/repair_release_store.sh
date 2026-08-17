#!/usr/bin/env bash
# Give the object API's service account every store directory it must write.
#
# `ecosystem/system` and its `.metadata` sidecar were left owned by root while
# every sibling namespace belonged to the service account, so an authorized
# immutable release write reached the store and failed with `Permission denied
# (os error 13)`. The authorization was never the problem, which is why fixing
# the 401 alone published nothing.
#
# Repairing one hand-picked directory left the sidecar behind and the write still
# failed, so this takes the whole store: every directory not owned by the service
# account. Only ownership changes, and only for directories. Undo by chowning the
# printed paths back to their printed previous owner.
set -euo pipefail

config="${STADO_CONFIG:-$HOME/.config/stado/config.json}"
root=$(
  /usr/bin/python3 - "$config" <<'PY'
import json, os, sys
with open(sys.argv[1], encoding="utf-8") as handle:
    data = json.load(handle)
path = ((data.get("storage") or {}).get("local") or {}).get("path") or ""
print(os.path.expanduser(path))
PY
)
if [ -z "$root" ] || [ ! -d "$root" ]; then
  printf 'store_root unresolved; nothing repaired\n' >&2
  exit 1
fi

account=$(id -un)
group=$(id -gn)
repaired=0
while IFS= read -r directory; do
  [ -n "$directory" ] || continue
  before=$(/usr/bin/stat -f '%Su:%Sg' "$directory")
  /usr/bin/sudo -n /usr/sbin/chown "$account:$group" "$directory"
  printf 'repaired %s: %s -> %s:%s\n' "${directory#"$root"/}" "$before" "$account" "$group"
  repaired=$((repaired + 1))
done <<EOF
$(/usr/bin/find "$root" -type d ! -user "$account" 2>/dev/null || true)
EOF

if [ "$repaired" -eq 0 ]; then
  printf 'every store directory already belongs to %s\n' "$account"
  exit 0
fi

# Prove the end state rather than trusting the loop: a second sweep must find
# nothing, which is the same check the report helper performs.
remaining=$(/usr/bin/find "$root" -type d ! -user "$account" 2>/dev/null | /usr/bin/wc -l | /usr/bin/tr -d ' ')
printf 'repaired_directories %s remaining %s\n' "$repaired" "$remaining"
[ "$remaining" -eq 0 ]
