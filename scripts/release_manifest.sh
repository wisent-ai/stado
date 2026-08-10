#!/bin/sh
# Emit the canonical immutable archive manifest shared by Stado products.
set -eu

die() {
    printf 'FATAL: %s\n' "$*" > /dev/stderr
    false
}

RELEASE_DIR="${STADO_RELEASE_DIR:?set STADO_RELEASE_DIR}"
ARCHIVE="${STADO_RELEASE_ARCHIVE:?set STADO_RELEASE_ARCHIVE}"
VERSION="${STADO_RELEASE_VERSION:?set STADO_RELEASE_VERSION}"
PLATFORM="${STADO_RELEASE_PLATFORM:?set STADO_RELEASE_PLATFORM}"
SOURCE_COMMIT="${STADO_RELEASE_SOURCE_COMMIT:?set STADO_RELEASE_SOURCE_COMMIT}"

for command_name in jq openssl; do
    command -v "$command_name" >/dev/null || die "required command is unavailable: $command_name"
done
[ -s "$ARCHIVE" ] || die "release archive is missing or empty: $ARCHIVE"
case "$VERSION:$PLATFORM" in
    *[![:alnum:]._-]* | *::* | :* | *:) die "invalid immutable release coordinate" ;;
esac
case "$SOURCE_COMMIT" in
    *[!0-9a-fA-F]* | "") die "source commit must be hexadecimal" ;;
esac

sha256="$(openssl dgst -sha256 "$ARCHIVE" | sed 's/^.*= //')"
manifest="$RELEASE_DIR/release-manifest-$PLATFORM.json"
jq -cS -n \
    --arg product stado \
    --arg version "$VERSION" \
    --arg platform "$PLATFORM" \
    --arg source_commit "$SOURCE_COMMIT" \
    --arg sha256 "$sha256" \
    '{
        "platform": $platform,
        "product": $product,
        "sha256": $sha256,
        "source_commit": $source_commit,
        "version": $version
    }' > "$manifest"
printf '%s\n' "$manifest"
