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

while IFS=$'\t' read -r target platform; do
  manifest="stado://releases/stado/$version/$platform/release-manifest-$platform.json"
  state="$($stado_bin storage stat "$manifest" --json | jq -r '.state // ""')"
  if [ "$state" != present ]; then
    echo "FATAL: $target needs $manifest, release channel state is ${state:-unknown}" >&2
    exit 1
  fi
  "$stado_bin" host declare-version "$target" --binary stado --version "$version" --json
  "$stado_bin" host release "$target" --binary stado --version "$version" --json
  "$stado_bin" service converge "$target" stado --json
  echo "$target: stado $version installed and in-sync"
done <<< "$targets"
