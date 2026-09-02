#!/usr/bin/env bash
set -euo pipefail

# Make a second attempt at one release coordinate possible.
#
# Release objects are create-only. The archive this train publishes is produced
# by `stado storage archive`, and tar+gzip output is not byte-reproducible
# across runs — mtimes, member order and the gzip header all move — while the
# executables inside it carry `env!("CARGO_MANIFEST_DIR")`, which is a fresh
# `$RUNNER_TEMP/stado-source.XXXX` on every checkout. So a rebuilt attempt at an
# already-published version produces DIFFERENT bytes for the same names, and the
# writer answers exactly that:
#
#   Error: immutable object already differs on the writer:
#   stado://releases/stado/0.13.47/darwin-arm64/stado-v0.13.47-darwin-arm64.tar.gz
#
# 0.13.47 died there. Attempt 1 published all thirteen objects of its darwin
# coordinate; attempt 2 rebuilt the archive, refused it twelve times, failed the
# job, and `deploy-fleet` was skipped — a release whose bytes were complete and
# public never reached a single host, and no later attempt at that tag could
# have ended differently.
#
# The way out is to stop rebuilding what is already published. If the archive is
# there, it IS the release: this adopts it and derives the whole release
# directory from it, so every subsequent `storage put --if-absent` either
# matches the published object exactly or creates the one that is missing. That
# also completes a half-published coordinate instead of leaving it stranded,
# which is the state `stado/0.11.0/darwin-arm64` has been in since April.
#
# Called with STADO_API_URL pointing at the authenticated writer, the same
# origin the publish loop uses.

if [ "$#" -ne 5 ]; then
  echo "usage: adopt_published_release.sh STADO_BIN RELEASE_DIR PRODUCT VERSION PLATFORM" >&2
  exit 2
fi

stado_bin="$1"
release_dir="$2"
product="$3"
version="$4"
platform="$5"

case "$product" in
  *[![:alnum:]_-]*|"") echo "invalid release product: $product" >&2; exit 2 ;;
esac
case "$version" in
  *[![:alnum:]._-]*|"") echo "invalid release version: $version" >&2; exit 2 ;;
esac
case "$platform" in
  *[![:alnum:]_-]*|"") echo "invalid release platform: $platform" >&2; exit 2 ;;
esac

test -x "$stado_bin"
test -d "$release_dir"

prefix="stado://releases/$product/$version/$platform"
archive_name="$product-v$version-$platform.tar.gz"
manifest_name="release-manifest-$platform.json"

published_state() {
  "$stado_bin" storage stat "$prefix/$1" --json | jq -er '.state'
}

# `storage stat` answers present or absent with exit status zero and reserves
# non-zero for a store that did not answer. Guessing "absent" from a store that
# is merely unreachable would rebuild over a live coordinate, so an unusable
# answer stops this instead.
archive_state="$(published_state "$archive_name")"
case "$archive_state" in
  absent)
    echo "no published $prefix/$archive_name: this attempt publishes its own build"
    exit 0
    ;;
  present) ;;
  *)
    echo "::error::the writer could not say whether $prefix/$archive_name exists" \
      "(state $archive_state); this attempt will not rebuild over a coordinate it cannot read" >&2
    exit 1
    ;;
esac

echo "adopting the published $prefix/$archive_name; this attempt publishes no rebuilt bytes"
"$stado_bin" storage get "$prefix/$archive_name" "$release_dir/$archive_name"
# The archive carries every executable plus SHA256SUMS, so extracting it over
# the release directory replaces this attempt's build with the bytes the
# coordinate already holds — which is what the publish loop, the public
# validation and the delivery all have to agree about.
/usr/bin/tar -xzf "$release_dir/$archive_name" -C "$release_dir"

# The manifest is regenerated deterministically from the adopted archive by the
# caller's own manifest step (product, version, platform, source_commit and the
# archive digest, and nothing else), so a published one must still match. Taking
# the published copy when it exists means the comparison is against the bytes a
# consumer gets rather than against a second derivation of them.
manifest_state="$(published_state "$manifest_name")"
case "$manifest_state" in
  present) "$stado_bin" storage get "$prefix/$manifest_name" "$release_dir/$manifest_name" ;;
  absent)
    echo "$prefix/$manifest_name is absent; the adopted archive's manifest will be published"
    ;;
  *)
    echo "::error::the writer could not say whether $prefix/$manifest_name exists" \
      "(state $manifest_state)" >&2
    exit 1
    ;;
esac
