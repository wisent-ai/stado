#!/usr/bin/env bash
# Recover local Stado admission capacity from disposable build and model caches.
set -euo pipefail

before=$(/bin/df -k "$HOME" | /usr/bin/awk 'NR == 2 { print $4 }')
for path in \
  "$HOME/Library/Developer/Xcode/DerivedData" \
  "$HOME/.cache/openwhispr" \
  "$HOME/.cache/uv"
do
  if [ -d "$path" ]; then
    /bin/chmod -R u+w "$path" 2>/dev/null || true
    /bin/rm -rf "$path"
  fi
done
after=$(/bin/df -k "$HOME" | /usr/bin/awk 'NR == 2 { print $4 }')
printf 'free_kb_before=%s\nfree_kb_after=%s\nfreed_kb=%s\n' \
  "$before" "$after" "$((after - before))"
