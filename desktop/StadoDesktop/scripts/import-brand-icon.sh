#!/bin/sh
set -eu

PRODUCT=${1:?"Usage: import-brand-icon.sh PRODUCT OUTPUT.icns"}
OUTPUT=${2:?"Usage: import-brand-icon.sh PRODUCT OUTPUT.icns"}
API_BASE=${WISENT_GROUND_TRUTH_API:-}

if [ -z "$API_BASE" ]; then
    printf '%s\n' 'WISENT_GROUND_TRUTH_API must point to the canonical asset resolver.' >&2
    exit 64
fi

for tool in curl sips iconutil; do
    if ! command -v "$tool" >/dev/null 2>&1; then
        printf 'Required tool not found: %s\n' "$tool" >&2
        exit 69
    fi
done

WORK_DIR=$(mktemp -d "${TMPDIR:-/tmp}/wisent-app-icon.XXXXXX")
trap 'rm -rf "$WORK_DIR"' EXIT HUP INT TERM
SOURCE="$WORK_DIR/source.svg"
PNG="$WORK_DIR/source.png"
ICONSET="$WORK_DIR/AppIcon.iconset"
ASSET_URL="${API_BASE%/}/assets/$PRODUCT/app_icon/0/content"

printf 'Importing canonical app icon: %s\n' "$ASSET_URL"
curl --fail --silent --show-error --location "$ASSET_URL" --output "$SOURCE"
sips -s format png "$SOURCE" --out "$PNG" >/dev/null
mkdir -p "$ICONSET" "$(dirname "$OUTPUT")"

for spec in \
    '16:icon_16x16.png' \
    '32:icon_16x16@2x.png' \
    '32:icon_32x32.png' \
    '64:icon_32x32@2x.png' \
    '128:icon_128x128.png' \
    '256:icon_128x128@2x.png' \
    '256:icon_256x256.png' \
    '512:icon_256x256@2x.png' \
    '512:icon_512x512.png' \
    '1024:icon_512x512@2x.png'
do
    pixels=${spec%%:*}
    name=${spec#*:}
    sips -z "$pixels" "$pixels" "$PNG" --out "$ICONSET/$name" >/dev/null
done

iconutil -c icns "$ICONSET" -o "$OUTPUT"
printf 'Imported canonical app icon to %s\n' "$OUTPUT"
