#!/usr/bin/env bash
# Builds capreal.app (ad-hoc signed, own bundle identifier) so macOS asks for screen,
# camera and microphone permission for capreal itself. Usage: make-capreal-app.sh OUT_DIR
# Run it:  open -W --stdout OUT/run.json --stderr OUT/run.log OUT/capreal.app --args --out DIR ...
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
APP="$1/capreal.app"
mkdir -p "$APP/Contents/MacOS"
clang -fobjc-arc -O2 -Wall -mmacosx-version-min=12.0 "$HERE/capreal.m" -o "$APP/Contents/MacOS/capreal" \
  -framework Foundation -framework AppKit -framework AVFoundation -framework CoreMedia \
  -framework CoreVideo -framework VideoToolbox -weak_framework ScreenCaptureKit
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleIdentifier</key><string>com.kreokit.rec.capreal-test</string>
  <key>CFBundleName</key><string>capreal</string>
  <key>CFBundleExecutable</key><string>capreal</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>0.0.0</string>
  <key>LSMinimumSystemVersion</key><string>12.0</string>
  <key>LSUIElement</key><true/>
  <key>NSCameraUsageDescription</key><string>KreoKit Rec research test: measures the camera's frame delivery.</string>
  <key>NSMicrophoneUsageDescription</key><string>KreoKit Rec research test: measures the microphone's timing.</string>
</dict></plist>
PLIST
codesign --force -s - "$APP"
echo "built $APP"
