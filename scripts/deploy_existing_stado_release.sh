#!/usr/bin/env bash
# Deliver one already-published Stado version to every registered supported host.
set -euo pipefail

version="${1:?usage: deploy_existing_stado_release.sh VERSION}"
stado_bin="${STADO_BIN:-$HOME/.stado/bin/stado}"
case "$version" in
  *[![:alnum:]._-]*|"") echo "FATAL: invalid Stado version: $version" >&2; exit 2 ;;
esac
[ -x "$stado_bin" ] || { echo "FATAL: Stado binary is not executable: $stado_bin" >&2; exit 2; }
command -v jq >/dev/null || { echo "FATAL: jq is required" >&2; exit 2; }

registry="$($stado_bin registry pull)"
targets="$({ printf '%s' "$registry" | jq -r '
  .targets[] |
  select(.release_platform == "darwin-arm64" or .release_platform == "linux-amd64") |
  [.name, .release_platform] | @tsv
'; })"
[ -n "$targets" ] || { echo "FATAL: registry declares no supported release targets" >&2; exit 1; }

work_root="${STADO_RELEASE_WORK_DIR:-$HOME/.stado/work/releases/$version}"
mkdir -p "$work_root"

ensure_host_archive() {
  local platform="$1"
  local source_uri="stado://releases/stado/$version/$platform/release.tar.gz"
  local archive_uri="stado://releases/stado/$version/$platform/stado-v$version-$platform.tar.gz"
  local archive="$work_root/$platform.tar.gz"
  local state
  state="$($stado_bin storage stat "$archive_uri" --json | jq -r '.state // ""')"
  if [ ! -s "$archive" ]; then
    rm -f "$archive"
    if [ "$state" = present ]; then
      "$stado_bin" storage get "$archive_uri" "$archive"
    elif [ "$state" = absent ]; then
      "$stado_bin" storage get "$source_uri" "$archive"
    else
      echo "FATAL: canonical archive state is ${state:-unknown}: $archive_uri" >&2
      exit 1
    fi
  fi
  if [ "$state" = absent ]; then
    env -u STADO_API_URL "$stado_bin" storage put "$archive_uri" "$archive" --if-absent
  fi
}


self_target="$($stado_bin registry self --name-only)"
while IFS=$'\t' read -r target platform; do
  manifest="stado://releases/stado/$version/$platform/release-manifest-$platform.json"
  state="$($stado_bin storage stat "$manifest" --json | jq -r '.state // ""')"
  if [ "$state" != present ]; then
    echo "FATAL: $target needs $manifest, release channel state is ${state:-unknown}" >&2
    exit 1
  fi
  ensure_host_archive "$platform"
  "$stado_bin" host declare-version "$target" --binary stado --version "$version" --json
  if [ "$target" = "$self_target" ]; then
    "$stado_bin" host install-release \
      "$target" "$work_root/$platform.tar.gz" stado "$version" \
      --platform "$platform" --json
    manifest_file="$work_root/$platform-manifest.json"
    rm -f "$manifest_file"
    "$stado_bin" storage get "$manifest" "$manifest_file"
    sha256="$(jq -er .sha256 "$manifest_file")"
    WISENT_RELEASE_ARCHIVE="$work_root/$platform.tar.gz" \
    WISENT_RELEASE_SHA256="$sha256" \
      "$stado_bin" release install-local --member stado --name stado
  else
    "$stado_bin" host release "$target" --binary stado --version "$version" --json
  fi
  "$stado_bin" service converge "$target" stado --json
  echo "$target: stado $version installed and in-sync"
done <<< "$targets"
