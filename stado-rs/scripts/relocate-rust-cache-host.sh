#!/bin/sh
set -eu
cache_root=/mnt/wd16tb/wisent-cache
[ -d /mnt/wd16tb ] || { printf '%s\n' "missing /mnt/wd16tb" >&2; exit 1; }
mkdir -p "$cache_root"
for name in .cargo .rustup
do
  source_path="$HOME/$name"
  target_path="$cache_root/$name"
  if [ -L "$source_path" ]; then
    [ "$(readlink "$source_path")" = "$target_path" ] || {
      printf '%s\n' "$source_path points outside $cache_root" >&2
      exit 1
    }
    continue
  fi
  [ -e "$target_path" ] && {
    printf '%s\n' "$target_path already exists without the canonical link" >&2
    exit 1
  }
  [ -d "$source_path" ] || mkdir -p "$source_path"
  mv "$source_path" "$target_path"
  ln -s "$target_path" "$source_path"
done
df -Pk / /mnt/wd16tb
