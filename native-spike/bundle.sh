#!/bin/sh
# Wraps the release binary as "Sirus Native (teste).app", its own app id, ad-hoc signed.
set -e
cd "$(dirname "$0")"
APP="Sirus Native (teste).app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS"
cp target/release/sirus-native-spike "$APP/Contents/MacOS/"
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>sirus-native-spike</string>
<key>CFBundleIdentifier</key><string>com.siruscode.native-spike</string>
<key>CFBundleName</key><string>Sirus Native (teste)</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>0.0.1</string>
<key>LSMinimumSystemVersion</key><string>13.0</string>
<key>NSHighResolutionCapable</key><true/>
</dict></plist>
PLIST
codesign --force -s - "$APP" 2>/dev/null
echo "$PWD/$APP"
