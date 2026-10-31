# Native enhancement models

ShopPhoto 0.4.6 runs the subject, point-mask, edge-matting and faithful crop
super-resolution networks through a separate Rust/Core ML host on macOS 15+.
The personal DMG contains the validated compiled models. Camera RAW denoise
uses a separate runtime; see [ShopPhoto RAW Denoise](raw-denoise.md).

## Model inventory and provenance

- Sky: OneFormer ADE20K Swin Tiny (MIT), pinned revision
  `05f2812b1eccf9909b3897777450f8d68148cafc`. This is the shipped native sky
  model. The earlier, optional Python MaskFormer fallback is noncommercial
  and is not included in the 0.4.6 native model set.
- People, cars and motorcycles: torchvision SSDLite320 MobileNetV3 Large COCO
  proposals, followed by SlimSAM box-prompt segmentation. A semantic category
  query cannot reliably separate two instances, so it is not used for this path.
- Include/exclude points: SlimSAM encoder plus variable-length point decoder,
  supporting 1–32 real prompts. Padding unused prompt tokens materially changed
  masks during evaluation and was rejected.
- Boundary alpha: ViTMatte Small Composition-1K, guided by original full-resolution
  RGB and a conservative trimap. Only uncertain boundary tiles run the network.
- Crop enlargement: original non-GAN classical SwinIR DF2K 2× and 4× weights.
  These estimate detail; they cannot guarantee recovery of camera sampling.

Each bundle contains `manifest.json`, `LICENSE.txt` and a compiled `model.mlmodelc`.
The manifest pins source/revision, original checkpoint identity, explicit tensor
names/shapes, precision, preprocessing, minimum OS, tile/halo, conversion parity,
and SHA-256 for every compiled-model file. The loader verifies checksums before
opening a new session and bounds tensor sizes. Changed files invalidate sessions.
Legacy checkpoints only use a native port when their actual SHA-256 matches;
renaming another checkpoint does not substitute a different model.

Primary sources:

- [OneFormer model](https://huggingface.co/shi-labs/oneformer_ade20k_swin_tiny)
  and [MIT code license](https://github.com/SHI-Labs/OneFormer/blob/main/LICENSE).
- [MaskFormer model](https://huggingface.co/facebook/maskformer-swin-tiny-ade)
  and [CC-BY-NC-4.0 license](https://github.com/facebookresearch/MaskFormer/blob/main/LICENSE).
- [Official SSDLite model](https://docs.pytorch.org/vision/main/models/generated/torchvision.models.detection.ssdlite320_mobilenet_v3_large.html).
  The [torchvision BSD code license](https://github.com/pytorch/vision/blob/main/LICENSE)
  does not alone settle pretrained dataset conditions; torchvision explicitly
  [documents that distinction](https://github.com/pytorch/vision#pre-trained-model-license).
- [SlimSAM Apache-2.0 model](https://huggingface.co/Zigeng/SlimSAM-uniform-77).
- [ViTMatte MIT project](https://github.com/hustvl/ViTMatte).
- [SwinIR Apache-2.0 project and checkpoints](https://github.com/JingyunLiang/SwinIR).

## Runtime behavior

The host passes named Float32 tensors in process; no Python interpreter is
launched for these paths. Rust performs preprocessing, proposal decoding/NMS,
coordinate mapping, overlap tiling, matting composition and 16-bit quantization.
The canonical rendered input and final mask artifacts reuse the existing editor
cache/serialization paths; they are not a Python IPC protocol on native runs.
Sessions and image products are bounded and reused on the serial interactive
worker. Cancellation is checked between calls/tiles, and stale image/edit/model
results cannot overwrite a different document or selected layer.

Sky is a continuous mask. Other categories provide individual previews and
selected layers. The original image, geometry, layer adjustments and lossless
16-bit alpha remain reversible. Saved masks reopen without any model installed.
No-match and empty-result errors create no layer. Occlusions, glass, hair,
spokes and distant objects still require visual inspection and point/brush repair.
No general pixel-perfect or native-speedup guarantee is made.

SwinIR uses 192×192 RGB tiles with a 32-pixel halo and 128-pixel useful center.
Reflected image-edge context differs from the earlier clipped Python edge tiles;
interior parity is checked using the same padding contract. SDR uses sRGB.
HDR retains the Rec.2020 linear-light master, supplying an sRGB compressed guide
to the SDR-trained network and applying only bounded luminance detail to the
master before PQ encoding. Final output remains 16-bit/color managed. Requests
over 150 MP are rejected before output allocation.

## Building and packaging

`scripts/export-native-ai.py` and `scripts/export-native-selection.py` are development
tools. They use pinned checkpoints, restricted weights-only loading and exact
fixed-shape positional interpolation, never arbitrary remote model code. Native
runtime bundles are staged and parity checked before installation. See the
scripts' `--help` for task-specific inputs; model conversion dependencies are
not dependencies of the installed app.

`scripts/package-native-ai.py` verifies the complete selected set, licenses and
all graph checksums before copying. `bundle-personal-macos.sh` defaults to
`~/Library/Application Support/shopphoto/enhance-models/native`; override with
`SHOPPHOTO_BUNDLE_ENHANCEMENT_MODELS`. Native enhancement models are packaged separately from
[ShopPhoto RAW Denoise](raw-denoise.md).

At runtime, `SHOPPHOTO_ENHANCEMENT_MODELS` is an exclusive model-root override
for diagnostics. Otherwise user-installed bundles take precedence over bundled
resources. On older macOS or other platforms, supported optional Python paths
remain available; no native Intel/Windows/Linux performance is inferred from
Apple Silicon tests.
