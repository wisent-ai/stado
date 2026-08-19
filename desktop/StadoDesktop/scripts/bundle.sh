#!/bin/zsh
# Build, sign, and install the Stado menu-bar app.
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT"

PRODUCT="Stado"
# macOS protects ~/Documents from processes without Full Disk Access, and this
# package lives there, so a build started by an automation agent or a job dies
# writing its own `.build`. The checkout stays put and only the build output
# moves when STADO_BUILD_DIR says so.
BUILD_DIR="${STADO_BUILD_DIR:-$ROOT/.build}"
BUNDLE="$BUILD_DIR/Stado.app"
INSTALLED_BUNDLE="${STADO_INSTALL_APP_PATH:-$HOME/Applications/Stado.app}"
EXECUTABLE="$BUILD_DIR/release/$PRODUCT"
LSREGISTER=/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister

unregister_bundle() {
    if output=$("$LSREGISTER" -u "$1" 2>&1); then
        return 0
    fi
    case "$output" in
        *-10814*) return 0 ;;
    esac
    print -u2 "$output"
    return 1
}

print "→ building release"
swift build -c release --product "$PRODUCT" --scratch-path "$BUILD_DIR"

if [[ ! -x "$EXECUTABLE" ]]; then
    print -u2 "build did not produce $EXECUTABLE"
    exit 1
fi

print "→ assembling $BUNDLE"
rm -rf "$BUNDLE"
mkdir -p "$BUNDLE/Contents/MacOS" "$BUNDLE/Contents/Resources"
cp "$ROOT/Resources/Info.plist" "$BUNDLE/Contents/Info.plist"
cp "$EXECUTABLE" "$BUNDLE/Contents/MacOS/Stado"
chmod +x "$BUNDLE/Contents/MacOS/Stado"

sh "$ROOT/scripts/import-brand-icon.sh" stado-desktop "$BUNDLE/Contents/Resources/AppIcon.icns"
IDENTITY="${STADO_SIGN_IDENTITY:-${WISENT_CODESIGN_IDENTITY:-}}"
if [[ -z "$IDENTITY" ]]; then
    IDENTITY=$(security find-identity -v -p codesigning \
        | awk -F '"' '/Developer ID Application/{print $2; exit}')
fi
if [[ -z "$IDENTITY" ]]; then
    IDENTITY=$(security find-identity -v -p codesigning \
        | awk -F '"' '/Apple Development:/{print $2; exit}')
fi
if [[ -z "$IDENTITY" || "$IDENTITY" == "-" ]]; then
    print -u2 "A stable Developer ID Application or Apple Development signing identity is required."
    print -u2 "Set STADO_SIGN_IDENTITY or WISENT_CODESIGN_IDENTITY; refusing ad-hoc signing."
    exit 1
fi

SIGN_ARGS=(--force --sign "$IDENTITY")
if [[ "$IDENTITY" == Developer\ ID\ Application:* ]]; then
    SIGN_ARGS+=(--options runtime --timestamp)
else
    SIGN_ARGS+=(--timestamp=none)
fi

IDENTITY_HELPER="$BUNDLE/Contents/Helpers/WisentIdentityKeychainHelper"
# The checkouts live in the scratch path, which STADO_BUILD_DIR moves; a
# hardcoded $ROOT/.build worked only in checkouts that had once been built
# in place, and failed in every fresh worktree.
"$BUILD_DIR/checkouts/wisent-desktop-auth/scripts/build-keychain-helper.sh" "$IDENTITY_HELPER"

print "→ signing with $IDENTITY"
codesign "${SIGN_ARGS[@]}" --identifier ai.wisent.identity.keychain-helper "$IDENTITY_HELPER"
codesign "${SIGN_ARGS[@]}" "$BUNDLE/Contents/MacOS/Stado"
codesign "${SIGN_ARGS[@]}" "$BUNDLE"
codesign --verify --strict --deep --verbose=2 "$BUNDLE"

print "→ installing $INSTALLED_BUNDLE"
rm -rf "$INSTALLED_BUNDLE"
mkdir -p "$(dirname "$INSTALLED_BUNDLE")"
ditto "$BUNDLE" "$INSTALLED_BUNDLE"
codesign --verify --strict --deep --verbose=2 "$INSTALLED_BUNDLE"
unregister_bundle "$BUNDLE"
"$LSREGISTER" -f "$INSTALLED_BUNDLE"
print "✓ $INSTALLED_BUNDLE"

RESTART_APP=${WISENT_RESTART_APP:-"$ROOT/scripts/wisent-restart-app"}
if [[ "${WISENT_RESTART_AFTER_BUILD:-1}" != 0 && -x "$RESTART_APP" ]]; then
    "$RESTART_APP" --if-running "$INSTALLED_BUNDLE"
fi

if [[ "${1:-}" == "--open" ]]; then
    open "$INSTALLED_BUNDLE"
fi
