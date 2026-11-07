#!/bin/sh
# Wraps the release binary, the Sirus host and the Mermaid and KaTeX files as "Sirus.app" (app id
# com.siruscode.native, kept from "Sirus Native" so its data and macOS permissions stay), signed
# with the identity chosen below. The version is the one in Cargo.toml.
# assets/reply-render holds the committed libraries the window's hidden page loads to draw
# diagrams and formulas in replies (reply_render.rs); nothing is built here, no npm or dist/.
set -e
cd "$(dirname "$0")"
APP="Sirus.app"
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS"
cp target/release/sirus-native-spike "$APP/Contents/MacOS/"
cp target/host/release/sirus-host "$APP/Contents/MacOS/"
# The host serves it at /assets/ with --assets-dir (backend.rs looks in Contents/Resources/reply-render).
mkdir -p "$APP/Contents/Resources"
ditto assets/reply-render "$APP/Contents/Resources/reply-render"
rm -rf "$APP/Contents/Resources/reply-render/build"
# Its own icon, for the Dock and Finder.
cp assets/AppIcon.icns "$APP/Contents/Resources/AppIcon.icns"
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>sirus-native-spike</string>
<key>CFBundleIdentifier</key><string>com.siruscode.native</string>
<key>CFBundleAllowMixedLocalizations</key><true/>
<key>CFBundleName</key><string>Sirus</string>
<key>CFBundleDisplayName</key><string>Sirus</string>
<key>CFBundleIconFile</key><string>AppIcon</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>$VERSION</string>
<key>CFBundleVersion</key><string>$VERSION</string>
<key>LSMinimumSystemVersion</key><string>13.0</string>
<key>NSHighResolutionCapable</key><true/>
<key>CFBundleURLTypes</key><array><dict>
<key>CFBundleURLName</key><string>com.siruscode.native.prompt-link</string>
<key>CFBundleURLSchemes</key><array><string>sirus</string></array>
</dict></array>
<key>NSMicrophoneUsageDescription</key><string>O Sirus usa o microfone para o ditado no composer.</string>
<key>NSSpeechRecognitionUsageDescription</key><string>O Sirus transcreve o ditado no composer.</string>
</dict></plist>
PLIST
# The Apple Development identity when this Mac has one: the Keychain ties an item to its team, so
# no later build asks for the password. A self-signed identity ties it to this build's exact hash,
# and every new build asks again; it remains the fallback for a Mac without the certificate.
IDENTITY="$(security find-identity -v -p codesigning | sed -n 's/.*"\(Apple Development:[^"]*\)".*/\1/p' | head -1)"
IDENTITY="${IDENTITY:-Sirus Code Local Signing}"
codesign --force -s "$IDENTITY" "$APP/Contents/MacOS/sirus-host" 2>/dev/null || true
codesign --force -s "$IDENTITY" "$APP" 2>/dev/null || codesign --force -s - "$APP" 2>/dev/null
echo "$PWD/$APP"
