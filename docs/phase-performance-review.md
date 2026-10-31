# Further performance review — 2026-10-10

This is an investigation following the AI worker/cache implementation. No
additional application optimization or installer was shipped in this phase.
The strongest measured next target is export preparation, followed by reuse
of the canonical image supplied to interactive AI masks.

## Measured export costs

Apple M5 Pro, 24 GiB RAM, optimized ARM64 Rust. An isolated offline Cargo
harness calls the current `sp-export` source; three repetitions per timing.
These are synthetic RGBA fixtures, not decoded photographs. Measurements
exclude RAW decoding, GPU development, model inference and file encoding.

| Operation | Median | Scope |
| --- | ---: | --- |
| Original-size, neutral 4000 × 3000 preparation | 0.761 s | `prepare_image_already_converted`, default geometry/settings |
| Original-size, neutral 1200 × 800 preparation | 0.0609 s | Same API and defaults |
| 1200 × 800 → 600 × 400 preparation | 1.726 s | Same API with a 600-pixel long edge |
| Existing resize stage alone | 1.660 s | Current private `resize_linear` copied verbatim into the harness |
| Cached colour/filter prototype | 0.0367 s | Same sampling and accumulation order; 45.17× stage speedup |

An earlier independent run measured 0.798 s for the 12 MP neutral preparation
and 1.771 s for the resized preparation. The resize-stage comparison used a
different synthetic pixel pattern from the complete-preparation comparison;
do not subtract their times to claim a measured total-job improvement.

The prototype predecodes input channels into linear light once and precomputes
Lanczos weights for each output axis. It retains the original nested weighted
sum and accumulation order. The existing implementation repeatedly computes
weights and calls a four-pixel bilinear sampler at integer tap positions,
including repeated transfer-function evaluations.

The measured 8-bit result matched exactly for all three repetitions. Six
additional odd-sized checks (513 × 341 → 173 × 113, each of 8/16-bit sRGB,
Adobe RGB and PQ) matched both output buffers exactly. These checks establish
feasibility, not general image-quality acceptance. The prototype stores the
entire linear input: a production version needs bounded row/strip caches to
avoid adding roughly 1.6 GB of temporary float storage for a 100 MP image.
Crop, alpha, very small dimensions, scaling ratios and transfer-function
boundaries still need broader equivalence checks before adoption.

Local reproduction artifacts are `/tmp/shopphoto-export-profile/`,
`/tmp/shopphoto-export-profile.log` and
`/tmp/shopphoto-export-resize-prototype.log`. They are temporary research
artifacts; the prototype is not linked into the application. Its standalone
Cargo release profile uses optimization level 3; it does not inherit the
application workspace's thin-LTO release profile.

## Recommended implementation order

1. **Faster export preparation.** Optimize the existing linear-light resizer
   with bounded decoded-row and axis-weight caches. Then use ownership or
   borrowing to avoid full-image copies when size/sharpening are unchanged.
   `geometry` currently resamples even default geometry; `resize_linear` and
   `output_sharpen` clone on their no-op paths. The 12 MP identity fixture
   changed 2,647 channel bytes in the geometry path, so an identity fast path
   also needs an explicit decision and tests about preserving original
   pixels rather than blindly retaining resampling roundoff. One 100 MP
   RGBA8 + RGBA16 buffer pair is 1.2 GB in decimal units, before other copies.
2. **Reuse canonical AI input.** `start_mask` currently creates a renderer,
   uploads the source and renders the entire canonical full-resolution image
   for every prompt, then writes a new temporary RGBA8 file. The new Python
   inference-product caches do not remove this Rust-side work. Retain one
   byte-bounded canonical input file keyed by source generation, denoise
   revision, normalized render edit and resolved profile/look/lens state.
   Prompt changes and local-mask additions should reuse it; pixel-changing
   edits must invalidate it. Release it on idle, cancellation and shutdown.
   Keep full-resolution edge refinement. No latency gain was measured here.
3. **Overlap export tile readbacks.** `render_export` processes 1024-pixel
   tiles serially. Each readback allocates a staging buffer and
   `GpuContext::map_read_buffer` waits for the GPU before the next tile.
   Investigate a small reused staging ring that queues the next tile while
   collecting a previous transfer. Copy each tile into staging before
   reusing its output textures; preserve apron trimming, cancellation,
   timeout/device-loss checks, output coverage validation and bounded memory.
   Histograms already use asynchronous readbacks and are not this bottleneck.
   wgpu documents asynchronous mapping and the required mapped/unmapped
   resource lifecycle in its [buffer documentation](https://docs.rs/wgpu/latest/wgpu/struct.Buffer.html).
   Actual benefit needs GPU timing on each supported backend.
4. **Reuse unaffected preview stages.** A changed edit runs the proxy's
   white balance, profile and enabled noise-reduction stages again before
   tone and display processing. Cache an intermediate image with explicit
   stage dependencies so a tone-only change can reuse earlier work. Also
   investigate stable uniform buffers and bind groups instead of recreating
   them per dirty render. Preserve profile/proof, denoise and local-mask
   invalidation and prove cached/fresh equivalence. Tile/scopes work is
   already deferred during interaction. No speedup was measured here.

Native enhancement inference and reduced precision remain subsequent
experiments. The existing Float32 Core ML depth probe improved model-forward
latency by only 9.6% for one static shape. That does not justify prioritizing
a full runtime migration above the measured export work. Tile batching also
remains opt-in after the earlier neutral measurements.

## Validation and delivery state

The isolated benchmark built and ran successfully; the measured resize
comparison and all six additional parity checks passed. Application Rust,
Python, dependencies, model bundles, version and DMG were not edited during
this review. The prior implementation's workspace validation remains recorded
in [the AI performance phase](phase-ai-performance.md); no new application
test run is being claimed for these research-only findings.
