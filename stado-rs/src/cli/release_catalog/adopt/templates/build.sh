#!/usr/bin/env bash
# Written by `stado release catalog adopt --kind ios-xcode`: archive, sign and
# export {{PROJECT}}.xcodeproj's {{SCHEME}} scheme as the release artifact.
set -euo pipefail

: "${WISENT_VERSION:?Stado must provide WISENT_VERSION}"
: "${WISENT_SOURCE_DIR:?Stado must provide WISENT_SOURCE_DIR}"
: "${WISENT_OUTPUT_DIR:?Stado must provide WISENT_OUTPUT_DIR}"
: "${WISENT_PLATFORM:?Stado must provide WISENT_PLATFORM}"
: "${IOS_DIST_P12_B64:?Skarbiec must provide the iOS distribution certificate}"
: "${IOS_DIST_P12_PASSWORD:?Skarbiec must provide the certificate password}"
: "${IOS_SIGN_IDENTITY:?Skarbiec must provide the signing identity}"
: "${IOS_PROFILE_B64:?Skarbiec must provide {{PRODUCT}}-signing#provisioning_profile_base64}"
[[ "$WISENT_PLATFORM" == "ios-arm64" ]]

team_id="${APPLE_TEAM_ID:-{{TEAM}}}"
work="$WISENT_OUTPUT_DIR/work"
dist="$WISENT_OUTPUT_DIR/dist"
archive="$work/{{APP}}.xcarchive"
exported="$work/export"
keychain="$work/{{PRODUCT}}-release.keychain-db"
keychain_password="$(uuidgen)"
profile_dir="$HOME/Library/MobileDevice/Provisioning Profiles"
profile="$profile_dir/stado-{{PRODUCT}}-${WISENT_VERSION}.mobileprovision"
mkdir -p "$work" "$dist" "$profile_dir"

cleanup() {
  security delete-keychain "$keychain" >/dev/null 2>&1 || true
  rm -f "$profile" "$work/distribution.p12"
  rm -rf "$work"
}
trap cleanup EXIT

printf '%s' "$IOS_DIST_P12_B64" | base64 --decode > "$work/distribution.p12"
printf '%s' "$IOS_PROFILE_B64" | base64 --decode > "$profile"
security create-keychain -p "$keychain_password" "$keychain"
security set-keychain-settings -lut 21600 "$keychain"
security unlock-keychain -p "$keychain_password" "$keychain"
security import "$work/distribution.p12" -k "$keychain" -P "$IOS_DIST_P12_PASSWORD" -T /usr/bin/codesign -T /usr/bin/security -A
security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "$keychain_password" "$keychain" >/dev/null
security cms -D -i "$profile" > "$work/profile.plist"
profile_uuid="$(/usr/libexec/PlistBuddy -c 'Print :UUID' "$work/profile.plist")"

# A marketing version of one to three numbers becomes the build number
# major*1000000 + minor*1000 + patch; missing parts count as zero.
numeric_version="${WISENT_VERSION%%-*}"
IFS=. read -r major minor patch <<< "$numeric_version"
minor="${minor:-0}"
patch="${patch:-0}"
[[ "$major" =~ ^[0-9]+$ && "$minor" =~ ^[0-9]+$ && "$patch" =~ ^[0-9]+$ ]]
build_number=$((10#$major * 1000000 + 10#$minor * 1000 + 10#$patch))

xcodebuild -project "$WISENT_SOURCE_DIR/{{PROJECT}}.xcodeproj" -scheme "{{SCHEME}}" \
  -configuration Release -destination 'generic/platform=iOS' \
  -archivePath "$archive" \
  DEVELOPMENT_TEAM="$team_id" \
  CODE_SIGN_STYLE=Manual \
  CODE_SIGN_IDENTITY="$IOS_SIGN_IDENTITY" \
  PROVISIONING_PROFILE="$profile_uuid" \
  MARKETING_VERSION="$WISENT_VERSION" \
  CURRENT_PROJECT_VERSION="$build_number" \
  OTHER_CODE_SIGN_FLAGS="--keychain $keychain" \
  archive

cat > "$work/ExportOptions.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>method</key><string>app-store-connect</string>
  <key>teamID</key><string>${team_id}</string>
  <key>destination</key><string>export</string>
  <key>signingStyle</key><string>manual</string>
  <key>signingCertificate</key><string>${IOS_SIGN_IDENTITY}</string>
  <key>provisioningProfiles</key><dict><key>{{BUNDLE_ID}}</key><string>${profile_uuid}</string></dict>
</dict></plist>
PLIST

xcodebuild -exportArchive \
  -archivePath "$archive" \
  -exportOptionsPlist "$work/ExportOptions.plist" \
  -exportPath "$exported" \
  OTHER_CODE_SIGN_FLAGS="--keychain $keychain"

ipa=("$exported"/*.ipa)
[[ ${#ipa[@]} -eq 1 ]]
cp "${ipa[0]}" "$dist/{{APP}}.ipa"
python3 "$WISENT_SOURCE_DIR/release/archive-tree.py" "$archive" "$dist/{{APP}}.xcarchive.tar.gz"
git -C "$WISENT_SOURCE_DIR" rev-parse HEAD > "$dist/SOURCE_REVISION"
ipa_sha="$(shasum -a 256 "$dist/{{APP}}.ipa" | cut -d ' ' -f 1)"
archive_sha="$(shasum -a 256 "$dist/{{APP}}.xcarchive.tar.gz" | cut -d ' ' -f 1)"
printf '{"build_number":%s,"ipa_sha256":"%s","schema_version":1,"version":"%s","xcarchive_sha256":"%s"}\n' \
  "$build_number" "$ipa_sha" "$WISENT_VERSION" "$archive_sha" > "$dist/build-evidence.json"
