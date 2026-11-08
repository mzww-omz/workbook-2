#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

APP_NAME="Workbook 2"
APP_DIR="$ROOT/dist/$APP_NAME.app"
BIN="$ROOT/target/release/workbook2"

cargo build --release -p workbook-app --bin workbook2

rm -rf "$APP_DIR"
mkdir -p "$APP_DIR/Contents/MacOS" "$APP_DIR/Contents/Resources"
cp "$BIN" "$APP_DIR/Contents/MacOS/Workbook2"
chmod 755 "$APP_DIR/Contents/MacOS/Workbook2"

cat > "$APP_DIR/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key>
  <string>ja</string>
  <key>CFBundleDisplayName</key>
  <string>Workbook 2</string>
  <key>CFBundleExecutable</key>
  <string>Workbook2</string>
  <key>CFBundleIdentifier</key>
  <string>local.workbook2.desktop</string>
  <key>CFBundleInfoDictionaryVersion</key>
  <string>6.0</string>
  <key>CFBundleName</key>
  <string>Workbook 2</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleShortVersionString</key>
  <string>0.1.0</string>
  <key>CFBundleVersion</key>
  <string>0.1.0</string>
  <key>LSApplicationCategoryType</key>
  <string>public.app-category.education</string>
  <key>LSMinimumSystemVersion</key>
  <string>13.0</string>
  <key>NSHighResolutionCapable</key>
  <true/>
  <key>NSPrincipalClass</key>
  <string>NSApplication</string>
  <key>NSRequiresAquaSystemAppearance</key>
  <false/>
</dict>
</plist>
PLIST

printf 'APPL????' > "$APP_DIR/Contents/PkgInfo"

# Local ad-hoc signature. Distribution still needs Developer ID + notarization.
codesign --force --deep --sign - "$APP_DIR" >/dev/null

if [[ "${1:-}" == "--dmg" ]]; then
  mkdir -p "$ROOT/dist"
  rm -f "$ROOT/dist/Workbook2-0.1.0-arm64.dmg"
  hdiutil create -quiet -volname "$APP_NAME" -srcfolder "$APP_DIR" \
    -ov -format UDZO "$ROOT/dist/Workbook2-0.1.0-arm64.dmg"
  echo "Created dist/Workbook2-0.1.0-arm64.dmg"
fi

echo "Created $APP_DIR"
echo "Open with: open \"$APP_DIR\""
