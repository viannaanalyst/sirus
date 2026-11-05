#!/bin/sh
# Wraps the release binary, the Sirus host and the phone's web UI as "Sirus Native.app", its own
# app id, signed with the local identity.
# The web UI (dist/) is built from this checkout; it is what a phone or browser on the tailnet
# opens when Settings -> Conexoes allows other devices. BUILD_UI=1 rebuilds it even when a dist/
# exists; it is built when missing. It adds about 97 MB to the bundle (about 34 MB compressed).
set -e
cd "$(dirname "$0")"
ROOT="$(cd .. && pwd)"
if [ "${BUILD_UI:-0}" = 1 ] || [ ! -f "$ROOT/dist/index.html" ]; then
  [ -d "$ROOT/node_modules" ] || (cd "$ROOT" && npm ci --no-audit --no-fund)
  (cd "$ROOT" && npm run build)
fi
APP="Sirus Native.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS"
cp target/release/sirus-native-spike "$APP/Contents/MacOS/"
cp target/host/release/sirus-host "$APP/Contents/MacOS/"
# The host serves it with --ui-dir (backend.rs looks in Contents/Resources/ui, else ../dist).
mkdir -p "$APP/Contents/Resources"
ditto "$ROOT/dist" "$APP/Contents/Resources/ui"
# The same icon as Sirus Code, for the Dock and Finder.
cp "$ROOT/src-tauri/icons/icon.icns" "$APP/Contents/Resources/AppIcon.icns"
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>sirus-native-spike</string>
<key>CFBundleIdentifier</key><string>com.siruscode.native</string>
<key>CFBundleName</key><string>Sirus Native</string>
<key>CFBundleIconFile</key><string>AppIcon</string>
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
