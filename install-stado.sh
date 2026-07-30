#!/bin/sh
# Install one exact immutable Stado release after verifying its manifest and
# every binary digest. This script never resolves a mutable latest release.
set -eu

die() {
    printf 'FATAL: %s\n' "$*" > /dev/stderr
    false
}

API_URL="${STADO_RELEASE_API_URL:?set STADO_RELEASE_API_URL}"
VERSION="${STADO_RELEASE_VERSION:?set STADO_RELEASE_VERSION}"
PLATFORM="${STADO_RELEASE_PLATFORM:?set STADO_RELEASE_PLATFORM}"
BIN_DIR="${STADO_BIN_DIR:-$HOME/.stado/bin}"

case "$API_URL" in
    https://*) ;;
    *) die "STADO_RELEASE_API_URL must use HTTPS" ;;
esac
case "$VERSION" in
    *[![:alnum:]._-]*|'') die "invalid STADO_RELEASE_VERSION" ;;
esac
case "$PLATFORM" in
    *[![:alnum:]._-]*|'') die "invalid STADO_RELEASE_PLATFORM" ;;
esac

for command_name in curl jq openssl mktemp; do
    command -v "$command_name" >/dev/null || die "required command is unavailable: $command_name"
done

work_dir="$(mktemp -d "${TMPDIR:-/tmp}/stado-install.XXXXXX")"
trap 'rm -rf "$work_dir"' EXIT HUP INT TERM

release_uri="stado://releases/stado/$VERSION/$PLATFORM"
download() {
    object_name="$1"
    destination="$2"
    curl --fail --silent --show-error --location --get \
        --data-urlencode "uri=$release_uri/$object_name" \
        "$API_URL/api/release/object" \
        --output "$destination"
}

download release-manifest.json "$work_dir/release-manifest.json"
download SHA256SUMS "$work_dir/SHA256SUMS"

jq -e \
    --arg version "$VERSION" \
    --arg platform "$PLATFORM" \
    '.product == "stado" and .version == $version and .platform == $platform and (.source_commit | type == "string") and (.artifacts | type == "array")' \
    "$work_dir/release-manifest.json" >/dev/null

jq -r '.artifacts[].name' "$work_dir/release-manifest.json" |
while IFS= read -r name; do
    case "$name" in
        stado|wc|stado-coverage|stado-fix|stado-watchdog|stado-mcp) ;;
        *) die "release manifest contains an unexpected binary: $name" ;;
    esac
    download "$name" "$work_dir/$name"
    expected="$(jq -r --arg name "$name" '.artifacts[] | select(.name == $name) | .sha256' "$work_dir/release-manifest.json")"
    actual="$(openssl dgst -sha256 "$work_dir/$name" | sed 's/^.*= //')"
    [ -n "$expected" ] || die "manifest digest is missing for $name"
    [ "$expected" = "$actual" ] || die "manifest digest verification failed for $name"
    grep -Fx "$expected  $name" "$work_dir/SHA256SUMS" >/dev/null || die "checksum list verification failed for $name"
done

mkdir -p "$BIN_DIR"
install_dir="$(mktemp -d "$BIN_DIR/.install.XXXXXX")"
trap 'rm -rf "$work_dir" "$install_dir"' EXIT HUP INT TERM

jq -r '.artifacts[].name' "$work_dir/release-manifest.json" |
while IFS= read -r name; do
    cp "$work_dir/$name" "$install_dir/$name"
    chmod a+x "$install_dir/$name"
done
cp "$work_dir/release-manifest.json" "$install_dir/release-manifest.json"
cp "$work_dir/SHA256SUMS" "$install_dir/SHA256SUMS"

jq -r '.artifacts[].name' "$work_dir/release-manifest.json" |
while IFS= read -r name; do
    if [ -e "$BIN_DIR/$name" ]; then
        cp "$BIN_DIR/$name" "$BIN_DIR/$name.previous"
    fi
    mv "$install_dir/$name" "$BIN_DIR/$name"
done
mv "$install_dir/release-manifest.json" "$BIN_DIR/release-manifest.json"
mv "$install_dir/SHA256SUMS" "$BIN_DIR/SHA256SUMS"
rmdir "$install_dir"

printf 'installed Stado %s for %s in %s\n' "$VERSION" "$PLATFORM" "$BIN_DIR"
