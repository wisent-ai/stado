#!/usr/bin/env bash
set -euo pipefail

if [ "$#" -ne 6 ]; then
  echo "usage: verify_public_stado_release.sh STADO_BIN RELEASE_DIR PRODUCT VERSION PLATFORM SCRATCH_DIR" >&2
  exit 2
fi

stado_bin="$1"
release_dir="$2"
product="$3"
version="$4"
platform="$5"
scratch_dir="$6"

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
test -d "$scratch_dir"

prefix="stado://releases/$product/$version/$platform"
archive_name="$product-v$version-$platform.tar.gz"
manifest_name="release-manifest-$platform.json"
test -f "$release_dir/$archive_name"
test -f "$release_dir/$manifest_name"

# One coordinate, two producers. This workflow publishes the executables,
# SHA256SUMS, the platform archive and the platform manifest. The product's own
# signed-release path publishes `release.json`, `release.sig`, `release.tar.gz`
# and `qualification.json` into the SAME prefix — see
# `cli::release_submit::publish`, which records
# `stado://releases/<product>/<version>/<platform>/qualification.json` as the
# run's qualification URI. 0.13.45's linux leg is exactly that state: nine
# objects from here, four from there, and a check that expected the coordinate
# to hold only what this runner had on disk refused a complete release.
#
# Those names, plus `source-revision.json` — the claim every publisher writes
# create-only before its first artifact, so one version can only ever attest
# one build — are declared once, in `release_control.rs`, and read out of that
# declaration here so this check cannot drift from the writers that make them.
# `[A-Z_]*` and not `[A-Z]*`: a two-word constant like
# `RELEASE_REVISION_NAME`'s successors would otherwise be silently skipped and
# read as a member nobody declares.
control_source="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/stado-rs/src/release_control.rs"
test -f "$control_source"
control_names="$(
  sed -n 's/^pub const RELEASE_[A-Z_]*_NAME: &str = "\([^"]*\)";$/\1/p' "$control_source"
)"
declared_control="$(printf '%s\n' "$control_names" | sed '/^$/d' | /usr/bin/wc -l | tr -d '[:space:]')"
if [ "$declared_control" -lt 5 ]; then
  echo "$control_source declares $declared_control release-owned object name(s);" \
    "this check needs them to know which members the pipeline and the coordinate" \
    "claim write" >&2
  exit 1
fi

# Every immutable put already reads the exact bytes back through the authenticated
# writer. Public validation therefore has a different job: prove that the public
# route exposes that complete coordinate. The archive contains every executable
# plus SHA256SUMS, so one exact archive read proves their bytes through the public
# route. The listing proves that every standalone object is also visible with its
# writer-verified size. Re-downloading each executable repeated hundreds of MiB
# and exceeded the release runner's wall-clock budget without proving more.
#
# A chunked put stages its parts at `<key>.__stado_upload/<id>/<index>` under
# this same prefix, and the composition that finishes the object deletes them.
# An interrupted upload therefore leaves parts a later attempt cleans up:
# 0.13.44's first darwin attempt left four when the runner lost its connection
# mid-archive. Parts are upload state, never coordinate members, so they are
# dropped here — while any OTHER unexpected member still fails the coordinate.
listing="$(
  $stado_bin storage objects releases "$product/$version/$platform/" --json |
    jq -c '{objects: [.objects[] | select((.key // .uri) | contains(".__stado_upload/") | not)]}'
)"
for source in "$release_dir"/*; do
  name="${source##*/}"
  uri="$prefix/$name"
  size="$(/usr/bin/wc -c < "$source" | tr -d '[:space:]')"
  if ! jq -e --arg uri "$uri" --argjson size "$size" \
    '([.objects[] | select(.uri == $uri and .size == $size)] | length) == 1' \
    <<<"$listing" >/dev/null; then
    echo "public release coordinate omitted $uri at its writer-verified size $size" >&2
    exit 1
  fi
done

# Every member is now accounted for by one of the two producers, so anything
# left over is an object nobody declares — a stray write into an immutable
# coordinate, which is what this check is for.
declared="$(
  {
    printf '%s\n' "$control_names"
    for source in "$release_dir"/*; do printf '%s\n' "${source##*/}"; done
  } | jq -R -s 'split("\n") | map(select(length > 0))'
)"
undeclared="$(
  jq -r --argjson declared "$declared" '
    ([.objects[] | (.key // .uri) | sub(".*/"; "")] - $declared) | unique | join(" ")
  ' <<<"$listing"
)"
if [ -n "$undeclared" ]; then
  echo "public release coordinate carries member(s) neither producer declares: $undeclared" >&2
  exit 1
fi

for name in "$archive_name" "$manifest_name"; do
  source="$release_dir/$name"
  uri="$prefix/$name"
  delivered="$scratch_dir/public-$platform-$name"
  visible=false
  for attempt in $(seq 1 12); do
    rm -f "$delivered"
    if "$stado_bin" storage get "$uri" "$delivered" &&
       cmp --silent "$source" "$delivered"; then
      visible=true
      break
    fi
    sleep "$((attempt * 2))"
  done
  if [ "$visible" != true ]; then
    echo "public release delivery did not serve immutable $uri" >&2
    exit 1
  fi
done
