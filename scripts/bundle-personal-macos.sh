#!/bin/sh
# Personal build: the standard bundle plus native HEIC/AVIF codecs and the
# locally installed native enhancement models.
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)

: "${SHOPPHOTO_DMG_SUFFIX:=-personal}"
: "${SHOPPHOTO_BUNDLE_NATIVE_CODECS:=1}"
: "${SHOPPHOTO_BUNDLE_ENHANCEMENT_MODELS:=$HOME/Library/Application Support/shopphoto/enhance-models/native}"
export SHOPPHOTO_DMG_SUFFIX SHOPPHOTO_BUNDLE_NATIVE_CODECS SHOPPHOTO_BUNDLE_ENHANCEMENT_MODELS

exec "$ROOT/scripts/bundle-macos.sh"
