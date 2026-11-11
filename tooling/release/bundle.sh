#!/usr/bin/env bash
# Builds Dispatch.app for macOS arm64 with its ZIP, DMG, checksums, and signed appcast:
#   bash tooling/release/bundle.sh <version>
# The bundle holds the GPUI app, which is also the `dispatch` core and CLI
# (Contents/Resources/dispatch links to it), the voice engine
# (Contents/Resources/dispatch-voice, with the ONNX Runtime dylibs it needs), the app
# icon, and Sparkle. Signing is ad-hoc, as the Electron app's was: no certificate,
# entitlements, hardened runtime, or notarization.
# SOURCE_DATE_EPOCH (the last commit's time by default) is the Sparkle build number.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
VERSION="${1:?usage: bundle.sh <version>}"
[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$ ]] || { echo "Invalid release version: $VERSION" >&2; exit 1; }
[[ "$(uname -s)" == Darwin && "$(uname -m)" == arm64 ]] || { echo 'Releases require a macOS arm64 host.' >&2; exit 1; }
if [[ -z "${SOURCE_DATE_EPOCH:-}" ]]; then
  SOURCE_DATE_EPOCH="$(git -C "$ROOT" log -1 --format=%ct 2>/dev/null || true)"
  [[ -n "$SOURCE_DATE_EPOCH" ]] || SOURCE_DATE_EPOCH="$(date +%s)"
fi
export SOURCE_DATE_EPOCH
cfg() { node -p "require(process.argv[1]).$1" "$ROOT/tooling/updater/config.cjs"; }
PRODUCT="$(cfg product)"
OUTPUT="$ROOT/target/release-bundle/$VERSION"
APP="$OUTPUT/$PRODUCT.app"
[[ ! -e "$OUTPUT" ]] || { echo "Release output already exists: $OUTPUT (move it before rebuilding)." >&2; exit 1; }
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources" "$APP/Contents/Frameworks"

bash "$ROOT/tooling/updater/fetch.sh"
(cd "$ROOT" && cargo build --release --locked -p dispatch-app)
cp "$ROOT/target/release/dispatch-app" "$APP/Contents/MacOS/$PRODUCT"
# The `dispatch` command agents, Polterware, and ~/.local/bin/dispatch run: the same binary,
# which runs as the CLI and the core when it is given a command or called dispatch.
ln -s "../MacOS/$PRODUCT" "$APP/Contents/Resources/dispatch"

# The voice engine, a binary of its own because its models take hundreds of MB and the
# core and the app stay lean. ort's `copy-dylibs` leaves the ONNX Runtime dylibs it
# needs beside it in the target folder when it links them dynamically (the macOS arm64
# build links ONNX Runtime statically and copies none); each goes next to the engine,
# found through @loader_path, and the engine's references follow.
(cd "$ROOT" && cargo build --release --locked -p voice-server --bin dispatch-voice)
VOICE="$APP/Contents/Resources/dispatch-voice"
cp "$ROOT/target/release/dispatch-voice" "$VOICE"
relink() { # points <binary>'s references to <dylib> at the copy beside it
  while read -r ref _; do
    [[ "$ref" == *"/$2" ]] || continue
    [[ "$ref" == @loader_path/* ]] && continue
    install_name_tool -change "$ref" "@loader_path/$2" "$1"
  done < <(otool -L "$1" | tail -n +2)
}
shopt -s nullglob
for dylib in "$ROOT"/target/release/libonnxruntime*.dylib; do
  name="$(basename "$dylib")"
  cp "$dylib" "$APP/Contents/Resources/$name"
  install_name_tool -id "@loader_path/$name" "$APP/Contents/Resources/$name"
  relink "$VOICE" "$name"
  for other in "$APP"/Contents/Resources/libonnxruntime*.dylib; do
    [[ "$other" == "$APP/Contents/Resources/$name" ]] || relink "$other" "$name"
  done
done
shopt -u nullglob

PLIST="$APP/Contents/Info.plist"
cat > "$PLIST" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleIdentifier</key><string>com.polterware.dispatch</string>
  <key>CFBundleName</key><string>$PRODUCT</string>
  <key>CFBundleDisplayName</key><string>$PRODUCT</string>
  <key>CFBundleExecutable</key><string>$PRODUCT</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleVersion</key><string>$SOURCE_DATE_EPOCH</string>
  <key>CFBundleIconName</key><string>dispatch</string>
  <key>CFBundleIconFile</key><string>dispatch</string>
  <key>LSMinimumSystemVersion</key><string>15.0</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.developer-tools</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSMicrophoneUsageDescription</key><string>Dispatch listens when you talk to a run; audio stays on this Mac.</string>
  <key>CFBundleURLTypes</key>
  <array>
    <dict>
      <key>CFBundleURLName</key><string>Dispatch</string>
      <key>CFBundleURLSchemes</key><array><string>dispatch</string></array>
    </dict>
  </array>
  <key>SUFeedURL</key><string>$(cfg feedUrl)</string>
  <key>SUPublicEDKey</key><string>$(cfg publicKey)</string>
  <key>SUEnableAutomaticChecks</key><true/>
  <key>SUScheduledCheckInterval</key><integer>86400</integer>
  <key>SUAutomaticallyUpdate</key><false/>
  <key>SUShowReleaseNotes</key><false/>
</dict>
</plist>
PLIST
plutil -lint "$PLIST"

# The Icon Composer icon, compiled into Assets.car and dispatch.icns.
xcrun actool "$ROOT/app/resources/dispatch.icon" --compile "$APP/Contents/Resources" \
  --platform macosx --minimum-deployment-target 15.0 --app-icon dispatch \
  --output-partial-info-plist "$OUTPUT/icon-info.plist" >/dev/null

ditto "$ROOT/tooling/updater/vendor/Sparkle.framework" "$APP/Contents/Frameworks/Sparkle.framework"
cp "$ROOT/tooling/updater/vendor/LICENSE" "$APP/Contents/Resources/Sparkle-LICENSE"

# Ad-hoc signing, inside out: the voice engine and its dylibs, Sparkle's helpers, the
# framework, then the app.
for path in "$VOICE" "$APP"/Contents/Resources/libonnxruntime*.dylib; do
  if [[ -e "$path" ]]; then
    codesign --force --sign - --timestamp=none "$path"
  fi
done
FRAMEWORK="$APP/Contents/Frameworks/Sparkle.framework"
for path in Versions/B/XPCServices/Downloader.xpc Versions/B/XPCServices/Installer.xpc Versions/B/Autoupdate Versions/B/Updater.app ""; do
  codesign --force --deep --sign - --timestamp=none "$FRAMEWORK/$path"
done
codesign --force --sign - --timestamp=none "$APP"
# The binary's mtime is pinned to the commit time, because the core's build handshake
# reads it; signing rewrote the file, and the signature does not cover the mtime.
touch -t "$(date -r "$SOURCE_DATE_EPOCH" +%Y%m%d%H%M.%S)" "$APP/Contents/MacOS/$PRODUCT"
codesign --verify --deep --strict "$APP"

ARTIFACT="${PRODUCT// /-}-$VERSION-arm64"
(cd "$OUTPUT" && ditto -c -k --sequesterRsrc --keepParent "$PRODUCT.app" "$ARTIFACT.zip")
hdiutil create -volname "$PRODUCT" -srcfolder "$APP" -ov -format UDZO "$OUTPUT/$ARTIFACT.dmg" >/dev/null
(cd "$OUTPUT" && shasum -a 256 "$ARTIFACT.dmg" "$ARTIFACT.zip" > SHA256SUMS)
# Rejection is expected: ad-hoc is not Developer ID signing and notarization.
spctl --assess --type execute --verbose=4 "$APP" > "$OUTPUT/gatekeeper.txt" 2>&1 || true
# The signed appcast needs the Sparkle key (POLTERWARE_SPARKLE_PRIVATE_KEY, or the
# Keychain account polterware); SKIP_APPCAST=1 builds a local bundle without it.
if [[ "${SKIP_APPCAST:-}" != 1 ]]; then
  node "$ROOT/tooling/updater/appcast.cjs" "$OUTPUT" "$VERSION"
fi
echo "Built $OUTPUT"
