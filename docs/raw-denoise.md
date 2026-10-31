# Neural RAW denoise

Since 0.5.0, ShopPhoto's AI RAW denoise runs one model on every Bayer RAW:
`r4_plain` from ShopPhoto's own
[`sndstone/rawdenoise-lab`](https://github.com/sndstone/rawdenoise-lab), trained
from random initialisation on the owner's photographs.

## In the app

- The **Detail → Noise Reduction** panel shows **ShopPhoto RAW Denoise** for
  every decoded RAW (all are 2×2 Bayer; X-Trans and Foveon are not decoded).
  **Cleanest** uses the full model result; **Preserve detail** blends back
  20–25 % of the original. Off leaves the manual luminance/chroma controls.
- The macOS DMG bundles the model in
  `ShopPhoto.app/Contents/Resources/DenoiseModels`. Builds without it show
  **Download ShopPhoto model…**, which fetches the pinned public release
  asset, checks its SHA-256 and compiles it with Core ML on the Mac (no Xcode
  needed) into the per-user `denoise-models` directory.
- `SHOPPHOTO_DENOISE_MODELS=<dir>` overrides both locations.

## Runtime contract

`sp-ai` matches `denoise_lab.inference.Denoiser` and the `rawdenoise` CLI
defaults:

- R/G1/B/G2 packing of a red-topped quad (top-left, top-right, bottom-right,
  bottom-left), `(code − black)/(white − black)` scaling, negatives kept.
- 512×512 tiles with a 60-px mirrored apron on every side and a 452-px stride;
  each tile is weighted by a `ramp^2.2` feather (`(i + 1)/(overlap + 1)` at the
  edges) and the sum is normalized by the accumulated weights.
- The stuck-photosite pre-pass (`correct_defects`, k = 8, floor 0.01,
  adjacency 0.1), ported in `sp-ai/src/defects.rs`. Blending always mixes
  against the untouched source.
- Files: `rawdenoise_r4_plain.mlmodelc` (macOS, compiled from
  `r4_plain_fp16.mlpackage`; input `inp`, float32 output read through a
  strided/Float16-aware copy) and `rawdenoise_r4_plain.onnx` (Windows/Linux,
  `r4_plain_fp32.onnx`; needs tract 0.23 for its two-input `Pad`).

## Installing and packaging

- `scripts/install-rawdenoise-model.sh [DESTINATION]` downloads the pinned
  release, verifies `SHA256SUMS` and installs the platform's model (macOS
  compiles it with `xcrun coremlcompiler`). Set `RAWDENOISE_ASSETS` to use
  already downloaded assets.
- `scripts/bundle-macos.sh` always bundles the model: it uses
  `SHOPPHOTO_BUNDLE_RAWDENOISE_MODEL` (a compiled `.mlmodelc`) or installs the
  release once into `target/rawdenoise-model`.

## Validation

- `cargo run --release -p sp-ai --example eval_raw -- RAW [REFERENCE_RAW]`
  runs the model (strength from `SHOPPHOTO_AI_STRENGTH`, default 0.75) and
  `SHOPPHOTO_AI_DUMP=out.u16` writes the result frame.
- On the three release demo DNGs the Rust output matches the CLI's fp32 ONNX
  output with a 0.99997–0.99999 correlation of the denoise deltas; only
  highlights the CLI pushes above white differ, because ShopPhoto stores
  16-bit codes.
- A 50 MP X1D II frame takes 1.7 s and a 100 MP X2D II frame 3.3 s on Core ML
  (Apple Silicon).
- The model was trained for the X1D II; see its model card
  (`DenoiseModels/rawdenoise_r4_plain.MODEL_CARD.md`) for scope and limits.
