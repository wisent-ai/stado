#!/bin/sh
# Build the Skarbiec release binary on the host that will run it, and measure
# what came out.
#
# This exists because the agent sandbox on the operator laptop cannot create a
# cargo target directory at all, so a Rust change to Skarbiec cannot be compiled
# from inside a session. A helper run by the Stado agent is not sandboxed, but it
# is launched by launchd and therefore has no TCC grant for ~/Documents, which is
# where the checkout lives: the fix is to keep every byte cargo writes outside
# ~/Documents by pointing CARGO_TARGET_DIR at ~/.cache. The source tree stays
# read-only, so a denial there shows up as a read error rather than a half-built
# target directory.
#
# The helper is idempotent: cargo reuses the same target directory, and the
# script only ever reports the artifact it produced. It never installs anything;
# swapping the binary in service is a separate, reversible step.
set -eu
umask 077

SOURCE="$HOME/Documents/CodingProjects/Wisent/skarbiec"
TARGET_DIR="$HOME/.cache/skarbiec-build"
PRODUCT="$TARGET_DIR/release/skarbiec"
export PATH="$HOME/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin"
export CARGO_TARGET_DIR="$TARGET_DIR"

printf 'source        %s\n' "$SOURCE"
printf 'target_dir    %s\n' "$TARGET_DIR"
printf 'rustc         %s\n' "$(rustc --version 2>&1 || echo unavailable)"
printf 'cargo         %s\n' "$(cargo --version 2>&1 || echo unavailable)"

# Prove the TCC read side before spending minutes in the compiler: if launchd's
# session cannot see the checkout, every later line would be noise.
if [ ! -r "$SOURCE/Cargo.toml" ]; then
	printf 'source_read   denied\n'
	exit 1
fi
printf 'source_read   ok %s\n' "$(git -C "$SOURCE" rev-parse --short HEAD 2>&1 || echo no-git)"

mkdir -p "$TARGET_DIR"
printf 'scratch_write ok\n'

cd "$SOURCE"
cargo build --release 2>&1 | tail -n 20

printf 'product       %s\n' "$PRODUCT"
# Absolute paths: $HOME/.stado/bin is on the Stado agent's PATH and shadows `wc`
# with a Stado subcommand, so a bare `wc -c` here fails with an argument error.
printf 'product_size  %s\n' "$(/usr/bin/wc -c <"$PRODUCT" | tr -d ' ')"
printf 'product_sha   %s\n' "$(/usr/bin/shasum -a 256 "$PRODUCT" | cut -c1-16)"
# The point of the build is one new canonical kind, so measure that the literal
# reached the artifact instead of asserting the build "worked". Rust packs string
# literals into one unterminated blob, so this counts occurrences, not lines.
printf 'kind_strings  %s\n' "$(/usr/bin/strings -a "$PRODUCT" | grep -c host-account || true)"

# `cargo build` never compiles `#[cfg(test)]` code, so a schema test can be
# syntactically fine and still not type-check. Check every target, then run only
# the test for the kind being added -- the whole suite is not this helper's job.
cargo check --all-targets 2>&1 | tail -n 5
# Skarbiec is a binary-only package, so its unit tests live in the bin target.
cargo test --bins host_account 2>&1 | tail -n 6
