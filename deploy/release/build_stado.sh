#!/usr/bin/env bash
# Build in the per-product, per-platform CARGO_TARGET_DIR supplied by Stado.
# The recipe's stage map is relative to WISENT_OUTPUT_DIR, so copy the finished
# binary there without moving the reusable compiler cache into disposable
# output. The staged member remains stado-rs/target/release/stado.
set -euo pipefail

source_dir=${WISENT_SOURCE_DIR:?WISENT_SOURCE_DIR is required}
output_dir=${WISENT_OUTPUT_DIR:?WISENT_OUTPUT_DIR is required}

if ! command -v cargo >/dev/null; then
  PATH="$HOME/.cargo/bin:$PATH"
  export PATH
fi
command -v cargo >/dev/null || { printf 'cargo is not installed for this builder\n' >&2; exit 69; }

# Where cargo will put the binary. With no cache handed down, cargo's own
# default applies: a `target` directory beside the manifest.
target_dir=${CARGO_TARGET_DIR:-"$source_dir/stado-rs/target"}

cargo build \
  --manifest-path "$source_dir/stado-rs/Cargo.toml" \
  --locked \
  --release \
  --bin stado

staged="$output_dir/stado-rs/target/release"
mkdir -p "$staged"
install -m 0755 "$target_dir/release/stado" "$staged/stado"
