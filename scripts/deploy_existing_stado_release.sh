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
  local need_local="${2:-no}"
  local source_uri="stado://releases/stado/$version/$platform/release.tar.gz"
  local archive_uri="stado://releases/stado/$version/$platform/stado-v$version-$platform.tar.gz"
  local archive="$work_root/$platform.tar.gz"
  local state
  state="$($stado_bin storage stat "$archive_uri" --json | jq -r '.state // ""')"
  if [ "$state" = present ] && [ "$need_local" != yes ]; then
    return
  fi
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
tag_commit="$(git rev-parse "stado-v$version^{commit}")"
while IFS=$'\t' read -r target platform; do
  manifest="stado://releases/stado/$version/$platform/release-manifest-$platform.json"
  state="$($stado_bin storage stat "$manifest" --json | jq -r '.state // ""')"
  if [ "$state" != present ]; then
    echo "FATAL: $target needs $manifest, release channel state is ${state:-unknown}" >&2
    exit 1
  fi
  manifest_file="$work_root/release-manifest-$platform.json"
  "$stado_bin" storage get "$manifest" "$manifest_file"
  expected_sha256="$(jq -er \
    --arg version "$version" \
    --arg platform "$platform" \
    --arg source_commit "$tag_commit" \
    'if (keys | sort) == ["platform", "product", "sha256", "source_commit", "version"]
        and .product == "stado"
        and .version == $version
        and .platform == $platform
        and .source_commit == $source_commit
        and (.sha256 | type == "string" and test("^[0-9a-f]{64}$"))
     then .sha256
     else error("release manifest identity mismatch")
     end' "$manifest_file")"
  "$stado_bin" host declare-version "$target" --binary stado --version "$version" --json
  if [ "$target" = "$self_target" ]; then
    ensure_host_archive "$platform" yes
    installed="$("$HOME/.stado/bin/stado" --version 2>/dev/null || true)"
    installed_version="${installed#stado }"
    installed_version="${installed_version%% *}"
    if [ "$installed_version" != "$version" ]; then
      WISENT_RELEASE_ARCHIVE="$work_root/$platform.tar.gz" \
        WISENT_RELEASE_SHA256="$expected_sha256" \
        WISENT_PRODUCT=stado WISENT_VERSION="$version" \
        env -u STADO_API_TOKEN "$stado_bin" release install-local \
          --member stado --name stado
    fi
    installed="$("$HOME/.stado/bin/stado" --version)"
    installed_version="${installed#stado }"
    installed_version="${installed_version%% *}"
    [ "$installed_version" = "$version" ] || {
      echo "FATAL: $target reports $installed after local Stado release" >&2
      exit 1
    }
    "$HOME/.stado/bin/stado" release converge-local-readers --name stado
  else
    ensure_host_archive "$platform" no
    "$stado_bin" host release "$target" --binary stado --version "$version" --json
  fi
  readers="$(printf '%s' "$registry" | jq -r --arg target "$target" '
    [.targets[] | select(.name == $target) | .services[]?
      | select((.program? | type) == "string")
      | select((.program | contains("/.stado/services/")) and (.program | endswith("/stado")))
      | .name]
    | if length == (unique | length) then .[]?
      else error("declared service-local Stado reader names must be unique")
      end
  ')"
  if [ -n "$readers" ]; then
    ensure_host_archive "$platform" yes
    archive="$work_root/$platform.tar.gz"
    actual_sha256="$(openssl dgst -sha256 "$archive")"
    [ "${actual_sha256##* }" = "$expected_sha256" ] || {
      echo "FATAL: $target private-reader archive digest mismatch" >&2
      exit 1
    }
    while IFS= read -r reader; do
      "$stado_bin" service update "$reader" --host "$target" \
        --from-archive "$archive" --refresh-image --json
    done <<< "$readers"
  fi
  "$stado_bin" service converge "$target" stado --apply --json
  echo "$target: stado $version installed and in-sync"
done <<< "$targets"
