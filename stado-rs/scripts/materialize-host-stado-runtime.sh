#!/bin/sh
set -eu
umask u=rwx,g=,o=

release_marker=${STADO_RUNTIME_RELEASE_MARKER:-$HOME/.stado/stado-runtime-version}
if [ ! -f "$release_marker" ]; then
  printf '%s\n' "missing Stado runtime release marker: $release_marker" >/dev/stderr
  false
fi
IFS= read -r release_version < "$release_marker"
case "$release_version" in
  .|..|*[![:alnum:]._-]*)
    printf '%s\n' 'invalid Stado runtime release marker' >/dev/stderr
    false
    ;;
esac
platform=${STADO_RUNTIME_PLATFORM:-linux-x86_64}
release_archive="$HOME/.stado/releases/stado-runtime/$release_version/$platform/stado-runtime.tar.gz"
release_root="$HOME/.stado/services/stado-runtime/releases/$release_version/$platform"

if [ ! -f "$release_archive" ]; then
  printf '%s\n' "missing Stado runtime release: $release_archive" >/dev/stderr
  false
fi
if [ ! -d "$release_root" ]; then
  staging="$release_root.staging.$$"
  mkdir -p "$(dirname "$release_root")" "$staging"
  trap 'rm -rf "$staging"' EXIT HUP INT TERM
  tar -C "$staging" -xzf "$release_archive"
  if [ ! -x "$staging/bin/stado" ]; then
    printf '%s\n' 'Stado runtime release is missing bin/stado' >/dev/stderr
    false
  fi
  mv "$staging" "$release_root"
  trap - EXIT HUP INT TERM
fi
printf '%s\n' "$release_root/bin/stado"
