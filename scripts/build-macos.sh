#!/usr/bin/env bash
# Build le frontend natif macOS (SwiftUI + Liquid Glass) et l'installe dans /Applications.
set -euo pipefail
cd "$(dirname "$0")/.."

# Meme minimum que Package.swift : sinon le code C de ring est compile pour la version du
# SDK installe et ld previent que les objets visent un macOS plus recent que l'app.
export MACOSX_DEPLOYMENT_TARGET=26.0

cargo build -p switchboard-ffi --release

pushd macos >/dev/null
swift build -c release
popd >/dev/null

APP="/Applications/Switchboard.app"
[ -w "/Applications" ] || APP="$HOME/Applications/Switchboard.app"

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp macos/.build/release/Switchboard "$APP/Contents/MacOS/"
cp icons/icon.icns "$APP/Contents/Resources/icon.icns"

cat > "$APP/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleExecutable</key><string>Switchboard</string>
  <key>CFBundleIdentifier</key><string>com.skolln.switchboard</string>
  <key>CFBundleName</key><string>Switchboard</string>
  <key>CFBundleIconFile</key><string>icon.icns</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>0.5.0</string>
  <key>CFBundleVersion</key><string>0.5.0</string>
  <key>LSMinimumSystemVersion</key><string>26.0</string>
</dict>
</plist>
EOF

echo "Installé : $APP"
open "$APP"
