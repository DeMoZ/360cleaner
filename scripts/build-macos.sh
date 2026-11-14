#!/bin/bash
# Собирает dist/360Cleaner.app (Apple Silicon), dist/360Cleaner-macOS.dmg и архив для обновления
# dist/360Cleaner-<версия>-macos-arm64.zip. Запуск: scripts/build-macos.sh
set -euo pipefail
cd "$(dirname "$0")/.."

# только Apple Silicon: сборку для Intel не выпускаем
target=aarch64-apple-darwin
rustup target add "$target" >/dev/null
cargo test --quiet
cargo build --release --target "$target"

version=$(cargo metadata --no-deps --format-version 1 | python3 -c "import json,sys; print(json.load(sys.stdin)['packages'][0]['version'])")
app="dist/360Cleaner.app"
rm -rf "$app" dist/360Cleaner-macOS.dmg dist/360Cleaner-*-macos-arm64.zip
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "target/$target/release/cleaner360" "$app/Contents/MacOS/cleaner360"
cp assets/AppIcon.icns "$app/Contents/Resources/AppIcon.icns"
cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>360Cleaner</string>
  <key>CFBundleDisplayName</key><string>360Cleaner</string>
  <key>CFBundleIdentifier</key><string>com.demoz.cleaner360</string>
  <key>CFBundleExecutable</key><string>cleaner360</string>
  <key>CFBundleIconFile</key><string>AppIcon</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$version</string>
  <key>CFBundleVersion</key><string>$version</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>LSApplicationCategoryType</key><string>public.app-category.photography</string>
  <key>CFBundleDevelopmentRegion</key><string>ru</string>
</dict>
</plist>
PLIST
# подпись «ad hoc»: без неё Apple Silicon не запустит скачанный бинарник даже после снятия карантина
codesign --force --deep --sign - "$app"
# архив для обновления из программы: ditto сохраняет подпись и атрибуты .app
ditto -c -k --keepParent "$app" "dist/360Cleaner-$version-macos-arm64.zip"

staging=$(mktemp -d)
cp -R "$app" "$staging/"
ln -s /Applications "$staging/Applications"
hdiutil create -quiet -volname "360Cleaner" -srcfolder "$staging" -ov -format UDZO dist/360Cleaner-macOS.dmg
rm -rf "$staging"
echo "Готово: $app, dist/360Cleaner-macOS.dmg, dist/360Cleaner-$version-macos-arm64.zip"
