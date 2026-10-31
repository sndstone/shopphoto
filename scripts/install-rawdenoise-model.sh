#!/bin/sh
# Install ShopPhoto's RAW denoise model (rawdenoise-lab r4_plain).
#
#   scripts/install-rawdenoise-model.sh [DESTINATION]
#
# DESTINATION defaults to the per-user model directory the app searches
# (macOS: ~/Library/Application Support/shopphoto/denoise-models).
# Set RAWDENOISE_ASSETS to a directory that already holds the release assets
# (and SHA256SUMS) to skip the download.
set -eu

REPO=sndstone/rawdenoise-lab
TAG=model-r4_plain-2026-10-30
BASE="https://github.com/$REPO/releases/download/$TAG"

case "$(uname -s)" in
  Darwin)
    DEFAULT_DEST="$HOME/Library/Application Support/shopphoto/denoise-models"
    ASSET=r4_plain_fp16.mlpackage.zip
    ;;
  *)
    DEFAULT_DEST="${XDG_DATA_HOME:-$HOME/.local/share}/shopphoto/denoise-models"
    ASSET=r4_plain_fp32.onnx
    ;;
esac
DEST=${1:-$DEFAULT_DEST}

WORK=$(mktemp -d)
# Release archives keep read-only permissions; make them removable.
trap 'chmod -R u+w "$WORK"; rm -rf "$WORK"' EXIT

if [ -n "${RAWDENOISE_ASSETS:-}" ]; then
  for FILE in SHA256SUMS MODEL_CARD.md "$ASSET"; do
    cp "$RAWDENOISE_ASSETS/$FILE" "$WORK/$FILE"
  done
else
  for FILE in SHA256SUMS MODEL_CARD.md "$ASSET"; do
    echo "Downloading $FILE"
    curl --fail --location --silent --show-error --output "$WORK/$FILE" "$BASE/$FILE"
  done
fi

for FILE in MODEL_CARD.md "$ASSET"; do
  WANT=$(awk -v name="$FILE" '{ sub(/^\*/, "", $2) } $2 == name { print $1 }' "$WORK/SHA256SUMS")
  if command -v sha256sum >/dev/null 2>&1; then
    GOT=$(sha256sum "$WORK/$FILE" | awk '{ print $1 }')
  else
    GOT=$(shasum -a 256 "$WORK/$FILE" | awk '{ print $1 }')
  fi
  if [ -z "$WANT" ] || [ "$WANT" != "$GOT" ]; then
    echo "error: checksum mismatch for $FILE" >&2
    exit 1
  fi
done

mkdir -p "$DEST"
case "$ASSET" in
  *.mlpackage.zip)
    ditto -x -k "$WORK/$ASSET" "$WORK/package"
    PACKAGE=$(find "$WORK/package" -maxdepth 2 -name '*.mlpackage' -type d | head -n 1)
    if [ -z "$PACKAGE" ]; then
      echo "error: no .mlpackage inside $ASSET" >&2
      exit 1
    fi
    xcrun coremlcompiler compile "$PACKAGE" "$WORK/compiled" >/dev/null
    COMPILED=$(find "$WORK/compiled" -maxdepth 1 -name '*.mlmodelc' -type d | head -n 1)
    rm -rf "$DEST/rawdenoise_r4_plain.mlmodelc"
    ditto "$COMPILED" "$DEST/rawdenoise_r4_plain.mlmodelc"
    INSTALLED="$DEST/rawdenoise_r4_plain.mlmodelc"
    ;;
  *)
    cp "$WORK/$ASSET" "$DEST/rawdenoise_r4_plain.onnx"
    INSTALLED="$DEST/rawdenoise_r4_plain.onnx"
    ;;
esac
cp "$WORK/MODEL_CARD.md" "$DEST/rawdenoise_r4_plain.MODEL_CARD.md"
echo "Installed $INSTALLED"
