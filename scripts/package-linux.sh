#!/bin/sh
set -eu
ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
VERSION=${SHOPPHOTO_VERSION:-$(sed -n 's/^version = "\([^"]*\)"/\1/p' "$ROOT/Cargo.toml" | head -1)}
VERSION=${VERSION:-0.1.0}
cargo build --manifest-path "$ROOT/Cargo.toml" -p sp-app --release
mkdir -p "$ROOT/dist/AppDir/usr/bin" \
  "$ROOT/dist/AppDir/usr/share/applications" \
  "$ROOT/dist/AppDir/usr/share/icons/hicolor/scalable/apps"
cp "$ROOT/target/release/shopphoto" "$ROOT/dist/AppDir/usr/bin/"
cp "$ROOT/assets/shopphoto.svg" "$ROOT/dist/AppDir/usr/share/icons/hicolor/scalable/apps/com.shopphoto.ShopPhoto.svg"
cp "$ROOT/assets/shopphoto.svg" "$ROOT/dist/AppDir/com.shopphoto.ShopPhoto.svg"
cp "$ROOT/packaging/linux/com.shopphoto.ShopPhoto.desktop" "$ROOT/dist/AppDir/"
cp "$ROOT/packaging/linux/com.shopphoto.ShopPhoto.desktop" "$ROOT/dist/AppDir/usr/share/applications/"
cp "$ROOT/packaging/linux/AppRun" "$ROOT/dist/AppDir/AppRun"
chmod +x "$ROOT/dist/AppDir/AppRun"
if command -v appimagetool >/dev/null 2>&1; then
  appimagetool "$ROOT/dist/AppDir" "$ROOT/dist/ShopPhoto-$VERSION.AppImage"
fi
if command -v flatpak-builder >/dev/null 2>&1; then
  flatpak-builder --force-clean "$ROOT/dist/flatpak-build" "$ROOT/packaging/linux/com.shopphoto.ShopPhoto.yml"
fi
