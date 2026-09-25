#!/usr/bin/env bash
# The build step of Stado's own release recipe.
#
# It exists so the compile can use the builder's cache while the recipe's
# stage map keeps naming a path inside the job's output directory.
#
# Stado hands every build job a per-product, per-platform CARGO_TARGET_DIR
# (stado-rs/src/cli/release_submit/builds/worker/environment.rs), so that the
# next release of a product recompiles only what its commit changed. The
# recipe used to override that with `--target-dir .wisent-output/...`, a path
# inside the scratch tree the job throws away, because the stage map is
# resolved against WISENT_OUTPUT_DIR and a cache path outside it cannot be
# named there. So every Stado release recompiled its whole dependency graph —
# about 520 packages — while the three quality steps beside it, which name no
# target directory, were already reusing the cache.
#
# The compile writes wherever the builder's cache is, and the one binary the
# recipe stages is copied into the output tree afterwards. `stage` therefore
# still reads `stado-rs/target/release/stado`.
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

# Stado must build under the already installed worker, whose recipe parser
# predates the `tests` key. Keep post-build qualification in this build entry
# instead of making the bootstrap depend on the version it is building.
# Source CLI checks run in `quality`; these flows consume the staged binary
# and retain the same evidence required by the qualification entrypoint.
export WISENT_TEST_EVIDENCE_DIR="$output_dir/test-evidence"

# The release journey starts a real Skarbiec broker, and nothing guaranteed
# a builder had one: Skarbiec publishes no linux-amd64 release, so every
# Linux build since the journey was added failed with `no real skarbiec
# binary`. The manifest pins a Skarbiec source as the `skarbiec` input and
# the worker extracts it to WISENT_INPUT_SKARBIEC_DIR; it is compiled here,
# into its own directory of the builder's cache, and handed to the tests as
# SKARBIEC_BIN — the path the tests themselves name.
skarbiec_source=${WISENT_INPUT_SKARBIEC_DIR:?WISENT_INPUT_SKARBIEC_DIR is required}
skarbiec_target="$target_dir/skarbiec-input"
printf '[qualification] skarbiec broker\n'
CARGO_TARGET_DIR="$skarbiec_target" cargo build \
  --manifest-path "$skarbiec_source/Cargo.toml" \
  --locked --release --bin skarbiec
export SKARBIEC_BIN="$skarbiec_target/release/skarbiec"
for journey in product product_registry product_sources product_releases; do
  printf '[qualification] %s\n' "$journey"
  cargo test \
    --manifest-path "$source_dir/stado-rs/Cargo.toml" \
    --locked --release --test "$journey" -- --nocapture
done
printf '[qualification] release-pipeline\n'
cargo test \
  --manifest-path "$source_dir/stado-rs/Cargo.toml" \
  --locked --release --test ci-cd \
  a_real_release_builds_publishes_and_installs_its_binary \
  -- --ignored --exact --nocapture
bash "$source_dir/tests/fleet-expansion/qualify.sh" cli
case "${WISENT_PLATFORM:?WISENT_PLATFORM is required}" in
  darwin-arm64)
    bash "$source_dir/tests/fleet-expansion/qualify.sh" desktop
    # Apple signing needs a Darwin host and the fleet's certificate item.
    printf '[qualification] product_signing\n'
    cargo test \
      --manifest-path "$source_dir/stado-rs/Cargo.toml" \
      --locked --release --test product_signing -- --nocapture
    ;;
esac
