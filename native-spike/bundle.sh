#!/bin/sh
# Wraps the release binary and the Sirus host as "Sirus Native.app", its own app id, ad-hoc signed.
set -e
cd "$(dirname "$0")"
APP="Sirus Native.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS"
cp target/release/sirus-native-spike "$APP/Contents/MacOS/"
cp target/host/release/sirus-host "$APP/Contents/MacOS/"
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>sirus-native-spike</string>
<key>CFBundleIdentifier</key><string>com.siruscode.native</string>
<key>CFBundleName</key><string>Sirus Native</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>0.0.1</string>
<key>LSMinimumSystemVersion</key><string>13.0</string>
<key>NSHighResolutionCapable</key><true/>
<key>NSMicrophoneUsageDescription</key><string>O Sirus Native usa o microfone para o ditado no composer.</string>
<key>NSSpeechRecognitionUsageDescription</key><string>O Sirus Native transcreve o ditado no composer.</string>
</dict></plist>
PLIST
# A stable identity, so macOS remembers "Always allow" for the Keychain across builds.
codesign --force -s "Sirus Code Local Signing" "$APP/Contents/MacOS/sirus-host" 2>/dev/null || true
codesign --force -s "Sirus Code Local Signing" "$APP" 2>/dev/null || codesign --force -s - "$APP" 2>/dev/null
echo "$PWD/$APP"
