#!/bin/sh
# The product catalogue calls this build. Installation belongs to wisent-products.
set -eu
ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
REPO=$(CDPATH= cd -- "$ROOT/../.." && pwd)
BUILD_DIR=${STADO_BUILD_DIR:-"$ROOT/.build"}
BUNDLE="$BUILD_DIR/Stado.app"
unsigned=false
case "${1:-}" in
    --unsigned-bundle) unsigned=true ;;
    '') ;;
    *) printf '%s\n' 'usage: build-app.sh [--unsigned-bundle]' >&2; exit 2 ;;
esac
[ "$#" -le 1 ] || { printf '%s\n' 'usage: build-app.sh [--unsigned-bundle]' >&2; exit 2; }

swift build --package-path "$ROOT" --configuration release --product Stado --scratch-path "$BUILD_DIR"
EXECUTABLE="$BUILD_DIR/release/Stado"
[ -x "$EXECUTABLE" ] || { printf 'build did not produce %s\n' "$EXECUTABLE" >&2; exit 1; }
VERSION=$(sed -n 's/^version = "\([^"]*\)"$/\1/p' "$REPO/stado-rs/Cargo.toml")
[ -n "$VERSION" ] || { printf '%s\n' 'Cargo.toml does not declare the Stado version' >&2; exit 1; }

# Replace only this build's bundle, never the installed application.
rm -rf "$BUNDLE"
mkdir -p "$BUNDLE/Contents/MacOS" "$BUNDLE/Contents/Resources" "$BUNDLE/Contents/Helpers"
cp "$ROOT/Resources/Info.plist" "$BUNDLE/Contents/Info.plist"
cp "$EXECUTABLE" "$BUNDLE/Contents/MacOS/Stado"
/usr/libexec/PlistBuddy -c "Set :CFBundleShortVersionString $VERSION" "$BUNDLE/Contents/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleVersion $VERSION" "$BUNDLE/Contents/Info.plist"
# The product and its dependencies resolve packaged bundles from resourceURL.
for resource in "$BUILD_DIR"/release/*.bundle; do
    [ -d "$resource" ] || continue
    ditto "$resource" "$BUNDLE/Contents/Resources/$(basename "$resource")"
done

# Build the canonical identity helper from the pinned Swift dependency.
HELPER="$BUNDLE/Contents/Helpers/WisentIdentityKeychainHelper"
sh "$BUILD_DIR/checkouts/wisent-desktop-auth/scripts/build-keychain-helper.sh" "$HELPER"

# The icon is derived from the checked-in product artwork on every build.
ICONSET="$BUILD_DIR/Stado.iconset"
mkdir -p "$ICONSET"
sips -s format png "$ROOT/Resources/AppIcon.svg" --out "$BUILD_DIR/Stado-icon.png" >/dev/null
for size in 16 32 128 256 512; do
    sips -z "$size" "$size" "$BUILD_DIR/Stado-icon.png" --out "$ICONSET/icon_${size}x${size}.png" >/dev/null
    double=$((size * 2))
    sips -z "$double" "$double" "$BUILD_DIR/Stado-icon.png" --out "$ICONSET/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$BUNDLE/Contents/Resources/AppIcon.icns"

if [ "$unsigned" = true ]; then
    printf 'staged unsigned bundle: %s\n' "$BUNDLE"
    exit 0
fi
IDENTITY=${STADO_SIGN_IDENTITY:-${WISENT_CODESIGN_IDENTITY:-}}
if [ -z "$IDENTITY" ]; then
    IDENTITY=$(security find-identity -v -p codesigning | sed -n 's/.*"\(Developer ID Application:[^"]*\)".*/\1/p' | sed -n '1p')
fi
if [ -z "$IDENTITY" ]; then
    IDENTITY=$(security find-identity -v -p codesigning | sed -n 's/.*"\(Apple Development:[^"]*\)".*/\1/p' | sed -n '1p')
fi
if [ -z "$IDENTITY" ] || [ "$IDENTITY" = '-' ]; then
    printf '%s\n' 'A stable Developer ID Application or Apple Development signing identity is required.' >&2
    exit 1
fi
case "$IDENTITY" in
    'Developer ID Application:'*) set -- --options runtime --timestamp ;;
    *) set -- --timestamp=none ;;
esac
codesign --force --sign "$IDENTITY" "$@" --identifier ai.wisent.identity.keychain-helper "$HELPER"
codesign --force --sign "$IDENTITY" "$@" "$BUNDLE/Contents/MacOS/Stado"
codesign --force --sign "$IDENTITY" "$@" "$BUNDLE"
codesign --verify --strict --deep "$BUNDLE"
printf 'built signed bundle: %s\n' "$BUNDLE"
