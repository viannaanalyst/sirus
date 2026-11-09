#!/bin/zsh
# Keeps the native iPhone app signed with the owner's free Apple account (ADR-135 addendum):
# a build signed by the Personal Team expires after 7 days, so this rebuilds it (a fresh
# provisioning profile) and reinstalls it over Wi-Fi when the iPhone is on the same network.
# Run by the LaunchAgent `com.siruscode.renew-iphone` every few hours; it only reinstalls once
# the last install is older than RENEW_AFTER_DAYS. Pass --force to reinstall now.
set -u
export PATH="$HOME/.local/bin:/usr/local/bin:/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin"

ROOT="${0:A:h:h}"
DEVICE="${SIRUS_IPHONE_ID:-00008150-000E18340100C01C}"
RENEW_AFTER_DAYS=3
STATE="$HOME/Library/Application Support/com.siruscode.native/iphone-renew"
BUILD=/tmp/sirus-iphone-renew
mkdir -p "$STATE"
log() { print -r -- "$(date '+%Y-%m-%d %H:%M:%S') $*"; }

if [[ "${1:-}" != "--force" && -f "$STATE/last-install" ]]; then
  age=$(( ($(date +%s) - $(stat -f %m "$STATE/last-install")) / 86400 ))
  if (( age < RENEW_AFTER_DAYS )); then
    exit 0
  fi
fi

# Reachable means paired and on this Wi-Fi (or the cable).
if ! xcrun devicectl list devices 2>/dev/null | grep -F "$DEVICE" | grep -qE "available|connected"; then
  log "iPhone not reachable; trying again later"
  exit 0
fi

# The update address baked into the app is the computer's public route (the only way the phone
# reaches it, ADR-155), read from the same `remote-public.json` that pairing uses; only its
# `url` is read, never the Access token. Override with SIRUS_UPDATE_URL. It is applied by
# `expo prebuild` (Expo.plist); a mismatch with the generated project means prebuild is due.
DATA="$HOME/Library/Application Support/com.siruscode.native"
if [[ -z "${SIRUS_UPDATE_URL:-}" && -f "$DATA/remote-public.json" ]]; then
  route=$(/usr/bin/plutil -extract url raw -o - "$DATA/remote-public.json" 2>/dev/null)
  [[ "$route" == https://* ]] && SIRUS_UPDATE_URL="${route%/}/api/app-update/manifest"
fi
export SIRUS_UPDATE_URL
if [[ -n "${SIRUS_UPDATE_URL:-}" ]]; then
  baked=$(/usr/bin/plutil -extract EXUpdatesURL raw -o - "$ROOT/ios/Sirus/Supporting/Expo.plist" 2>/dev/null)
  [[ "$baked" == "$SIRUS_UPDATE_URL" ]] || log "warning: ios/ has updates URL '${baked:-none}', expected $SIRUS_UPDATE_URL; run expo prebuild with SIRUS_UPDATE_URL"
else
  log "warning: no remote-public.json route; the build has updates off"
fi

log "building"
if ! xcodebuild -workspace "$ROOT/ios/Sirus.xcworkspace" -scheme Sirus -configuration Release \
    -destination generic/platform=iOS -derivedDataPath "$BUILD" -allowProvisioningUpdates build \
    > "$STATE/build.log" 2>&1; then
  log "build failed; see $STATE/build.log"
  exit 1
fi

log "installing"
if xcrun devicectl device install app --device "$DEVICE" "$BUILD/Build/Products/Release-iphoneos/Sirus.app" > "$STATE/install.log" 2>&1; then
  touch "$STATE/last-install"
  log "renewed"
else
  log "install failed; see $STATE/install.log"
  exit 1
fi
