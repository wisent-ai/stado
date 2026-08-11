#!/bin/sh
# Move this Linux host's Stado binary onto one exact repository revision.
#
# Run through `stado host install-helper` + `run-helper`, which passes no
# arguments, so the revision is pinned here and changes with a commit. Idempotent:
# an already-installed revision reports and exits without building.
#
# The beacon publisher and the fleet's readers must agree on where a host beacon
# is written. A host older than that agreement publishes where nobody reads, so
# `stado host ping` calls it stale while ssh and the box are perfectly healthy.
set -eu

REVISION=293fa59695f07010b0c9f5a9285edc847082d93e
REPOSITORY=https://github.com/wisent-ai/stado.git
WORK="$HOME/.stado/build-work/stado"
TARGET="$HOME/.stado/bin/stado"
STAMP="$HOME/.stado/bin/stado.revision"

PATH="$HOME/.cargo/bin:/usr/local/cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"
export PATH

if [ -f "$STAMP" ] && [ "$(cat "$STAMP")" = "$REVISION" ]; then
  printf 'already at %s\n' "$REVISION"
  exit 0
fi

cargo_bin=$(command -v cargo || true)
if [ -z "$cargo_bin" ]; then
  for candidate in "$HOME"/.rustup/toolchains/*/bin/cargo /usr/local/rustup/toolchains/*/bin/cargo; do
    [ -x "$candidate" ] && cargo_bin="$candidate" && break
  done
fi
[ -n "$cargo_bin" ] || { printf 'no cargo toolchain on this host\n'; exit 1; }
printf 'cargo: %s\n' "$cargo_bin"

mkdir -p "$(dirname "$WORK")"
if [ ! -d "$WORK/.git" ]; then
  rm -rf "$WORK"
  git clone --filter=blob:none --no-checkout "$REPOSITORY" "$WORK"
fi
git -C "$WORK" fetch --depth 1 origin "$REVISION"
git -C "$WORK" checkout --detach --force "$REVISION"

CARGO_TARGET_DIR="$WORK/target" "$cargo_bin" build --locked --release \
  --manifest-path "$WORK/stado-rs/Cargo.toml" --bin stado

built="$WORK/target/release/stado"
[ -x "$built" ] || { printf 'build produced no stado binary\n'; exit 1; }

if [ -f "$TARGET" ]; then
  backup="$TARGET.before-$REVISION"
  [ -f "$backup" ] || cp -p "$TARGET" "$backup"
  printf 'backup: %s (%s)\n' "$backup" "$("$backup" --version 2>/dev/null || echo unknown)"
fi
install -m 0755 "$built" "$TARGET"
printf '%s\n' "$REVISION" >"$STAMP"
printf 'installed: %s\n' "$("$TARGET" --version 2>/dev/null || echo 'version unavailable')"
