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
# dropped from the listing read here.
#
# What is asserted is presence at the writer-verified size, one declared object
# at a time, and NOT the size of the coordinate. One `product/version/platform`
# coordinate has two writers: this train publishes nine objects, and
# `stado release submit`'s signed pipeline publishes `release.json`,
# `release.sig`, `release.tar.gz` and `qualification.json` beside them.
# 0.13.45/linux-amd64 held all thirteen, so an equality against the nine this
# job built refused a coordinate that was whole — a check measuring the
# publisher's own directory rather than the property that matters.
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
