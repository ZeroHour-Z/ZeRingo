#!/bin/zsh
# 打包 ZeRingo.app 到 dist/。用法：packaging/bundle.sh [--install] [--dmg]
#   --install  同时复制到 /Applications（会覆盖旧版本）
#   --dmg      同时生成 dist/ZeRingo-<版本>-<架构>.dmg，用于分发
set -euo pipefail
cd "$(dirname "$0")/.."

NAME=ZeRingo
BIN=zeringo
ID=com.zerohour.zeringo
VERSION=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
APP="dist/$NAME.app"

cargo build --release

# 图标
ICONSET=$(mktemp -d)/$NAME.iconset
mkdir -p "$ICONSET"
[[ -f packaging/icon-1024.png ]] || python3 -I packaging/make_icon.py packaging/icon-1024.png
for s in 16 32 128 256 512; do
  sips -z $s $s packaging/icon-1024.png --out "$ICONSET/icon_${s}x${s}.png" >/dev/null
  sips -z $((s*2)) $((s*2)) packaging/icon-1024.png --out "$ICONSET/icon_${s}x${s}@2x.png" >/dev/null
done

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "target/release/$BIN" "$APP/Contents/MacOS/$BIN"
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/$NAME.icns"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>$NAME</string>
  <key>CFBundleDisplayName</key><string>$NAME</string>
  <key>CFBundleIdentifier</key><string>$ID</string>
  <key>CFBundleExecutable</key><string>$BIN</string>
  <key>CFBundleIconFile</key><string>$NAME</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>LSMinimumSystemVersion</key><string>13.0</string>
  <key>LSUIElement</key><true/>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSHumanReadableCopyright</key><string>© ZeroHour</string>
</dict>
</plist>
PLIST

# 本机自签名（ad-hoc），本机使用无需开发者账号
codesign --force --deep --sign - "$APP"
echo "已生成：$APP"

if [[ " $* " == *" --dmg "* ]]; then
  # 用 dmgbuild 直接写入窗口布局（不需要控制 Finder）
  VENV=packaging/.venv
  if [[ ! -x "$VENV/bin/dmgbuild" ]]; then
    python3 -m venv "$VENV"
    "$VENV/bin/pip" install -q dmgbuild
  fi
  ARCH=$(lipo -archs "$APP/Contents/MacOS/$BIN" | tr ' ' '+')
  DMG="dist/$NAME-$VERSION-$ARCH.dmg"
  TMP=$(mktemp -d)
  python3 -I packaging/make_dmg_bg.py "$TMP"
  tiffutil -cathidpicheck "$TMP/bg.png" "$TMP/bg@2x.png" -out "$TMP/bg.tiff" >/dev/null
  rm -f "$DMG"
  "$VENV/bin/dmgbuild" -s packaging/dmg_settings.py \
    -D app="$APP" -D bg="$TMP/bg.tiff" -D icon="$APP/Contents/Resources/$NAME.icns" \
    "$NAME" "$DMG" >/dev/null
  rm -rf "$TMP"
  echo "已生成：$DMG"
fi

if [[ " $* " == *" --install "* ]]; then
  pkill -x "$BIN" 2>/dev/null || true
  rm -rf "/Applications/$NAME.app"
  cp -R "$APP" /Applications/
  echo "已安装到：/Applications/$NAME.app"
fi
