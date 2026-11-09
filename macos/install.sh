#!/bin/sh
# Build, sign, notarize, and install the TerminalCam Camera app (container
# for the CMIO virtual camera extension).
#
# Requirements:
#   - Xcode signed into an Apple Developer account (cloud signing mints the
#     Developer ID certificate on first export)
#   - xcodegen (brew install xcodegen)
#   - notarytool credentials stored once via:
#       xcrun notarytool store-credentials terminal-cam-notary \
#           --apple-id <apple-id> --team-id <team-id>
#
# macOS requires system extensions to be Developer ID signed AND notarized;
# a plain development build is refused by sysextd. On MDM-managed Macs the
# extension must additionally be allowlisted — see
# allow-terminal-cam-extension.mobileconfig.
set -eu
cd "$(dirname "$0")"

NOTARY_PROFILE="${NOTARY_PROFILE:-terminal-cam-notary}"

xcodegen generate
xcodebuild \
    -project TerminalCamCamera.xcodeproj \
    -scheme TerminalCamCamera \
    -configuration Release \
    -archivePath build/TerminalCam.xcarchive \
    -allowProvisioningUpdates \
    -allowProvisioningDeviceRegistration \
    archive

cat > build/exportOptions.plist <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>method</key>
	<string>developer-id</string>
	<key>destination</key>
	<string>export</string>
</dict>
</plist>
PLIST

xcodebuild \
    -exportArchive \
    -archivePath build/TerminalCam.xcarchive \
    -exportOptionsPlist build/exportOptions.plist \
    -exportPath build/export \
    -allowProvisioningUpdates

APP="build/export/TerminalCamCamera.app"

ditto -c -k --keepParent "$APP" build/TerminalCamCamera.zip
xcrun notarytool submit build/TerminalCamCamera.zip \
    --keychain-profile "$NOTARY_PROFILE" --wait
xcrun stapler staple "$APP"

DEST="/Applications/TerminalCamCamera.app"
osascript -e 'quit app "TerminalCamCamera"' 2>/dev/null || true
rm -rf "$DEST"
ditto "$APP" "$DEST"
echo "Installed $DEST"
open "$DEST"
