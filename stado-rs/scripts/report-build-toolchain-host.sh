#!/bin/sh
# Whether this host can build a Stado program itself.
#
# A fix that lives only in a checkout on the operator's laptop does not change
# what a Linux host runs, and macOS arm64 cannot produce its binary. The fleet
# already keeps `deploy/install_rust_toolchain.sh` for exactly this, so the
# question worth asking first is what is already here.
#
# Read-only.
set -eu

for tool in cargo rustc cc docker; do
  printf '%s\t' "$tool"
  if command -v "$tool" >/dev/null 2>&1; then
    "$tool" --version 2>&1 | head -n 1
  else
    printf 'absent\n'
  fi
done

printf 'cargo_home\t'
if [ -d /root/.cargo ]; then printf '%s\n' "$(ls -1 /root/.cargo 2>/dev/null | tr '\n' ' ')"; else printf 'absent\n'; fi

printf 'build_dirs\t'
for dir in /root/.stado/build /root/.stado/build-work /root/.cache/stado-build; do
  if [ -d "$dir" ]; then printf '%s(%s) ' "$dir" "$(du -sh "$dir" 2>/dev/null | cut -f1)"; fi
done
printf '\n'

printf 'installed_stado\t'
if [ -x /root/.stado/bin/stado ]; then /root/.stado/bin/stado --version 2>&1 | head -n 1; else printf 'absent\n'; fi

printf 'source_tarballs\t'
ls -1 /root/.stado/files 2>/dev/null | tr '\n' ' ' || true
printf '\n'
