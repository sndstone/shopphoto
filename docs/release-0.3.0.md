# ShopPhoto 0.3.0 — enhancement preview

This version implements local AI selections, prepare/review/export batches,
and optional non-diffusion super-resolution.

The local macOS arm64 artifact is
`dist/ShopPhoto-0.3.0-preview.app`. It does not replace `/Applications/ShopPhoto.app`.

## Masking

**Tools → AI masks → Pick object / sky in image** creates an original-resolution
16-bit continuous alpha selection. SlimSAM provides the initial selection;
ViTMatte Small refines uncertain boundaries from original pixels in tiles.
Depth Anything V2 Small provides relative-depth ranges, with edge-guided
refinement. Depth values are relative within one image, not measured metres.

Use **Local & retouch** to adjust exposure, brightness, contrast and color only
inside the selection. **M** toggles the coverage overlay; **Paint + / Paint −**
repair the mask; feather, invert and undo are available. Inspect at 1:1.
Imported grayscale alpha PNGs must match the uncropped developed image size;
16-bit mattes preserve soft coverage. Mask pixels are shared by undo snapshots
in memory and losslessly delta/DEFLATE-compressed in sidecars. Earlier 0.3.0
prototype run-length masks can still be read. Older ShopPhoto versions cannot read alpha-mask
edits; do not open these sidecars in 0.2.x.

Automatic selection is **not guaranteed pixel-perfect**. Fine hair, wires,
glass, smoke, motion blur and mistaken foreground/background labels still need
inspection and repair. Sky selection is point-prompted segmentation, not an
automatic semantic sky detector. Matting cannot recover omitted objects well
outside its uncertainty band. Changing lens/geometry after creating a mask may
require regenerating it. Results computed against an edit that changes during
inference are discarded.

## Batch prepare → inspect → export

1. Select photos. If desired, use **Tools → Sync** to copy specific setting
   groups from the active photo first, including Detail & denoise. Auto Finish
   does not impose the active image's profile, white balance or crop on others.
2. **Tools → Batch → Precompute Auto Finish** computes Auto Balance separately
   for each image using its current profile/look and neutral global tone.
   Existing selected AI denoise runs first and its inference result is cached.
3. Saved results update filmstrip previews. Open each photo to inspect/edit;
   choose **Tools → Batch → Export selected** when satisfied.

The active edit is flushed before analysis. Each result retains annotations,
metadata, snapshots and a “Before Auto Finish 0.3.0” history entry. The save
worker checks for intervening edits; failures and extreme-key/low-contrast
scenes appear under Items requiring review. Cancellation stops between images
(existing denoise inference is not interrupted). Successfully saved files stay
saved. The status reports the number actually saved.

The Auto Balance algorithm remains a conservative histogram-based starting
point, not an aesthetically perfect auto-profile. There is no burst-wide
exposure harmonization or learned subject-aware grading in this release.

Prepared thumbnails are cached by source and sidecar revision. Inference
results use a separate, disposable 8 GiB LRU cache under
`shopphoto/inference-v1` in the OS cache directory, keyed by sensor pixels,
calibration, model selection and model-file revisions. Reopening/re-exporting
can reuse these results; eviction or changed models requires recomputation.

## Super-resolution

In **Export → Output size**, choose a DRCT, HAT or SwinIR RGB checkpoint
(`.pth` state dictionary or `.safetensors`) with native 2× or 4× scale. The
installed SwinIR 2× shortcut uses the official non-GAN classical-SR checkpoint.
DRCT/HAT are additional quality options, not a proven universal ranking.
Use only trusted checkpoints and review their licenses before redistribution.

Processing order: existing RAW denoise/develop → crop and requested resize →
AI upscale → output sharpening → decorations → encoding. Choose Original
working size to avoid downscaling before AI. The dimensions shown before AI
are multiplied by the checkpoint's native scale. A 150 MP output limit avoids
unbounded allocations; large original images may require cropping or a smaller
pre-AI size. Tiled inference bounds GPU activations, but output still uses host
RAM. Fine-texture seams and hallucinated detail need inspection. The worker
loads on demand and exits afterward; no diffusion model or always-resident
enhancement model is installed. Models currently reload for each export file.

SDR AI exports require sRGB. HDR exports preserve a Rec.2020/PQ master and use
an sRGB-converted guide for AI detail, bounded to ±0.20 EV luminance modulation
with black/specular attenuation. Master chromaticity is retained. This is an
**experimental HDR-preserving integration**, not a natively HDR-trained SR
model or a claim to reconstruct missing highlight detail. PQ PNG, HDR AVIF,
HDR JPEG XL and the existing Ultra HDR workflow retain their existing metadata
handling. Verify demanding HDR work on a calibrated display.

## Separate enhancement setup

From the repository, using Python 3.11 or newer:

```sh
python3 scripts/setup-enhance.py --upscale-swinir
```

On Windows use `py -3 scripts/setup-enhance.py --upscale-swinir` with a suitable
Python installation. Setup creates a separate `shopphoto/enhance-venv` under
the OS application-data directory, downloads revision-pinned public masking
checkpoints, and optionally installs a SHA-256-verified SwinIR 2× checkpoint.
Inference uses local
files only; photographs are never uploaded. Initial setup requires Internet.

macOS uses PyTorch MPS when available; Windows uses CUDA when a compatible
PyTorch/CUDA installation is present, otherwise CPU. This is not a Core ML or
WinML port. Windows/Intel Mac hardware parity remains unverified. Override
`SHOPPHOTO_ENHANCE_PYTHON` for a custom environment and
`SHOPPHOTO_ENHANCE_DEVICE=cpu` for CPU troubleshooting. The package's runtime
dependencies occupy disk space even though models are not resident when idle.

## UI and verification

Tools groups external editing, masks, batch and sync commands to reduce toolbar
crowding. Menus have explicit usable widths, export size controls wrap, and
the preset explanation wraps instead of clipping. UI checks were performed
with the computer-use skill on macOS, using a temporary RAW copy.

Automated coverage includes soft-alpha serialization/sampling, GPU masked
export, stale batch-save rejection, thumbnail revision invalidation, tiled SR
dimensions/seams, real SlimSAM/ViTMatte/depth inference, and real SwinIR SDR/PQ
output. The repository-wide Rust test command is
`cargo test --workspace --all-targets --locked`; some existing real-camera
tests are environment-gated and do not imply a full camera corpus run.

Optional model integration checks:

```sh
SHOPPHOTO_TEST_MASKS=1 SHOPPHOTO_TEST_SR=/path/to/SwinIR-M-x2.pth \
  /path/to/enhance-venv/bin/python scripts/test_enhance.py
```

## Primary references

- [SlimSAM checkpoint](https://huggingface.co/Zigeng/SlimSAM-uniform-77)
- [ViTMatte Small checkpoint](https://huggingface.co/hustvl/vitmatte-small-composition-1k)
- [Depth Anything V2 Small checkpoint](https://huggingface.co/depth-anything/Depth-Anything-V2-Small-hf)
- [Official SwinIR models and usage](https://github.com/JingyunLiang/SwinIR)
- [DRCT](https://github.com/ming053l/DRCT), [HAT](https://github.com/XPixelGroup/HAT)
- [Spandrel architecture loaders](https://github.com/chaiNNer-org/spandrel)

The earlier [research report](advanced-ai-features.html) contains candidate
architectures and mockups; it is not a statement that every research proposal
ships in 0.3.0. In particular, do not assume a DRCT-L 2× checkpoint is bundled
or validated, or that synthetic benchmark rankings establish “best” quality
for real RAW crops.
