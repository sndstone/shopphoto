# Native AI, subject adjustment layers and crop super-resolution

Research and implementation: 2026-10-15. The original research/plan is retained
below; [0.4.6](release-0.4.6.md) supersedes its pending runtime and UI status.

## Delivered in 0.4.6

Rust/Core ML now executes sky segmentation, real individual-object proposals,
SlimSAM box and variable-length point prompts, ViTMatte refinement and faithful
SwinIR 2×/4×. The personal package includes validated native models. People and
vehicle previews create separate selected adjustment layers; points can refine
an existing mask without losing its adjustments or undo. Crop provides direct
2×/4× choices, exact output previews and correct crop → AI → final size ordering.
See [native runtime/model details](native-ai-models.md).

Actual testing rejected two approaches: semantic MaskFormer queries merged two
people, so instances use a detector plus box-prompt segmentation; fixed padded
point tensors changed coverage, so the native decoder accepts 1–32 real prompts.
FP16 instability was rejected and validated Float32 graphs ship. Numerical
graph parity, real-photo mask inspection and export/seam/precision checks are
recorded separately. Depth/text and arbitrary additional checkpoints retain the
optional Python fallback. The larger labeled quality benchmark and original
user sky photo remain outstanding evidence, not claims of completed validation.

## Original research and implementation plan

## Requested workflow

Select **Sky / People / Cars / Motorcycles**, inspect the detected region, then
create a named adjustment layer. Exposure, white balance, color, curves and
detail controls affect that region only. Keep the original and all other regions
intact. Allow independent objects to have separate layers, including two people
or a rider and motorcycle. Provide overlay inspection, include/exclude points,
brush repair, inversion, layer amount, rename, duplicate, enable and undo.

After cropping, offer faithful 2×/4× AI upscaling with the cropped input size,
final pixel dimensions and megapixels visible before rendering. Save an export
or derivative; retain the original photograph and reversible crop.

## Existing foundation and first increment

The application already has losslessly stored full-resolution 16-bit alpha masks,
masked local adjustment controls, GPU mask evaluation, independent layer amount,
repair strokes, composition and undo. See [local masks](phase-local-masks.md).
It also already applies SwinIR/HAT/DRCT after geometry/crop and before final
output sharpening/decorations. Reuse these paths rather than adding a separate
image compositor or a second export renderer.

Implemented in the current source:

- One element picker for all four requested categories. Successful results
  become named local adjustment layers and automatically show their overlay.
  This first increment selects the union of a category, not an instance list;
  point selection remains available for choosing one object.
- Whole-label aliases support ADE20K's `minibike, motorbike` category. A car
  query cannot accidentally match `skyscraper` or `card`.
- The misleading sky-at-point entry is removed from the UI. That old worker
  route was generic point-prompt segmentation, not semantic sky detection.
- Python rejects empty, nonfinite or misaligned masks after matting and 16-bit
  quantization. Rust independently rejects empty/misaligned generated results
  before adding a layer. Imported blank masks remain usable for manual painting.
- Missing semantic weights report which runtime setup is needed. The current
  semantic backend remains the explicitly optional, noncommercial MaskFormer;
  the native replacement below is not silently enabled or downloaded.
- The crop HUD opens AI export at original crop working size, preserving a
  selected upscale model or choosing the installed SwinIR 2× checkpoint.
  The existing model chooser accepts supported native 2×/4× checkpoints.

The user's reported failure is an empty mask. The source previously checked
dimensions, but not whether generated coverage contained any nonzero pixels.
That acceptance gap is fixed. On the bundled 1672×941 landscape fixture, the
existing automatic sky backend produced nonempty continuous coverage over
27.95% of pixels, and people coverage over 0.97%; outputs were visually inspected.
These are smoke checks, not ground-truth accuracy or performance benchmarks.
The user's failing photograph has not been reproduced. Rejecting an empty
result prevents false success; it does not itself improve every model prediction.

## Open-source shortlist

The following are candidates supported by primary project/model sources,
checked on 2026-10-15. Model cards, model weights, conversion code and training
data have separate provenance; record the exact selected artifacts before shipping.

| Candidate | Use in ShopPhoto | Evidence and tradeoff |
| --- | --- | --- |
| OneFormer ADE20K Swin Tiny | Automatic sky/person/car/motorbike proposals; semantic and instance modes | [Publisher model card](https://huggingface.co/shi-labs/oneformer_ade20k_swin_tiny) lists MIT and all three segmentation tasks. [Official labels](https://github.com/SHI-Labs/OneFormer/blob/main/datasets/prepare_ade20k_pan_seg.py) contain the requested classes. Candidate for replacement; Core ML conversion/quality still to measure. |
| SAM 2 / SAM 2.1 Tiny or Small | Refine a detected instance with a box and positive/negative points | [Official project](https://github.com/facebookresearch/sam2) licenses code and checkpoints under Apache 2.0. [Hugging Face conversion](https://github.com/huggingface/segment-anything-2) and [Apple's Core ML Tiny weights](https://huggingface.co/apple/coreml-sam2-tiny) provide a native starting point. Promptable segmentation does not assign the four semantic classes by itself. |
| ViTMatte; BiRefNet matting for comparison | Hair, foliage, spokes, soft and transparent boundaries | [ViTMatte](https://github.com/hustvl/ViTMatte) is MIT and already part of the reference workflow. [BiRefNet matting weights](https://huggingface.co/ZhengPeng7/BiRefNet-matting) list MIT. BiRefNet foreground matting is not a class detector or automatic instance selector; compare before replacing a guided matte. |
| SwinIR classical/non-GAN 2× and 4× | Default faithful crop enhancement | [Official SwinIR](https://github.com/JingyunLiang/SwinIR) is Apache 2.0 and supplies classical, lightweight and real-world restoration variants. Existing 2× implementation/checkpoint gives a parity baseline; native export is still required. |
| Real-ESRGAN / RealESRNet | Optional degraded-image and texture mode | [Official Real-ESRGAN](https://github.com/xinntao/Real-ESRGAN) is BSD-3-Clause. [Native ncnn implementation](https://github.com/xinntao/Real-ESRGAN-ncnn-vulkan) proves Python is not required for this family. Its file CLI is not a drop-in 16-bit/HDR/color-managed backend. |

Additional options:

- [Grounding DINO](https://github.com/IDEA-Research/GroundingDINO) provides
  text-conditioned boxes under Apache 2.0. Combine with SAM 2 for future arbitrary
  object descriptions. It is not a pixel mask and should not be the sole sky
  solution; dense semantic sky avoids assuming sky is a bounded object.
- [SAM 3](https://github.com/facebookresearch/sam3) supports concept prompts,
  which makes it worth evaluating against the multi-model pipeline. It uses
  the [custom SAM License](https://github.com/facebookresearch/sam3/blob/main/LICENSE),
  not SAM 2's Apache license. Keep it a separately assessed option; do not
  describe it as an interchangeable permissive dependency.
- [Real-ESRGAN Core ML conversion](https://github.com/hanxiao/real-esrgan-coreml)
  is an additional conversion reference. Its published speed figures are not
  ShopPhoto measurements, and its notes explicitly caution against assuming
  that Core ML automatically places these models on the Neural Engine.

Recommendation: evaluate OneFormer for automatic classes, SAM 2 Tiny/Small for
interactive instances, and the existing ViTMatte for final boundaries. Start
native super-resolution with the exact existing SwinIR 2× weights to separate
runtime changes from changes in restoration style. Existing options are
sufficient to start; training a model from scratch is not the first step.

## Native runtime design

### macOS first

Use Rust orchestration with a narrow Core ML bridge. Model conversion may use
an isolated development Python environment; deployed native inference should
not launch Python or transfer temporary PNG/JSON files.

Keep RAW denoise and enhancement sessions separate. Add an enhancement runtime with named, typed,
shape-checked input/output tensors, strides, explicit errors and ownership.
Do not use arbitrary output ordering from `featureNames.anyObject`.

Proposed modules: `sp-ai/src/enhancement/{runtime,subject,matting,upscale}.rs`
and a separately cfg-gated Core ML bridge. Keep egui in `sp-app` and all
wgpu compute/copy work in `sp-gpu` / the existing renderer.

The model manifest records exact revision and SHA-256, original license and
notices, conversion version, tensor names/shapes, input transfer/color space,
normalization/resizing/padding, class mapping, native scale, tile halo, model
minimum OS and validated precision. Verify the actual SAM 2 variant/signatures
rather than relying on a filename or inconsistent card prose.

Use FP32 reference output and evaluate FP16, including thin edges; quantize
only after measured quality acceptance. Core ML's allowed compute policy is
not proof of per-layer GPU/NPU placement. Profile first-load, warm requests,
full-resolution refinement, peak memory and energy separately. Respect each
model's minimum OS rather than silently raising the whole app requirement.

### Other platforms and fallback

Use the ONNX Runtime C/C++ API behind the same Rust interface where graph export
and backend coverage pass. [Core ML EP](https://onnxruntime.ai/docs/execution-providers/CoreML-ExecutionProvider.html)
is an alternative to a direct Apple bridge; benchmark both rather than shipping
two redundant defaults. Use appropriate installed GPU providers or CPU fallback.
For new Windows work, assess WinML: [Microsoft's DirectML documentation](https://onnxruntime.ai/docs/execution-providers/DirectML-ExecutionProvider.html)
now places DirectML in sustained engineering and recommends WinML for new feature
development. [CUDA EP](https://onnxruntime.ai/docs/execution-providers/CUDA-ExecutionProvider.html)
is an option on supported NVIDIA systems. No Windows/Linux GPU result is claimed.

### Avoid new native bottlenecks

- Build the low-resolution neural input directly from the canonical developed
  source. Cache image embeddings by source generation, model revision, tensor
  preprocessing and pixel-changing edit identity.
- Produce one proposal pass for all four class choices. Retain instance IDs and
  regions; sky may be a single semantic region while people/vehicles stay separate.
- Run SAM only for selected or uncertain instances. Full-resolution matting
  processes boundary tiles with the original RGB and a trimap; do not independently
  segment tiles without global context or replace fine detail with a resized hard mask.
- Move resize/normalize, alpha compositing and proven edge filters into Rust or
  existing GPU compute. Pass typed tensors or buffers in memory, not encoded images.
- Classify `NoMatch` separately from corrupt-model/device failures. A missing
  subject must not create an empty layer or unnecessarily tear down healthy
  cached native sessions.
- Keep inference concurrency bounded per device; account for weights, embeddings,
  activations, staging and the editor's GPU image memory separately.
- Cancellation is cooperative between Core ML calls/tiles. Discard stale results
  after source/version changes; do not promise to interrupt an in-flight native call.

## Tool and layer integration

Next increment adds an instance preview list: **Sky**, **Person 1**, **Person 2**,
**Car 1**, **Motorcycle 1**. Selecting one previews its mask; **Create adjustment
layer** makes exactly one independent masked adjustment. Provide select-all by
class as an explicit alternative. A rider and motorcycle can be edited separately.

Retain full-resolution continuous alpha in canonical pre-lens coordinates.
Geometry is applied consistently to photo, mask, brush and export. Layer edits
must reuse the current retouch GPU pass. A saved mask renders without the model
being installed; reopening must not silently rerun AI or change past output.
Record model provenance separately from a user-editable layer name.

“Pixel perfect” is the quality objective, not a blanket model guarantee. Include
occlusion, hair, fences, branches, glass, motion blur and wheel spokes in review.
Offer include/exclude points, edge-aware brush correction, soft matte preview,
feather/edge shift and overlay/black/white inspection. Never turn low confidence
into an all-selected or silently empty layer.

## Crop and upscale design

The current shortcut exposes the existing post-crop model route. Next provide
**Off / Faithful 2× / Faithful 4×**, with a separate opt-in texture restoration
mode if Real-ESRGAN wins evaluation. Show exact dimensions before rendering and
provide a 100% before/after preview of a representative region.

Example: a 6000×4000 photo cropped to 1500×1000 has 1.5 MP. At 2× it produces
3000×2000 (6 MP); at 4× it produces 6000×4000 (24 MP). The extra pixels are
estimated detail, not a recovery of the original camera sampling.

Required order: developed source and local adjustments → final crop/geometry
at original working size → tiled super-resolution → exact requested output
resize if needed → output sharpening → attribution/borders → encoding.
Avoid downsampling to a target and then upscaling past it. The first increment
forces original crop working size only when opening the crop-specific workflow;
existing general export sizing behavior is preserved.

Use overlap halos with consistent padding, bounded output/storage buffers,
progress and cancellation. Reject requests above the existing 150 MP ceiling
before large allocations. 4× linear scale produces 16× as many output pixels.
Preserve 16-bit/color-managed output; HDR must retain the existing conservative
Rec.2020 luminance-detail path or pass a separately validated native equivalent.
Do not pass PQ codes through an SDR network or substitute an 8-bit CLI silently.

## Delivery stages and acceptance

1. **Immediate source increment — implemented.** Four-class selector, named masked
   adjustments, empty-result safeguards and crop-to-upscale entry point. Existing
   Python backend remains; no new weights downloaded and no new DMG built.
2. **Native feasibility — next.** Export/pin OneFormer, SAM 2, ViTMatte and SwinIR
   candidates into an isolated model directory. Run their actual graphs through
   the Rust native host. Compare output and end-to-end timings against current
   inference on identical inputs. Keep a Python fallback until parity is demonstrated.
3. **Native subject tools.** Instance selection, native boundary refinement,
   cached embeddings, diagnostics and model installer manifests. Test real user
   empty-sky failures, not only the bundled demonstration scene.
4. **Native crop enhancement.** Direct 2×/4× choices, tiled ROI preview, exact
   final sizing and faithful/texture comparison. Validate crop order, ICC, HDR,
   seams, peak memory and source preservation.
5. **Packaging.** Universal build and mounted acceptance with actual installed
   model/backend availability, both missing-model and unsupported-OS behavior,
   sidecar reopen/undo and a real photo export. Rebuild a new personal DMG only
   after the corresponding changes and model conversions pass.

Quality benchmark: create a held-out, legally usable set of skies (including
white skies/snow/water), hair/portraits, cars/windows/reflections and motorcycle
rider/spoke scenes. Use reference masks to report class/instance IoU and boundary
F-score plus alpha SAD/gradient errors on matte cases. Inspect at 100% and 200%;
measure missed-object and empty-result rates separately. Smoke tests establish
execution, not segmentation accuracy. Native parity, quality acceptance and
speed/peak-memory measurements are all separate gates.

Super-resolution benchmark: real camera crops, naturally degraded images and
controlled downsampling pairs; check PSNR/SSIM where references exist, edge/text
and face fidelity, color differences, seams and oversharpening. Compare at equal
final dimensions. Report cold/warm end-to-end timings on the target Mac; no
unmeasured speedup or Neural Engine guarantee is promised.

## If existing models do not meet quality

Do not start by training a foundation model. Fine-tune a compact four-class
proposal/instance model (four target classes plus background) or boundary
refiner on licensed photographs and corrected
masks, using existing larger models only as optional annotation proposals.
Human-review pseudo-labels and keep capture sequences out of both train and test.
Retain high-resolution hair, foliage, glass and spokes rather than downsampled
binary labels. Distill to a native-friendly student after the reference model
passes the held-out set. Assess code, weights, source data and teacher conditions
independently. Scope compute and costs only after determining the measured gap.

## Validation record

The all-feature workspace passed 583 tests (zero failures, seven explicitly
ignored); strict all-target/all-feature Clippy, formatting and whitespace checks
passed. The new ignored crop test was explicitly invoked with the actual installed
SwinIR 2× model and passed. The running subject and crop UIs were captured and
reviewed; no QA sidecars were written. Python passed 23 cases with two
portrait-dependent skips. The Python
suite exercises class aliasing, empty/nonfinite/misaligned final masks and all
four class requests using the same cached semantic prediction. The bundled
landscape does not establish positive car/motorcycle quality; dedicated real
fixtures are still required. Final workspace and real crop-upscale test results
are recorded in PROGRESS.md.
