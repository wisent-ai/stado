#!/usr/bin/env bash
# Install a minimal Rust toolchain for the fleet's Linux builder.
#
# `release submit` refuses to publish a product declaring `linux-amd64` while no
# fleet builder broadcasts that platform verified. This host is the fleet's only
# Linux machine, and it carries git but no `cargo`, so the platform cannot be
# built anywhere -- a cross build from macOS fails in `ring`, which needs a C
# cross compiler, and an emulated container build exceeded an hour.
#
# Idempotent: an existing toolchain is reported and left alone. Undo by removing
# ~/.rustup and ~/.cargo.
set -euo pipefail

export PATH="$HOME/.cargo/bin:$PATH"

if command -v cargo >/dev/null; then
  printf 'cargo present %s\n' "$(cargo --version)"
  printf 'rustc present %s\n' "$(rustc --version)"
  exit 0
fi

if ! command -v curl >/dev/null; then
  printf 'curl unavailable; cannot fetch the toolchain installer\n' >&2
  exit 1
fi

# `--profile minimal` keeps this to what a release build needs: no docs, no
# clippy, no rustfmt on a machine that only compiles.
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
  | sh -s -- -y --profile minimal --no-modify-path >/dev/null

if ! command -v cargo >/dev/null; then
  printf 'toolchain install completed but cargo is still absent\n' >&2
  exit 1
fi
printf 'cargo installed %s\n' "$(cargo --version)"
printf 'rustc installed %s\n' "$(rustc --version)"
printf 'target %s\n' "$(rustc -vV | /usr/bin/awk '/^host:/ {print $2}')"
