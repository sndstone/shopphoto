# Rendered photograph input

ShopPhoto opens JPEG, PNG, TIFF, WebP, BMP and JPEG XL as non-destructive
develop sources. JPEG XL uses the pure-Rust `jxl-oxide` decoder. Originals
stay unchanged; edits use the same `.spedit` documents as camera RAW files.

The input decoder retains the original bit depth and embedded ICC bytes,
applies EXIF orientation, and converts RGB into floating-point linear
Rec.2020. Untagged integer images use sRGB. The existing GPU working buffers
use half floats, including for high-precision export; a 16-bit source is not
reduced to an 8-bit preview before editing. Capture metadata includes camera,
lens, exposure, ISO, time and GPS when available.

ICC matrix/shaper conversion enables extended float range. Linear TIFF
masters retain negative working RGB and HDR values above one when reopened.
The decoder reads TIFF ICC metadata before applying pixel-allocation limits,
avoiding a codec issue that could drop profiles on small images. Raster input
is bounded at 150 MP.

Rendered images bypass sensor preparation, demosaicing, RAW noise reduction,
camera profiles, the default RAW tone curve and clipped-sensor highlight
reconstruction. Standard tone/color, curves, masks, retouching, crop,
geometry, manual noise reduction, sharpening and export share the develop
pipeline. White balance is a relative correction of the rendered image;
5500 K/zero tint represents its original color. The neutral picker samples
the rendered RGB source.

Transparency is explicitly rejected rather than flattened against an
arbitrary background. Non-RGB ICC profiles and untagged floating-point
sources return an actionable error. This is a photograph develop workflow;
it does not preserve layered TIFF/PSD editing documents. PQ-tagged input
uses 203 cd/m² reference white, matching ShopPhoto's HDR export. HLG input
requires an explicit display-reference interpretation and is currently
rejected.

## Optional native AVIF and HEIF input

The `native-codecs` feature adds AVIF, HEIC and HEIF via `libheif`. It is off
by default, and portable builds omit those formats from supported-input
lists. It preserves high-bit-depth samples and honors RGB ICC or NCLX color
descriptions. The installed libheif must include the appropriate AV1/HEVC
decoder plugins; missing decoders are reported as errors.

For a development machine with libheif and pkg-config installed:

```sh
cargo run -p sp-app --features native-codecs
cargo test -p sp-raw --features native-codecs raster:: --lib
```

Enabling this feature changes native runtime dependencies. Release packagers
must bundle libheif, its dependent libraries and required decoder plugins,
or document/install them on the target system. The existing portable build
commands deliberately leave the feature disabled.

## Validation

```sh
cargo test -p sp-raw raster:: --lib
cargo test -p sp-core source::tests --lib
cargo test -p sp-pipeline --test raster
```

The GPU integration test requires a real adapter and verifies neutral
16-bit TIFF rendering, retained sub-8-bit differences, proxy/full export
agreement, exposure adjustment, ignored RAW-only controls, RGB reduction
and unchanged source pixels. Decoder tests cover ICC color conversion,
EXIF/orientation, transparency errors, JPEG XL precision and PQ headroom;
native tests also cover HEVC, AVIF/NCLX and PQ AVIF.
