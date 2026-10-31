#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
VERSION=${SHOPPHOTO_VERSION:-$(sed -n 's/^version = "\([^"]*\)"/\1/p' "$ROOT/Cargo.toml" | head -1)}
VERSION=${VERSION:-0.1.0}
APP="$ROOT/dist/ShopPhoto.app"
APP_EXECUTABLE="$APP/Contents/MacOS/ShopPhoto"
DMG_SUFFIX=${SHOPPHOTO_DMG_SUFFIX:-}

# Native codecs use the locally installed libheif and its decoder libraries.
# Enable them for the host architecture, and package those libraries below.
# The other universal slice retains the self-contained standard raster codecs.
ARM_FEATURES=
INTEL_FEATURES=
if [ "${SHOPPHOTO_BUNDLE_NATIVE_CODECS:-0}" = 1 ]; then
  case "$(uname -m)" in
    arm64) ARM_FEATURES=--features=sp-app/native-codecs ;;
    x86_64) INTEL_FEATURES=--features=sp-app/native-codecs ;;
    *) echo "error: native-codec packaging needs a supported macOS host" >&2; exit 1 ;;
  esac
fi
rustup target add aarch64-apple-darwin x86_64-apple-darwin
cargo build --manifest-path "$ROOT/Cargo.toml" -p sp-app --release --locked --target aarch64-apple-darwin $ARM_FEATURES
cargo build --manifest-path "$ROOT/Cargo.toml" -p sp-app --release --locked --target x86_64-apple-darwin $INTEL_FEATURES

# `dist/ShopPhoto.app` is a generated release artifact. Recreate that exact
# bundle so files from an older packaging layout cannot leak into this DMG.
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
lipo -create \
  "$ROOT/target/aarch64-apple-darwin/release/shopphoto" \
  "$ROOT/target/x86_64-apple-darwin/release/shopphoto" \
  -output "$APP_EXECUTABLE"
sed "s/@VERSION@/$VERSION/g" "$ROOT/packaging/macos/Info.plist.in" > "$APP/Contents/Info.plist"
if [ "${SHOPPHOTO_BUNDLE_NATIVE_CODECS:-0}" = 1 ]; then
  python3 "$ROOT/scripts/package-macos-libraries.py" "$APP"
fi

# ShopPhoto's own rawdenoise-lab model ships in every build. Use a compiled
# model from SHOPPHOTO_BUNDLE_RAWDENOISE_MODEL, or download the pinned release,
# verify its SHA-256 and compile it once into target/rawdenoise-model.
RAWDENOISE_CACHE="$ROOT/target/rawdenoise-model"
RAWDENOISE_MODEL=${SHOPPHOTO_BUNDLE_RAWDENOISE_MODEL:-$RAWDENOISE_CACHE/rawdenoise_r4_plain.mlmodelc}
if [ -z "${SHOPPHOTO_BUNDLE_RAWDENOISE_MODEL:-}" ] && [ ! -f "$RAWDENOISE_MODEL/model.mil" ]; then
  "$ROOT/scripts/install-rawdenoise-model.sh" "$RAWDENOISE_CACHE"
fi
if [ ! -f "$RAWDENOISE_MODEL/model.mil" ] || [ ! -d "$RAWDENOISE_MODEL/weights" ]; then
  echo "error: not a compiled Core ML model: $RAWDENOISE_MODEL" >&2
  exit 1
fi
mkdir -p "$APP/Contents/Resources/DenoiseModels"
ditto "$RAWDENOISE_MODEL" "$APP/Contents/Resources/DenoiseModels/rawdenoise_r4_plain.mlmodelc"
CARD="$(dirname "$RAWDENOISE_MODEL")/rawdenoise_r4_plain.MODEL_CARD.md"
if [ -f "$CARD" ]; then
  cp "$CARD" "$APP/Contents/Resources/DenoiseModels/"
fi
echo "Included the ShopPhoto denoise model from $RAWDENOISE_MODEL"

if [ -n "${SHOPPHOTO_BUNDLE_ENHANCEMENT_MODELS:-}" ]; then
  python3 "$ROOT/scripts/package-native-ai.py" "$SHOPPHOTO_BUNDLE_ENHANCEMENT_MODELS" \
    --copy-to "$APP/Contents/Resources/EnhancementModels"
  cp "$ROOT/docs/native-ai-models.md" "$APP/Contents/Resources/Native AI Models.md"
fi

ICON_ROOT=$(mktemp -d)
DMG_STAGE=$(mktemp -d)
trap 'rm -rf "$ICON_ROOT" "$DMG_STAGE"' EXIT
ICONSET="$ICON_ROOT/ShopPhoto.iconset"
mkdir -p "$ICONSET"
for SIZE in 16 32 128 256 512; do
  sips -s format png -z "$SIZE" "$SIZE" "$ROOT/assets/shopphoto.svg" --out "$ICONSET/icon_${SIZE}x${SIZE}.png" >/dev/null
  DOUBLE=$((SIZE * 2))
  sips -s format png -z "$DOUBLE" "$DOUBLE" "$ROOT/assets/shopphoto.svg" --out "$ICONSET/icon_${SIZE}x${SIZE}@2x.png" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/ShopPhoto.icns"

if [ -n "${APPLE_SIGNING_IDENTITY:-}" ]; then
  codesign --force --deep --options runtime --timestamp --sign "$APPLE_SIGNING_IDENTITY" "$APP"
else
  codesign --force --deep --sign - "$APP"
fi

cp -R "$APP" "$DMG_STAGE/ShopPhoto.app"
mkdir -p "$DMG_STAGE/Photography Guide"
cp "$ROOT/docs/photography-roadmap.md" "$ROOT/docs/photography-acceptance.md" \
  "$ROOT/docs/release-0.4.0.md" \
  "$ROOT/docs/ai-runtime.md" "$ROOT/docs/native-ai-models.md" "$ROOT/docs/photo-merges.md" \
  "$ROOT/docs/phase-"*.md "$ROOT/docs/raster-input.md" \
  "$ROOT/docs/astro-stacking.md" "$ROOT/docs/sequence-matching.md" \
  "$ROOT/docs/tether-capture.md" "$ROOT/docs/lens-calibration.md" \
  "$ROOT/docs/platform-photo-capabilities.md" \
  "$DMG_STAGE/Photography Guide/"
cp "$ROOT/docs"/release-0.4.*.md "$DMG_STAGE/Photography Guide/"
if [ -f "$ROOT/docs/release-$VERSION.md" ]; then
  cp "$ROOT/docs/release-$VERSION.md" "$DMG_STAGE/Photography Guide/"
fi
cp "$ROOT/docs/raw-denoise.md" "$DMG_STAGE/Photography Guide/"
if [ -f "$APP/Contents/Resources/DenoiseModels/rawdenoise_r4_plain.MODEL_CARD.md" ]; then
  cp "$APP/Contents/Resources/DenoiseModels/rawdenoise_r4_plain.MODEL_CARD.md" \
    "$DMG_STAGE/Photography Guide/ShopPhoto RAW Denoise model card.md"
fi
cp "$ROOT/packaging/macos/Read Me.txt" "$DMG_STAGE/Read Me.txt"
if [ -f "$APP/Contents/Resources/Native Codec Requirements.txt" ]; then
  cat "$APP/Contents/Resources/Native Codec Requirements.txt" >> "$DMG_STAGE/Read Me.txt"
fi
cp -R "$ROOT/docs/research-2026-08-31" "$DMG_STAGE/Research Notebook"
ln -s /Applications "$DMG_STAGE/Applications"
ln -s "Research Notebook/index.html" "$DMG_STAGE/Open Research Notebook.html"
hdiutil create -volname ShopPhoto -srcfolder "$DMG_STAGE" -ov -format UDZO "$ROOT/dist/ShopPhoto-$VERSION$DMG_SUFFIX.dmg"

if [ -n "${APPLE_NOTARY_PROFILE:-}" ]; then
  xcrun notarytool submit "$ROOT/dist/ShopPhoto-$VERSION$DMG_SUFFIX.dmg" --keychain-profile "$APPLE_NOTARY_PROFILE" --wait
  xcrun stapler staple "$ROOT/dist/ShopPhoto-$VERSION$DMG_SUFFIX.dmg"
fi
