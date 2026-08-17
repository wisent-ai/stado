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

# The release quality gate runs `cargo fmt` and `cargo clippy`, so a toolchain
# that merely compiles is not enough: brama 0.2.13 built on this host and then
# failed the gate with "'cargo-fmt' is not installed for the toolchain
# 'stable-aarch64-apple-darwin'". Idempotent -- rustup reports and skips a
# component it already has.
ensure_components() {
  if ! command -v rustup >/dev/null; then
    printf 'rustup unavailable; cannot verify gate components\n' >&2
    return 0
  fi
  for component in rustfmt clippy; do
    if rustup component list --installed 2>/dev/null | grep -q "^${component}"; then
      printf 'component present %s\n' "$component"
      continue
    fi
    if rustup component add "$component" >/dev/null 2>&1; then
      printf 'component installed %s\n' "$component"
    else
      printf 'component FAILED %s\n' "$component" >&2
    fi
  done
}

if command -v cargo >/dev/null; then
  printf 'cargo present %s\n' "$(cargo --version)"
  printf 'rustc present %s\n' "$(rustc --version)"
  ensure_components
  exit 0
fi

if ! command -v curl >/dev/null; then
  printf 'curl unavailable; cannot fetch the toolchain installer\n' >&2
  exit 1
fi

# `--profile minimal` keeps the download to what compiling needs; the components
# the release quality gate runs are added explicitly below.
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
  | sh -s -- -y --profile minimal --no-modify-path >/dev/null

if ! command -v cargo >/dev/null; then
  printf 'toolchain install completed but cargo is still absent\n' >&2
  exit 1
fi
printf 'cargo installed %s\n' "$(cargo --version)"
printf 'rustc installed %s\n' "$(rustc --version)"
printf 'target %s\n' "$(rustc -vV | /usr/bin/awk '/^host:/ {print $2}')"
ensure_components
