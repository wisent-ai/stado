#!/bin/sh
# stado-up: install (or remove) a persistent local stado agent via launchd.
# Usage: stado-up <target-name> [uninstall]
set -eu

TARGET="${1:?"usage: stado-up <target-name> [uninstall]"}"
LABEL="com.stado.agent.${TARGET}"
PLIST="${HOME}/Library/LaunchAgents/${LABEL}.plist"
WC_PYTHON="$(command -v python3.12 || command -v python3)"
STADO_BIN="${HOME}/.stado/bin/stado"
LOG_DIR="${HOME}/.stado/logs"

if [ "${2:-}" = "uninstall" ]; then
    launchctl bootout "gui/$(id -u)/${LABEL}" 2>/dev/null || true
    launchctl bootout "user/$(id -u)/${LABEL}" 2>/dev/null || true
    rm -f "$PLIST"
    echo "removed ${LABEL}"
    exit 0
fi

BIN_DIR="${HOME}/.stado/bin"
mkdir -p "$BIN_DIR"
VERSION="$(curl -fsSL https://storage.googleapis.com/wisent-compute/releases/stado/latest.json \
    | "$WC_PYTHON" -c 'import json,sys; sys.stdout.write(json.load(sys.stdin)["version"])')"
BASE="https://storage.googleapis.com/wisent-compute/releases/stado/${VERSION}/darwin-arm64"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
curl -fsSL "${BASE}/stado" -o "${TMP}/stado"
curl -fsSL "${BASE}/SHA256SUMS" -o "${TMP}/SHA256SUMS"
EXPECTED="$(grep -E '[ *]stado$' "${TMP}/SHA256SUMS")"
EXPECTED="${EXPECTED%% *}"
ACTUAL="$(openssl dgst -sha256 "${TMP}/stado")"
ACTUAL="${ACTUAL##* }"
[ "$ACTUAL" = "$EXPECTED" ]
chmod +x "${TMP}/stado"
mv "${TMP}/stado" "$STADO_BIN"

mkdir -p "$LOG_DIR"
cat > "$PLIST" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>${LABEL}</string>
    <key>ProgramArguments</key>
    <array>
        <string>${STADO_BIN}</string>
        <string>agent</string>
        <string>--target</string>
        <string>${TARGET}</string>
    </array>
    <key>WorkingDirectory</key>
    <string>${HOME}</string>
    <key>EnvironmentVariables</key>
    <dict>
        <key>GCP_PROJECT</key>
        <string>wisent-480400</string>
        <key>GOOGLE_CLOUD_PROJECT</key>
        <string>wisent-480400</string>
        <key>WC_PYTHON</key>
        <string>${WC_PYTHON}</string>
        <key>PATH</key>
        <string>/usr/local/bin:/usr/bin:/bin:/opt/homebrew/bin</string>
    </dict>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>${LOG_DIR}/${LABEL}.out.log</string>
    <key>StandardErrorPath</key>
    <string>${LOG_DIR}/${LABEL}.err.log</string>
</dict>
</plist>
EOF

launchctl bootout "gui/$(id -u)/${LABEL}" 2>/dev/null || true
launchctl bootout "user/$(id -u)/${LABEL}" 2>/dev/null || true
if launchctl print "gui/$(id -u)" >/dev/null 2>&1; then
    launchctl bootstrap "gui/$(id -u)" "$PLIST"
    echo "installed ${LABEL} into gui domain (logs: ${LOG_DIR})"
else
    # Headless box reached over SSH: no Aqua domain for this user, so
    # bootstrap into the per-user domain (persists while any session lives;
    # ~/Library/LaunchAgents reloads into the gui domain at console login).
    launchctl bootstrap "user/$(id -u)" "$PLIST"
    echo "installed ${LABEL} into user domain (headless; logs: ${LOG_DIR})"
fi
