#!/bin/zsh
# Build, sign, and install the Stado menu-bar app.
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT"

PRODUCT="Stado"
BUNDLE="$ROOT/.build/Stado.app"
INSTALLED_BUNDLE="${STADO_INSTALL_APP_PATH:-$HOME/Applications/Stado.app}"
EXECUTABLE="$ROOT/.build/release/$PRODUCT"
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
swift build -c release --product "$PRODUCT"

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

APP_PROVISIONING_PROFILE=${WISENT_APP_PROVISIONING_PROFILE:-}
if [[ -n "$APP_PROVISIONING_PROFILE" ]]; then
    if [[ ! -f "$APP_PROVISIONING_PROFILE" ]]; then
        print -u2 "App provisioning profile not found: $APP_PROVISIONING_PROFILE"
        exit 1
    fi
    install -m 0644 "$APP_PROVISIONING_PROFILE" "$BUNDLE/Contents/embedded.provisionprofile"
    SIGN_ARGS+=(--entitlements "$ROOT/Resources/WisentDesktop.entitlements")
fi

print "→ signing with $IDENTITY"
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
