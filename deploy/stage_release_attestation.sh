#!/usr/bin/env bash
# Leave the receipt the fleet's provenance check reads, for a delivery that
# installs from an already-built release directory.
#
# `stado host release` stages every binary it delivers at
# `$HOME/.stado/releases/<binary>/<version>/<platform>/<binary>`, and
# `cli::service_converge::attest_installed` decides provenance by comparing the
# installed file against exactly that path. `self_update::stage_for_attestation`
# does the same for the self-update path, after that path spent months
# installing verified bytes and throwing the evidence away.
#
# The control-plane delivery in `deploy_stado_rust.sh` was the third path and it
# staged nothing: on 2026-09-02 the release train published 0.13.46 on both
# platforms, delivered it here, and then `deploy-fleet` refused the run with
# `unattested` — "the host runs 0.13.46 and no delivered copy of 0.13.46 is
# staged at $HOME/.stado/releases" — for bytes this same train had verified
# against the canonical manifest twice.
#
# Kept as its own script so both the delivery path and an operator repairing one
# host run the same code, and so staging can be performed without re-running the
# coordinator bootstrap that `deploy_stado_rust.sh` ends with.
set -euo pipefail

if [ "$#" -lt 1 ] || [ "$#" -gt 2 ]; then
  echo "usage: stage_release_attestation.sh RELEASE_DIR [PLATFORM]" >&2
  exit 2
fi

release_dir="$1"
platform="${2:-${STADO_RELEASE_PLATFORM:-}}"

if [ ! -d "$release_dir" ]; then
  echo "release directory $release_dir does not exist" >&2
  exit 1
fi
if [ -z "${HOME:-}" ]; then
  echo "HOME is unset, so the attestation copy has nowhere to go" >&2
  exit 1
fi

# The platform is whatever manifest the release directory carries, when the
# caller did not name one. Two manifests mean two platforms in one directory,
# which is a directory nothing here can attribute.
if [ -z "$platform" ]; then
  for manifest in "$release_dir"/release-manifest-*.json; do
    [ -f "$manifest" ] || continue
    name="${manifest##*/}"
    name="${name#release-manifest-}"
    candidate="${name%.json}"
    if [ -n "$platform" ]; then
      echo "$release_dir carries manifests for more than one platform; name the platform" >&2
      exit 1
    fi
    platform="$candidate"
  done
fi
if [ -z "$platform" ]; then
  echo "$release_dir carries no release manifest, so the platform is unresolved" >&2
  exit 1
fi

manifest="$release_dir/release-manifest-$platform.json"
if [ ! -f "$manifest" ]; then
  echo "$manifest is absent, so the version these bytes claim is unresolved" >&2
  exit 1
fi

# Read with `tr` and `sed` rather than `jq`: this runs on every host a release
# reaches, and a receipt that depends on a tool the host may not carry is a
# receipt that silently does not get written.
version="$(tr ',{}' '\n' < "$manifest" | sed -n 's/^ *"version" *: *"\([^"]*\)".*/\1/p' | head -n 1)"
manifest_platform="$(tr ',{}' '\n' < "$manifest" | sed -n 's/^ *"platform" *: *"\([^"]*\)".*/\1/p' | head -n 1)"
case "$version" in
  ""|*[![:alnum:]._-]*)
    echo "$manifest declares no usable version" >&2
    exit 1 ;;
esac
if [ -n "$manifest_platform" ] && [ "$manifest_platform" != "$platform" ]; then
  echo "$manifest declares platform $manifest_platform, not $platform" >&2
  exit 1
fi

staged=0
for name in stado stado-coverage stado-fix stado-watchdog stado-mcp; do
  source="$release_dir/$name"
  [ -f "$source" ] || continue
  coordinate="$HOME/.stado/releases/$name/$version/$platform"
  mkdir -p "$coordinate"
  # Dot-prefixed then renamed, so a reader never attests against a partial copy.
  temporary="$coordinate/.$name.staging"
  cp "$source" "$temporary"
  chmod 0755 "$temporary"
  mv "$temporary" "$coordinate/$name"
  echo "staged $name $version for $platform at $coordinate/$name"
  staged=$((staged + 1))
done

if [ "$staged" -lt 1 ]; then
  echo "$release_dir carries none of the binaries this delivery stages" >&2
  exit 1
fi
