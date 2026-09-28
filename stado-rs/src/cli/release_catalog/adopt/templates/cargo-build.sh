#!/usr/bin/env bash
# Written by `stado release catalog adopt --kind cargo`: builds every binary
# {{PRODUCT}} ships from the locked tree and stages each with its digest.
set -euo pipefail

source_dir=${WISENT_SOURCE_DIR:?WISENT_SOURCE_DIR is required}
output_dir=${WISENT_OUTPUT_DIR:?WISENT_OUTPUT_DIR is required}
platform=${WISENT_PLATFORM:?WISENT_PLATFORM is required}
version=${WISENT_VERSION:?WISENT_VERSION is required}
: "${CARGO_TARGET_DIR:?The builder-owned CARGO_TARGET_DIR is required}"
binaries=({{BINARIES}})
# A fleet builder's job environment carries a minimal PATH; rustup installs
# cargo under ~/.cargo/bin.
if ! command -v cargo >/dev/null; then
  PATH="$HOME/.cargo/bin:$PATH"
  export PATH
fi
command -v cargo >/dev/null || {
  printf 'cargo is not installed for this builder\n' >&2
  exit 69
}

case "$platform" in
  darwin-arm64) expected_os=Darwin; expected_arch=arm64 ;;
  linux-amd64) expected_os=Linux; expected_arch=x86_64 ;;
  *) printf 'unsupported release platform: %s\n' "$platform" >&2; exit 1 ;;
esac
if [[ "$(uname -s)" != "$expected_os" || "$(uname -m)" != "$expected_arch" ]]; then
  printf 'this builder cannot produce %s\n' "$platform" >&2
  exit 1
fi

declared=$(cargo metadata --no-deps --format-version 1 --manifest-path "$source_dir/Cargo.toml" \
  | jq -er '.packages[] | select(.name == "{{PRODUCT}}") | .version')
if [[ "$declared" != "$version" ]]; then
  printf 'Cargo.toml declares %s but the release is %s\n' "$declared" "$version" >&2
  exit 1
fi

mkdir -p "$output_dir/bin" "$output_dir/evidence"
cargo build --locked --release --bins --manifest-path "$source_dir/Cargo.toml" \
  --message-format=json-render-diagnostics | tee "$output_dir/evidence/build.jsonl"
for name in "${binaries[@]}"; do
  binary=$(jq -er --arg name "$name" \
    'select(.reason == "compiler-artifact" and .target.name == $name and .executable != null) | .executable' \
    "$output_dir/evidence/build.jsonl")
  install -m 0755 "$binary" "$output_dir/bin/$name"
done
(
  cd "$output_dir"
  shasum -a 256 bin/* > evidence/DIGESTS
)
