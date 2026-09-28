#!/usr/bin/env bash
# Written by `stado release catalog adopt --kind cargo`: the post-build test a
# fleet builder runs is {{PRODUCT}}'s own test suite against the built tree,
# including tests marked ignored because they reach real services.
set -euo pipefail

if ! command -v cargo >/dev/null; then
  PATH="$HOME/.cargo/bin:$PATH"
  export PATH
fi
command -v cargo >/dev/null || {
  printf 'cargo is not installed for this builder\n' >&2
  exit 69
}
exec cargo test --locked --release --manifest-path "${WISENT_SOURCE_DIR:?WISENT_SOURCE_DIR is required}/Cargo.toml" -- --include-ignored
