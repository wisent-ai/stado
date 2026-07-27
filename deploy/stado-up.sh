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
# Release channel base URL -- same contract the Rust binary resolves via
# config::release_base_url(): <base>/latest.json, then
# <base>/<version>/<platform>/stado and a sibling SHA256SUMS. Defaults to
# the public GCS endpoint the release pipeline has always published to,
# so an unset environment installs exactly what it installed before.
#
# An Azure blob channel must be PRE-AUTHENTICATED: this installer runs on
# operator laptops that have no managed identity to mint a bearer token
# from, and shelling out to `az` is deliberately not a dependency of
# stado-up. Either make the container public-read, or append a container
# SAS to WC_RELEASE_BASE_URL -- the query string is split off here and
# re-appended after each object path, which is the only place a SAS works.
RELEASE_BASE="${WC_RELEASE_BASE_URL:-https://storage.googleapis.com/wisent-compute/releases/stado}"
RELEASE_QS=""
case "$RELEASE_BASE" in
    *\?*)
        RELEASE_QS="?${RELEASE_BASE#*\?}"
        RELEASE_BASE="${RELEASE_BASE%%\?*}"
        ;;
esac
RELEASE_BASE="${RELEASE_BASE%/}"
VERSION="$(curl -fsSL "${RELEASE_BASE}/latest.json${RELEASE_QS}" \
    | "$WC_PYTHON" -c 'import json,sys; sys.stdout.write(json.load(sys.stdin)["version"])')"
BASE="${RELEASE_BASE}/${VERSION}/darwin-arm64"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
curl -fsSL "${BASE}/stado${RELEASE_QS}" -o "${TMP}/stado"
curl -fsSL "${BASE}/SHA256SUMS${RELEASE_QS}" -o "${TMP}/SHA256SUMS"
EXPECTED="$(grep -E '[ *]stado$' "${TMP}/SHA256SUMS")"
EXPECTED="${EXPECTED%% *}"
ACTUAL="$(openssl dgst -sha256 "${TMP}/stado")"
ACTUAL="${ACTUAL##* }"
[ "$ACTUAL" = "$EXPECTED" ]
chmod +x "${TMP}/stado"
mv "${TMP}/stado" "$STADO_BIN"

mkdir -p "$LOG_DIR"
# Propagate a non-default release channel to the installed agent so its
# own self-update resolves the same base URL instead of the compiled-in
# GCS default. Unset -> nothing is emitted and the plist is byte-identical
# to before. The value is XML-escaped because a SAS query string carries
# ampersands.
RELEASE_ENV=""
if [ -n "${WC_RELEASE_BASE_URL:-}" ]; then
    RELEASE_XML="$(printf '%s' "$WC_RELEASE_BASE_URL" \
        | sed -e 's/&/\&amp;/g' -e 's/</\&lt;/g' -e 's/>/\&gt;/g')"
    RELEASE_ENV="        <key>WC_RELEASE_BASE_URL</key>
        <string>${RELEASE_XML}</string>
"
fi

# Propagate the deployment's storage/provider configuration into the agent's
# launchd environment. launchd gives a LaunchAgent none of the invoking
# shell's environment, so anything the agent needs must be materialised here
# or come from ~/.config/stado/config.json. Only variables that are actually
# set in the installing shell are emitted, so an unset environment produces a
# plist byte-identical to the one this script has always written.
#
# GCP_PROJECT / GOOGLE_CLOUD_PROJECT below stay unconditional: they are inert
# on an Azure deployment (read only by the GCP provider arm and the BigQuery
# billing collector) and rewriting them would churn every existing install.
CUTOVER_ENV=""
for _wc_key in WC_PROVIDERS WC_STORAGE_BACKEND WC_AZURE_STORAGE_ACCOUNT \
               WC_AZURE_CONTAINER WC_BUCKET WC_ALERTS_TOPIC \
               AZURE_SUBSCRIPTION_ID AZURE_RESOURCE_GROUP AZURE_LOCATIONS \
               AZURE_VM_IDENTITY_ID; do
    # eval is the POSIX-sh way to read a variable by name; the loop values are
    # literals from the list above, never user input.
    eval "_wc_val=\${${_wc_key}+set}"
    [ "${_wc_val:-}" = "set" ] || continue
    eval "_wc_val=\${${_wc_key}}"
    _wc_xml="$(printf '%s' "$_wc_val" \
        | sed -e 's/&/\&amp;/g' -e 's/</\&lt;/g' -e 's/>/\&gt;/g')"
    # WC_ALERTS_TOPIC is deliberately emitted even when empty: an empty topic
    # is what disables the dead GCP Pub/Sub alert channel, and dropping it
    # would let the non-empty compiled-in default win.
    CUTOVER_ENV="${CUTOVER_ENV}        <key>${_wc_key}</key>
        <string>${_wc_xml}</string>
"
done
unset _wc_key _wc_val _wc_xml

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
${RELEASE_ENV}${CUTOVER_ENV}    </dict>
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
