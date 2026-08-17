#!/bin/sh
# Report whether the always-on Mac holds the Wisent Backend media namespace.
# Install and run through `stado host install-helper` + `run-helper`; it reads
# only fixed Stado storage paths and emits counts, never object contents.
set -eu

for store in \
  /Users/charles/.stado/local-storage \
  /Users/charles/.stado/local-backup
do
  root="$store/ecosystem/wisent-backend/images/characters"
  if [ ! -d "$root" ]; then
    printf '%s\tpresent=no\n' "$root"
    continue
  fi

  count=$(/usr/bin/find "$root" -type f -print | /usr/bin/wc -l | /usr/bin/tr -d ' ')
  kib=$(/usr/bin/du -sk "$root" | /usr/bin/cut -f1)
  sample=$(/usr/bin/find "$root" -type f -name '8808.webp' -print -quit)
  if [ -n "$sample" ]; then
    sample_present=yes
  else
    sample_present=no
  fi
  printf '%s\tpresent=yes\tfiles=%s\tkib=%s\tsample_8808=%s\n' \
    "$root" "$count" "$kib" "$sample_present"
done
