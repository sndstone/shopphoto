# ShopPhoto 0.4.6 — native subject layers and crop enlargement

This release turns AI selection into an element workflow: choose Sky, People,
Cars or Motorcycles, then edit only that element on a reversible adjustment layer.
It retains the edit-save, image-correctness and network-sync fixes from 0.4.5.

## Changes

- Native Rust/Core ML inference for MIT-licensed OneFormer sky, individual-object proposals,
  promptable segmentation, full-resolution edge matting and SwinIR 2×/4×.
  The personal package includes the validated enhancement models; these paths
  do not require the optional Python environment. Native enhancement needs macOS 15+.
- People and vehicle detections show individual previews. Choose one or several
  to create separate named adjustment layers. Semantic category unions are not
  mistaken for individual objects.
- Empty, nonfinite or misaligned generated masks are rejected before layer
  creation. Image/edit generations and model provenance guard asynchronous
  results and selected preview IDs.
- Include/exclude points can refine an existing selected alpha layer, preserving
  its tone, color, detail, opacity and other layers. Undo/redo restores the mask.
  New or refined masks jump directly to the selected layer's local controls.
  Full-resolution 16-bit alpha remains editable and survives sidecar reopening.
- Crop offers direct faithful 2×/4× enlargement, with cropped input, intermediate
  and final dimensions visible. Processing order is crop → AI → requested final
  size → sharpening/decorations. The 150 MP intermediate limit is checked early.
- Native tensors have explicit names and checked dimensions/strides. Model
  files are pinned and verified; model caches stay separate from RAW denoise.
  Python remains available for depth, text selection and additional checkpoints.

## Quality and validation

Real-model acceptance covers separate people in one image, cars, motorcycles,
sky, include/exclude prompts, saved masks, native 2×/4× SDR/HDR, cropping and exact
final sizing. Numerical conversion parity, output precision and tile seams are
checked independently of segmentation quality. FP16 output instability and
fixed-padding prompt regressions found during evaluation were rejected.

Difficult occlusions can include unwanted foreground detail; small distant
objects can return no match. Inspect the overlay and use points or brush repair.
The user's original empty-sky photograph was not available for reproduction.
No model can promise universally pixel-perfect masks or reconstructed true detail.
Native inference eliminates the Python runtime for the listed paths; its speed
depends on image shape, selected model and hardware, and is not uniformly faster
than Metal/PyTorch in the measured small-crop comparisons.

The final all-feature workspace passed **595 tests, zero failures and 12 explicit
ignores**. Strict all-target/all-feature Clippy, formatting and whitespace checks
passed. Python passed 28 real-runtime cases with two portrait-fixture skips.
The ignored native subject, dynamic prompt, sky, crop and SR parity tests were
explicitly run with real models. Native photo SR matched the identical Torch
tile contract within 1/65535 for SDR and 53/65535 for HDR; mean HDR error stayed
below 0.174/65535. Tile seams and genuine 16-bit output were checked. Integrated
2×/4× crop exports and exact final sizing passed with Python unavailable.

The running GUI detected two separate people using only native models, and
sky/people/car/motorcycle overlays were visually inspected. OneFormer selected
27.97% sky coverage in the bundled scene. These are functional and visual checks,
not a labeled generalization benchmark. The package contains eight native
enhancement bundles, preferring MIT OneFormer and excluding noncommercial
MaskFormer. Every graph file and upstream license notice is verified.
See [model provenance and runtime details](native-ai-models.md).

## Package

The personal package contains universal arm64/x86_64 code, native enhancement
models. It uses ad-hoc signing unless signing credentials are supplied and
is not notarized by default. Native codec OS requirements from 0.4.5 remain.
The installed application and previous DMGs are preserved by the build.

## Final artifact verification

- `dist/ShopPhoto-0.4.6-personal.dmg`: **1,192,828,051 bytes** (about 1.11 GiB).
- SHA-256: `701f7fec2b9d0de245dc69ba79cfdc9a519663141defddde3b47f48c9a7f357a`.
  The adjacent basename-based checksum file was verified independently.
- Final DMG integrity, deep strict signature, version and both arm64/x86_64
  slices passed. Signing is ad-hoc; this package is not notarized.
- All native enhancement bundles (eight total) and 48 pinned files, including licenses,
  passed mounted verification; noncommercial MaskFormer is absent. Actual
  mounted-model inference passed sky, separate people, cars, motorcycles,
  variable-point refinement and native crop/SR with Python unavailable.
- The final mounted GUI was reviewed: detections show distinct previews, and a
  generated sky layer opens its own Local & retouch controls directly. Native
  1/4-point refinement retained 98.9% of the chosen-person test ROI and 0% of the
  excluded-person ROI. These fixture metrics are not a general accuracy claim.
- Mounted CLI crop exports produced 128×96 → 256×192 at 2× and → 512×384 at 4×;
  subsequent exact sizing produced 192×144 in both cases. 16-bit pixels and
  center color were preserved, with no pixels from outside the crop entering AI.
- PNG/JPEG/AVIF/HEIC rendering, 16-bit resized export, JSONL edits/export/undo,
  explicit sidecar save, overwrite-original refusal, MCP preview, Rec.2020 ICC
  independent CMS checks and Display P3 Ultra HDR metadata passed.
- The six bundled codec libraries loaded without external Homebrew paths.
- Disposable source hashes and expected sidecar absence were checked. The QA
  image was detached; installed ShopPhoto remains 0.4.4, earlier DMGs remain,
  and source work is uncommitted. Physical Intel/other-platform and the user's
  original failing sky-photo acceptance are not claimed.

The DMG's enclosed notes precede these final mounted checks; this repository
record and the adjacent checksum describe the final artifact.
