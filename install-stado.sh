#!/bin/sh
# Install one exact immutable Stado archive after verifying its canonical
# manifest. This script never resolves a mutable release.
set -eu

die() {
    printf 'FATAL: %s\n' "$*" > /dev/stderr
    false
}

API_URL="${STADO_API_URL:?set STADO_API_URL}"
VERSION="${STADO_RELEASE_VERSION:?set STADO_RELEASE_VERSION}"
PLATFORM="${STADO_RELEASE_PLATFORM:?set STADO_RELEASE_PLATFORM}"
BIN_DIR="${STADO_BIN_DIR:-$HOME/.stado/bin}"

case "$API_URL" in
    https://*) ;;
    *) die "STADO_API_URL must use HTTPS" ;;
esac
case "$VERSION" in
    *[![:alnum:]._-]*|'') die "invalid STADO_RELEASE_VERSION" ;;
esac
case "$PLATFORM" in
    *[![:alnum:]._-]*|'') die "invalid STADO_RELEASE_PLATFORM" ;;
esac

for command_name in curl jq openssl mktemp tar; do
    command -v "$command_name" >/dev/null || die "required command is unavailable: $command_name"
done

work_dir="$(mktemp -d "${TMPDIR:-/tmp}/stado-install.XXXXXX")"
trap 'rm -rf "$work_dir"' EXIT HUP INT TERM
release_uri="stado://releases/stado/$VERSION/$PLATFORM"
archive_name="stado-v$VERSION-$PLATFORM.tar.gz"
manifest_name="release-manifest-$PLATFORM.json"
download() {
    curl --fail --silent --show-error --location --get \
        --data-urlencode "uri=$release_uri/$1" \
        "$API_URL/api/release/object" \
        --output "$2"
}

download "$manifest_name" "$work_dir/$manifest_name"
download "$archive_name" "$work_dir/$archive_name"
jq -e \
    --arg version "$VERSION" \
    --arg platform "$PLATFORM" \
    'keys == ["platform","product","sha256","source_commit","version"] and
     .product == "stado" and .version == $version and .platform == $platform and
     (.source_commit | test("^([0-9A-Fa-f]{40}|[0-9A-Fa-f]{64})$")) and
     (.sha256 | test("^[0-9a-f]{64}$"))' \
    "$work_dir/$manifest_name" >/dev/null
expected="$(jq -r .sha256 "$work_dir/$manifest_name")"
actual="$(openssl dgst -sha256 "$work_dir/$archive_name" | sed 's/^.*= //')"
[ "$expected" = "$actual" ] || die "release archive digest verification failed"

members=""
while IFS= read -r name; do
    case "$name" in
        stado|wc|stado-coverage|stado-fix|stado-watchdog|stado-mcp|LICENSE) ;;
        *) die "release archive contains an unexpected member: $name" ;;
    esac
    case " $members " in
        *" $name "*) die "release archive duplicates member: $name" ;;
    esac
    members="$members $name"
done <<EOF
$(tar -tzf "$work_dir/$archive_name")
EOF
for name in stado wc stado-coverage stado-fix stado-watchdog stado-mcp; do
    case " $members " in
        *" $name "*) ;;
        *) die "release archive is missing binary: $name" ;;
    esac
done

mkdir -p "$BIN_DIR"
install_dir="$(mktemp -d "$BIN_DIR/.install.XXXXXX")"
trap 'rm -rf "$work_dir" "$install_dir"' EXIT HUP INT TERM
tar -xzf "$work_dir/$archive_name" -C "$install_dir"
for name in stado wc stado-coverage stado-fix stado-watchdog stado-mcp; do
    [ -f "$install_dir/$name" ] && [ ! -L "$install_dir/$name" ] ||
        die "release binary is not a regular file: $name"
    chmod a+x "$install_dir/$name"
    if [ -e "$BIN_DIR/$name" ]; then
        cp "$BIN_DIR/$name" "$BIN_DIR/$name.previous"
    fi
    mv "$install_dir/$name" "$BIN_DIR/$name"
done
mv "$work_dir/$manifest_name" "$BIN_DIR/release-manifest.json"
rm -f "$install_dir/LICENSE"
rmdir "$install_dir"

printf 'installed Stado %s for %s in %s\n' "$VERSION" "$PLATFORM" "$BIN_DIR"
