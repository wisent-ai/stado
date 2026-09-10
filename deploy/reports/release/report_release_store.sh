#!/usr/bin/env bash
# Report every object-store path the API's service account cannot write.
#
# An authorized immutable release write returned `500 Permission denied
# (os error 13)` from this host: the policy accepted the publisher, and then the
# store refused the file. That is a property of this filesystem, not of the
# request. Guessing which directory it was cost one wrong repair, so this reports
# the whole store rather than a hand-picked list of namespaces.
set -euo pipefail

config="${STADO_CONFIG:-$HOME/.config/stado/config.json}"
root=$(
  /usr/bin/python3 - "$config" <<'PY' 2>/dev/null || true
import json, os, sys
try:
    with open(sys.argv[1], encoding="utf-8") as handle:
        data = json.load(handle)
except OSError:
    print("")
    raise SystemExit
path = ((data.get("storage") or {}).get("local") or {}).get("path") or ""
print(os.path.expanduser(path))
PY
)

account=$(id -un)
printf 'service_user %s\n' "$account"
if [ -z "$root" ] || [ ! -d "$root" ]; then
  printf 'store_root unresolved\n'
  exit 0
fi
printf 'store_root %s\n' "$root"

# Directories are what a create needs to be writable; files matter only if the
# API rewrites them, which the release namespaces never do.
foreign=$(/usr/bin/find "$root" -type d ! -user "$account" 2>/dev/null || true)
if [ -z "$foreign" ]; then
  printf 'foreign_directories 0\n'
else
  printf 'foreign_directories %s\n' "$(printf '%s\n' "$foreign" | /usr/bin/wc -l | /usr/bin/tr -d ' ')"
  printf '%s\n' "$foreign" | while IFS= read -r directory; do
    printf '  %s %s\n' "$(/usr/bin/stat -f '%Su:%Sg %Sp' "$directory")" "${directory#"$root"/}"
  done
fi

if /usr/bin/sudo -n /usr/bin/true 2>/dev/null; then
  printf 'passwordless_sudo yes\n'
else
  printf 'passwordless_sudo no\n'
fi
