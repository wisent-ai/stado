#!/usr/bin/env bash
# Written by `stado release catalog adopt --kind ios-xcode`. The quality gate:
# the project file parses and the app compiles for the simulator without
# signing, so a source change that breaks the build fails here, before any
# signing secret is used.
set -euo pipefail

: "${WISENT_SOURCE_DIR:?Stado must provide WISENT_SOURCE_DIR}"
: "${WISENT_PLATFORM:?Stado must provide WISENT_PLATFORM}"
: "${WISENT_OUTPUT_DIR:?Stado must provide WISENT_OUTPUT_DIR}"
[[ "$WISENT_PLATFORM" == "ios-arm64" ]]

plutil -lint "$WISENT_SOURCE_DIR/{{PROJECT}}.xcodeproj/project.pbxproj"
derived="$WISENT_OUTPUT_DIR/quality-derived-data"
trap 'rm -rf "$derived"' EXIT
xcodebuild -project "$WISENT_SOURCE_DIR/{{PROJECT}}.xcodeproj" -scheme "{{SCHEME}}" \
  -configuration Debug -destination 'generic/platform=iOS Simulator' \
  -derivedDataPath "$derived" CODE_SIGNING_ALLOWED=NO build
