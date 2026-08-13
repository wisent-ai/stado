#!/bin/sh
set -eu

PRODUCT=${1:?"Usage: import-brand-icon.sh PRODUCT OUTPUT.icns"}
OUTPUT=${2:?"Usage: import-brand-icon.sh PRODUCT OUTPUT.icns"}

if [ "$PRODUCT" != "stado-desktop" ]; then
    printf 'Unsupported product: %s\n' "$PRODUCT" >&2
    exit 64
fi

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
SOURCE_ASSET=${WISENT_STADO_APP_ICON_FILE:-"$SCRIPT_DIR/../Resources/AppIcon.svg"}

for tool in sips iconutil; do
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

printf 'Importing canonical app icon: %s\n' "$SOURCE_ASSET"
cp "$SOURCE_ASSET" "$SOURCE"
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
