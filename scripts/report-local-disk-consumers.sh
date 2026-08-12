#!/usr/bin/env bash
# Fixed-scope inventory for diagnosing local Stado admission disk pressure.
set -euo pipefail

report_children() {
  local root="$1"
  [ -d "$root" ] || return 0
  printf 'children\t%s\n' "$root"
  { /usr/bin/du -sk "$root"/* 2>/dev/null || true; } |
    /usr/bin/sort -nr |
    /usr/bin/sed -n '1,20p'
}

for path in \
  "$HOME/.cache" \
  "$HOME/.cargo" \
  "$HOME/.rustup" \
  "$HOME/.transcript-lake" \
  "$HOME/Library/Caches" \
  "$HOME/Library/Developer/CoreSimulator" \
  "$HOME/Library/Developer/Xcode/DerivedData" \
  "$HOME/Documents/CodingProjects/Wisent"
do
  if [ -e "$path" ]; then
    /usr/bin/du -sk "$path" 2>/dev/null || true
  fi
done

report_children "$HOME/.cache"
report_children "$HOME/Library/Caches"
report_children "$HOME/Library/Developer/CoreSimulator"
report_children "$HOME/Library/Developer/Xcode/DerivedData"
report_children "$HOME/Documents/CodingProjects/Wisent"
