# ShopPhoto — Progress Report / Handoff Document

> **Read this first if you are picking up the work.** BUILD.md is the full
> specification (phases, architecture, acceptance criteria). This file tracks
> what is actually done, exact environment facts, and the next concrete steps.
> Keep it updated at the end of every work session.

> **Current roadmap:** [`docs/photography-roadmap.md`](docs/photography-roadmap.md)
> tracks the user-approved photography expansion; [`docs/review-improvements.html`](docs/review-improvements.html)
> retains the earlier release review. This file remains the
> chronological engineering handoff and should not be read as the open-task list.

**0.5.0 ShopPhoto RAW Denoise (2026-10-31):**
ShopPhoto's own `rawdenoise-lab` `r4_plain` model provides neural RAW
denoise, offered for every Bayer RAW and bundled in every macOS build
(`scripts/bundle-macos.sh` installs the pinned release into
`target/rawdenoise-model` or takes `SHOPPHOTO_BUNDLE_RAWDENOISE_MODEL`).
`sp-ai` follows the reference tool: mirrored 60-px apron,
452-px stride, ramp^2.2 feather and the ported defect pre-pass
(`sp-ai/src/defects.rs`); strided/Float16 Core ML output (50 MP 11.9 s →
1.7 s), single-input name lookup, a pinned in-app downloader that compiles the
Core ML package on the Mac and `scripts/install-rawdenoise-model.sh`. Rust
output matches the reference CLI on its demo DNGs (delta correlation
0.99997–0.99999). tract moved to 0.23.9 for the model's two-input Pad; the
non-macOS ONNX path was checked against tract 0.23.9 in a scratch crate but
not cross-compiled. See `docs/raw-denoise.md`.
Launch fix: `detected_import_sources` walked every `/Volumes` entry three
levels deep on the main thread (the `Macintosh HD` link and SMB shares: 33.5 s
before the first frame). It now skips symlinks, the startup disk and network
mounts (macOS `MNT_LOCAL`, Linux network filesystem types) and runs on a
background thread, also for Import from card ("Looking for cards…"). Scan
33.5 s → 0.03 s, a FAT test card with `DCIM` is still found; launch to first
frame 40.1 s → 3.1 s (incl. 2.5 s screenshot settle).
Workspace tests, formatting and clippy (two pre-existing `preset.rs`
warnings) pass. See `docs/release-0.5.0.md`.

**0.4.9 Golden Hour and Neon Night presets (2026-10-25):**
Added two creative presets (13 built-ins total). Only enabled HDR receives
preset highlight-rolloff changes; enabled/target/headroom/scene white remain
unchanged, with exact preservation of disabled HDR settings. Selection,
hover and thumbnail rendering share the current HDR baseline; inactive
presets cannot restore stale edits after reset. Amount keeps its saved
baseline. Dutch labels and the research preset table updated. Core tests:
148 passed/1 ignored; five targeted app tests and formatting passed.
Universal standard DMG `dist/ShopPhoto-0.4.9.dmg` built successfully;
DMG integrity, SHA256, deep strict app signature, both architecture slices,
version and mounted documentation verified. Ad-hoc signed, not notarized.
The new looks still need visual calibration on representative golden-hour
and neon photos and a physical HDR display. See `docs/release-0.4.9.md`.

**0.4.8 file compatibility, library states and audit fixes (2026-10-22):**
HEIC/HEIF/AVIF via macOS ImageIO (`sp-raw` `raster/imageio.m`), `IMG.ARW.xmp` and
`.XMP` sidecars, raster formats from Finder/launch, multi-file drop, specific
messages for X-Trans RAF, linear DNG and reduced ARW, XMP rating/label on export.
Library "No preview"/"File missing" tiles, middle-elided names, labelled
metadata and colour labels, WB status row, folder sidebar icons. Codex audit
fixes (sidecar sync direction, no-clobber exports, profile validation, streamed
model downloads, sidecar quarantine). Workspace 619 passed/13 ignored, clippy
clean. Personal DMG `dist/ShopPhoto-0.4.8-personal.dmg`, 1,192,538,707 bytes,
SHA256 `538e17c880f750d9ce43377e21ac6213b80c5f017199d378222d680f46094f00`;
universal, ad-hoc signed, not notarized; DMG checksum, deep strict signature,
version, 8 native enhancement bundles and an installed-app Finder HEIC open
(24.5 MP, 820 ms decode) passed. The first launch after install paused about
30 s before the first frame; later launches did not. See `docs/release-0.4.8.md`.

**0.4.7 guided AI selections and halo-free layer clarity (2026-10-15):** AI rail
status card plus per-method numbered steps; detected elements tinted, numbered
and clickable in the preview; pink mask overlay with naming banner; segmented
Presets/AI/Workflow, method and denoise controls. Layer clarity
(`shaders/retouch.wgsl` `guarded_mean`) now uses perceptual lightness with
edge- and coverage-weighted taps, removing finger/sleeve/collar rims; the native
matte itself was verified tight (`native_person_edges` ignored test). sp-app 106
passed/11 ignored, sp-pipeline all suites passed. Personal DMG
`dist/ShopPhoto-0.4.7-personal.dmg`, 1,192,791,680 bytes, SHA256 `dff0c332ffa619430d72accdbd4c262a9aee9e64373b09d5039e39216a36cc78`;
universal, ad-hoc signed, not notarized; integrity, checksum, deep strict
signature, version, 8 native bundles and a mounted-app native people-layer run
passed. Open: global clarity halos, 33 px matting band at 100 MP, 1536 px
guided-refinement bake. See `docs/release-0.4.7.md`.

**0.4.6 native subject layers and crop enlargement (2026-10-15):** completed
Rust/Core ML OneFormer sky (MIT), SSDLite individual people/cars/motorcycles,
SlimSAM box and flexible 1–32-point segmentation, ViTMatte boundary alpha and
faithful SwinIR 2×/4×. New personal packaging verifies and includes eight native
enhancement bundles with full upstream licenses, pinned source/revision and all
file hashes; optional noncommercial MaskFormer is excluded. Native workflows execute with
Python deliberately unavailable; depth/text/additional checkpoints retain the
optional Python path. Distinct previews generate separate selected layers;
point refinement preserves adjustments and undo. Final GUI review added a direct
scroll to the selected Local & retouch card, preventing accidental global edits.
Guards reject empty/misaligned
results, stale image/edit/model requests and oversized allocations. Expected
no-match retains healthy caches. AI selections now appear first in the AI rail.
Crop has direct 2×/4×, accurate intermediate/final dimensions, and crop → AI →
exact final sizing → sharpening/decorations. Actual native real-photo acceptance
passed two separate people, cars, motorcycles, 1/4-point keep/exclude and saved
mask roundtrip. OneFormer sky+matte passed at27.97% coverage; native SR2x/4x
SDR/HDR, seams/16-bit precision and exact crop sizing passed. Same-tile Torch
parity max1u16SDR /53u16HDR (meanHDR<0.174u16). Native timings vary and were
slower than variable-edge Python on a small crop; no universal speed claim.
Fixed padded prompts materially changed masks and FP16 produced nonfinite
outputs; both rejected. Final workspace **595 passed,0failed,12ignored**;
strict all-target/all-feature Clippy, formatting/diff checks passed. The final
inspector-focus fix additionally passed all104app tests and strictClippy. Python
**28passed,2portraitskips**. Logs `/tmp/shopphoto-046-final-{workspace,clippy}.log`,
`/tmp/shopphoto-native-prompt-review.log`, `/tmp/shopphoto-046-oneformer-actual.log`,
`/tmp/shopphoto-crop-upscale-no-python.log`, `/tmp/shopphoto-native-sr-parity/`.
See `docs/release-0.4.6.md`, `docs/native-ai-models.md` and
`docs/native-ai-conversion.md`. Universal personal packaging and final mounted
checks are complete; artifact record follows below. Difficult occlusions still need repairs; original
user sky failure photo and labeled held-out quality benchmark remain unavailable.

**0.4.6 final personal DMG:** `dist/ShopPhoto-0.4.6-personal.dmg`,
1,192,828,051bytes, SHA256
`701f7fec2b9d0de245dc69ba79cfdc9a519663141defddde3b47f48c9a7f357a`.
Universal x86_64/arm64, ad-hoc signed, not notarized. DMG integrity, checksum,
deep strict signature, mounted version and 8 native bundles/48 hashed files passed.
Actual final mounted native models passed
individual people/car/motorcycle selection, dynamic1/4point refine, savedalpha;
mountedGUI nativeSky/People workedwithoutPython and inspectorfocusfix reviewed.
MountedCLI native2x/4x cropfull/exactoutputs, PNG/JPEG/HEIC/AVIF, ICC/UltraHDR,
JSONLedit/export/undo, explicit sidecarsafety and MCPpreview passed. No QA original
changed or unwantedsidecarwritten. Logs `/tmp/shopphoto-046-final-{mounted-qa,
gui,native-acceptance,image-verify}.log`. QA root path is recorded in
`/tmp/shopphoto-046-qa-path`. Mount detached. Installedapp preserved0.4.4,
olderDMGs preserved, no commits/pushes/externalrepositorywrites.

**Subject tools and native-AI research (2026-10-15, earlier source increment):** the user
reported empty sky masks and requested sky/people/car/motorcycle adjustment
layers, native inference and crop super-resolution options. Added one four-class
selector and named masked adjustments, whole-label motorcycle aliases, removal
of the misleading generic sky-at-point UI entry, Python post-matting/quantization
validation and independent Rust generated-mask checks. Empty masks now fail
visibly without creating a layer; valid empty imports remain supported. Added
crop-HUD AI export entry preserving original crop working size, selected model
or installed SwinIR 2×. Actual crop/SwinIR regression confirms 128×96 → 256×192,
without red pixels outside the crop entering inference. GUI screenshots of both
new entry points were reviewed; QA image sidecars were not written.
Primary-source research recommends evaluating OneFormer, Core ML SAM 2 and
ViTMatte, plus native SwinIR conversion; Real-ESRGAN is an optional texture
alternative. Full native migration and detected-instance lists are planned,
not implemented. Current selection still uses the optional noncommercial
MaskFormer/Python backend. The bundled landscape produced a valid sky mask;
the user's failing photograph and positive car/motorcycle fixtures remain
unverified. No universal pixel-perfect or unmeasured speed claim is made.
Workspace: 583 passed / 0 failed / 7 ignored; explicit new crop-upscale test
passed, strict all-target/all-feature Clippy, formatting and whitespace clean.
Actual cached-model Python suite: 23 passed / 2 portrait-fixture skips.
Research and staged implementation: [phase-native-subject-tools.md](docs/phase-native-subject-tools.md).
Existing 0.4.5 DMG and installed app were not rebuilt/replaced in this increment.

**0.4.5 personal DMG completed (2026-10-15):** packaged all reliability and
image-correctness fixes from the gpt-6.1-sol review as
`dist/ShopPhoto-0.4.5-personal.dmg`, preserving earlier DMGs and the installed
0.4.4 app. Universal arm64/x86_64; six relocated native codec libraries; ad-hoc signed, not notarized.
Fresh all-feature workspace: 582 passed / 0 failed / 6 ignored; strict all-target
all-feature Clippy, formatting, whitespace, packaging shell/plist checks passed.
Mounted acceptance passed integrity, deep strict signature, architectures,
native PNG/JPEG/HEIC/AVIF rendering,
resized 16-bit PNG, actual Rec.2020 ICC/LittleCMS comparison, Display P3 Ultra HDR,
explicit sidecar save, original-overwrite refusal, JSONL edits/export/undo and
MCP preview passed. The GUI rendered edited controls at 100% zoom, captured a
reviewed SDR screenshot and exited normally. Packaged `--hdr-check` reported
PASS with Rgba16Float/extended-linear-sRGB and ×16 potential panel headroom;
physical by-eye HDR acceptance remains separate. Disposable originals retained
their hashes; GUI/MCP/render checks did not create their sidecars.
DMG size: 703,864,515 bytes (671.3 MiB).
SHA-256: `118b4857d0ef596973d1d43e6a296918312fb082fc40cba2374b6d5ca89005a0`;
adjacent basename-based checksum verified. Mounted QA artifacts are in
`/var/folders/32/7t53k4nj4r9gr3f5x2syfc_80000gn/T/shopphoto-045-qa-lkm_m9h3`;
build/test logs use `/tmp/shopphoto-045-*`. Mount detached after acceptance.
Details: [release-0.4.5.md](docs/release-0.4.5.md).

**Extensive gpt-6.1-sol reliability review (2026-10-15):** three subagents
reviewed catalog persistence, image/export correctness and app workflows, followed
by cross-review and bounded follow-up fixes. Fixed outgoing-image edit leakage,
forced-save failure handling, reversed Auto Finish network sync, concurrent
sidecar staging collisions, settings-sync preflight/undo, truncated source-cache
reuse, Rec.2020 ICC transfer mismatch and wide-gamut Ultra HDR luminance. Added
RAW metadata/preview panic recovery and geometry validation, finite matrix
inversion, raster/sequence input checks, GPU transfer validation, safe AI blend
guards and Python worker protocol recovery. Existing modified/untracked work
preserved; source-only changes, no package/install update or publication.
Final all-feature/native-codec workspace: **582 passed, 0 failed, 6 ignored**.
Strict all-target/all-feature Clippy, formatting and whitespace checks passed.
Actual cached-model Python suite: **21 passed, 2 portrait-fixture skips**;
three Rust/Python worker acceptance tests explicitly invoked and passed.
Physical camera/RAW-fixture, HDR, printer, network-disconnect and cross-platform
GUI acceptance remain outside this pass. Existing dependency future-compatibility
notice: `proc-macro-error2` (strict Clippy still passes).
Details: [phase-review-2026-10-15.md](docs/phase-review-2026-10-15.md).

**0.4.4 personal DMG completed (2026-10-10):**
`dist/ShopPhoto-0.4.4-personal.dmg` includes live zoomed control updates and
saved Camera RAW compute policies under Tools → Settings. Universal
arm64/x86_64, ad-hoc signed, 670.6 MiB (703,131,024 bytes); not notarized.
Native-codec workspace 554 passed / 0 failed / 6 ignored; strict all-target
Clippy, formatting and diff checks passed. Mounted package passed integrity,
deep strict signature, relocated codec loads, JPEG/HEIC/AVIF, 16-bit resized
PNG and JSONL/MCP. Mounted GUI at 100%
zoom captured and reviewed; temporary source/sidecars preserved. QA mount
detached; installed app remains 0.4.1 and older DMGs retained.
SHA-256 `af7aaf6106aa6a994e81a528942ae6a1bd29e4101534f2835d25d240362c337d` (adjacent checksum verified).
Details: [release-0.4.4.md](docs/release-0.4.4.md).

**Post-0.4.3 live preview and Camera RAW hardware preferences (2026-10-10):**
zoomed full-resolution tiles and before/after comparisons update during control
drags; pure viewport gestures retain deferred tile coverage. Tools → Settings…
now saves Automatic / GPU + CPU / Neural Engine + CPU / CPU-only RAW policies.
Explicit per-request policies reach interactive, batch Auto and export workers,
and invalidate both retained sessions and disk/in-memory inference caches.
Workspace 550 passed / 0 failed / 6 ignored;
real held-pointer GPU regression, English/Dutch settings layouts, strict Clippy,
format/diff and Intel compilation checked. Existing 0.4.3 DMG and installed app
unchanged. Details: [phase-live-preview-compute.md](docs/phase-live-preview-compute.md).

**0.4.3 performance implementation (2026-10-10):** bounded linear-light
resizing, owned export handoffs/no-op bypasses, canonical AI input reuse,
two-slot GPU readbacks and a 64 MiB preview-prefix cache are implemented.
The same small export-preparation fixture improved from 1.726 to 0.0940 s;
NR-enabled proxy tone edits measured 11.59 to 1.45 ms and two-slot 12 MP
exports reduced readback/development latency by 38–39% against serial slots.
Native workspace: 547 passed, zero failed, six explicit ignores; all three
real-worker acceptance cases passed; Python: 20 passed, two portrait skips.
Strict native all-target Clippy, formatter and whitespace checks passed.
See
[`docs/phase-performance-implementation.md`](docs/phase-performance-implementation.md)
and [`docs/release-0.4.3.md`](docs/release-0.4.3.md). Personal DMG packaging
and mounted acceptance passed: `dist/ShopPhoto-0.4.3-personal.dmg`, 703,002,354
bytes (670.4 MiB), SHA-256
`6b1197e0218a7fb54d6c0379e81fb5511dc29b6a4b23740ae06a6600033d919e`.
DMG integrity, mounted deep strict signature, version/universal slices, six relocated
codec libraries, JPEG/HEIC/AVIF, resized 16-bit PNG and JSONL/MCP preview/edit/
export/undo checks passed. Original hashes and sidecars were preserved. The
installed app remains 0.4.1; earlier DMGs remain in place. The package is ad-hoc
signed, not notarized. ARM native codecs require macOS 27.0; the Intel slice
retains standard codecs/macOS 12.0 and was compiled rather than physically run.

**Further performance review (2026-10-10):** measured an export-preparation
bottleneck in optimized ARM64 Rust. Neutral 12 MP preparation took 0.761 s;
1200 × 800 → 600 × 400 preparation took 1.726 s. An isolated resize prototype
reusing decoded colours and filter weights reduced that stage from 1.660 to
0.0367 s (45.17×), with identical measured output and six additional 8/16-bit
sRGB/Adobe RGB/PQ parity checks. It needs bounded row caches and broader quality
acceptance before production use. Further targets are canonical AI input reuse,
overlapped export tile readbacks and unaffected preview-stage reuse. No additional
application or package changes were made. See
[`docs/phase-performance-review.md`](docs/phase-performance-review.md).

**AI performance implementation (2026-10-10):** interactive masks now use an
app-owned serial Rust executor with process reuse, idle release and cancellation
cleanup. Neural weights and content/revision/device-keyed image products are
bounded separately; point prompts, depth thresholds and semantic classes reuse
image inference. GUI masks use validated mapped RGBA8 input and release temporary
render/source buffers before inference. Results reject stale source generations.
Isolated M5 Pro/MPS medians improved from 0.780 to 0.351 s for point masks and
0.160 to 0.0447 s for depth masks; compared 16-bit outputs matched exactly.
Tile batching remains opt-in after neutral benchmarks. A separate Float32 Core ML
depth prototype exported successfully and reduced model-forward latency by 9.6%;
it is not a production backend. Full native workspace passed 537 tests, zero
failures and five explicit ignores; both new real-worker bridge cases passed
when explicitly invoked. Python passed 20 cases with two portrait-dependent
skips. Strict Clippy, formatter and whitespace checks passed. The source/local
development build is updated; the existing 0.4.2 DMG is unchanged. See
[`docs/phase-ai-performance.md`](docs/phase-ai-performance.md).

**AI performance investigation (2026-10-10):** identified per-action interactive
worker teardown and repeated SAM image encoding. On M5 Pro/MPS, the 1672 × 941
fixture's text mask worker took 2.68 s on its first request and 0.51 s warm;
a temporary retained-embedding prototype reduced the warm complete Python
point-mask step from 0.82 s to 0.35 s with identical 16-bit output. These exclude
Rust rendering and are not large-RAW or cross-platform guarantees. No runtime
implementation or package changed. See
[`docs/ai-performance-review.md`](docs/ai-performance-review.md) for measurements,
scope and the proposed worker/cache/native-inference sequence.

**0.4.2 personal DMG (2026-10-10):** the dynamic sidebar is packaged in
[`dist/ShopPhoto-0.4.2-personal.dmg`](dist/ShopPhoto-0.4.2-personal.dmg),
702,515,777 bytes (670.0 MiB), SHA-256
`5b8007232521a31070f7707c5e19108e64e9fd16377af2e79ca8198b0513e0f8`. Universal architectures,
version, signatures, DMG integrity, native codec loading and mounted JPEG/HEIC/AVIF
and JSONL/MCP workflows passed. The installed app and previous DMGs remain
in place. Apple Silicon requires macOS 27.0; Intel retains its standard-codec
macOS 12 baseline. See [`docs/release-0.4.2.md`](docs/release-0.4.2.md).

**Dynamic sidebar implementation (2026-10-10):** Develop now switches between
Presets, AI and Workflow in the existing left rail. Selection persists in
preferences and named workspaces, with backward-compatible defaults. AI tools
reuse the existing workers and undo path; Tools → AI masks
shares its selection controls with the rail. Busy photo actions are disabled,
and hidden preset thumbnails stop rendering. English/Dutch layouts and actual
egui pointer interaction tests passed; full native-codec workspace validation
passed 535 tests, zero failures and three existing explicit ignores. Strict
Clippy, formatter and whitespace checks passed. See
[`docs/phase-dynamic-sidebar.md`](docs/phase-dynamic-sidebar.md).

**0.4.1 personal DMG (2026-10-10):** the latest implementation is packaged as
[`dist/ShopPhoto-0.4.1-personal.dmg`](dist/ShopPhoto-0.4.1-personal.dmg),
703,444,286 bytes (670.9 MiB), SHA-256
`d876158eb4aecdcd0b623ec3a0eaddcb677e978424d028af1af245965a149bf8`. Universal arm64/x86_64, ad-hoc
signatures, DMG integrity, relocated native codecs, mounted JPEG/HEIC/AVIF
rendering and JSONL/MCP workflows passed. Full native-codec workspace checks
passed 532 tests with zero failures and three existing explicit ignores; strict
Clippy and formatting passed. Updated codec libraries require macOS 27.0 in
the Apple Silicon slice; Intel retains its standard-codec/macOS 12 baseline.
The installed app and previous 0.4.0 DMG were left in place. See
[`docs/release-0.4.1.md`](docs/release-0.4.1.md) for package details and limits.

**Lightcraft lessons implementation (2026-10-10):** prioritized latest-open
decode scheduling, foreground memory gating, bounded adjacent RAW preview and
thumbnail caches, shared validated undoable develop/mask commands, JSONL/MCP
automation, optional pinned CLIPSeg text selections, platform GPU defaults and
checked readbacks are integrated locally. New rendering equivalence checks
cover cached/fresh, preview/export, window/full and a CPU tone reference.
See [`docs/phase-lightcraft-lessons.md`](docs/phase-lightcraft-lessons.md) for
usage, scope and validation. Formatter, strict workspace/all-target Clippy
(including native codecs) and all 528 workspace tests pass, with three existing
manual-fixture/benchmark ignores. The optional Python suite passed 13 tests
with four unrelated fixture skips, including real text-selection inference.
Ten dialogs rendered offscreen in English and Dutch; CLI/MCP smoke checks
verified edits, export, undo, preview and source/sidecar preservation. Existing
0.4.0 package artifacts are unchanged.

**Photography implementation checkpoint (2026-09-30):** Lightroom-style editing
and workflow additions are integrated locally; document layers, text/vector
editing and compositing remain excluded. The implementation checklist and
unfinished items are in [`docs/photography-roadmap.md`](docs/photography-roadmap.md),
with checks and exact limitations in [`docs/photography-acceptance.md`](docs/photography-acceptance.md).
This includes catalog/import/offline reliability, versions/stacks/search,
color-managed raster input, compound/semantic/painted masks, retouching and
corrective color/detail, multi-photo merges, output/proof/print/gallery, persistent
delivery queue, workflows/CLI and interface customization.

The final workspace/all-target checkpoint passed 517 tests (zero failures, three
explicit manual-fixture ignores); strict Clippy passed with native codecs,
and the native-codec app binary built. Actual optional AI inference passed all
eleven Python tests and the explicitly invoked Rust worker/mask/sidecar bridge.
Nine dialogs/inspectors rendered through offscreen WGPU in English and Dutch.
Native full-window capture is blocked by compositor occlusion on this host.
Dynamic Dutch templates/tool hints are now connected and checked.
Real pixel-shift/astro/lens/tether capture sets,
Linux/Windows HDR, assistive
technology and physical printer acceptance remain documented. The ad-hoc-signed
0.4.0 universal personal DMG is recorded in `docs/release-0.4.0.md`; it is not a
claim of universal hardware support or a notarized public release. No external repository
state was changed. The final artifact is `dist/ShopPhoto-0.4.0-personal.dmg`
(702,767,288 bytes, 670.2 MiB), SHA-256
`dca82a8efc7484f8dbcba7727589232f6bb5b48e7dc75522942a9d2790704521`.
DMG integrity, mounted signature/content checks, mounted JPEG/HEIC/AVIF GPU
renders passed. Apple Silicon
native codecs require macOS 26+; the Intel standard-codec slice is compiled but
cannot be executed on this host without Rosetta.

**0.2.5 closeout (2026-09-04):** all locally actionable
items in the post-release review are complete. The final workflow pass added
two-up/four-up culling, external-editor TIFF companions, Oklab color-range
masks, an on-canvas curve pipette and opt-in AHD demosaic for 1:1/export.
Workspace test coverage is 324 passing tests; strict Clippy, rustfmt and both
WGSL precision variants pass. AI-02 remains explicitly blocked on
provenance-checked redistribution-safe weights. See [`docs/release-0.2.5.md`](docs/release-0.2.5.md).

**0.2.3 closeout audit (2026-09-03):** repository implementation is complete
for all locally actionable plan items, workspace tests and strict Clippy pass,
real Hasselblad decode/render/export and M5 Pro strict performance pass, and
rawler PRs 837–839 are open upstream. The remaining release gates require
other physical GPUs, matched Phocus exports, human HDR
viewing, signing credentials, or the merge of those upstream PRs. See
[`docs/release-0.2.3.md`](docs/release-0.2.3.md) for the exact matrix.
The local universal ad-hoc-signed `dist/ShopPhoto-0.2.3.dmg` is 53 MB, passes `hdiutil verify`, and has SHA-256
`73404fac84da0d3fa08ca9f6611f1616d02c56c5409b165668995267a0b302e8`.

**0.2.3 improvement pass (2026-09-02):** added backward-compatible contracts
and UI/workflows for highlight recovery controls, local masks/spots, batch
settings sync with undo, persistent history and snapshots, user presets,
masked sharpening, film grain, lens distortion/lateral CA, quarter-turn and
perspective geometry, searchable descriptive metadata, duplicate hashing,
safe moves/card-copy primitives, export borders/watermarks, UI scaling and a
localization seam. Decoder fuzzing now runs weekly and on decoder changes.
The remaining implementation pass completed masks/retouch authoring, wide
highlight propagation, history/virtual copies, preset exchange, geometry,
library ingest/file operations, default JPEG XL, custom ICC proofing, portable
ONNX denoise, Windows scRGB presentation, GPU-loss recovery, visual CI, and
AccessKit/file-backed localization. Hardware, real Phocus fixtures, signing
credentials and upstream PR submission remain external
release gates and are recorded as such in the plan.

**Session 19, viewport tile stability (2026-09-02):** The 1:1 tile is rendered from the previous
frame's view, so a pinch that outran its 256 px apron drew proxy UVs on the
tile texture on the EDR surface (image jumping), popped to the proxy on SDR,
and could stay on the proxy after the gesture ended. The HDR blit now keeps
separate proxy and tile bindings selected per draw (dynamic-offset uniform
slots), the panel overlays the stale tile where it still covers and requests
the follow-up frame, split compare only uses tiles when both describe the same
region, the pan center settles onto the clamped region so edge zooms stop
jumping, and fit→1:1 double-click lands on the clicked detail.

**Session 19, clipped-highlight follow-up:** partially clipped highlights
were being blended toward equal *camera-space* components before unequal WB
gains, turning the transition into a pink rim. `wb_color.wgsl` now reconstructs
the clipped camera channels as before, then neutralizes unrecoverable chroma
in linear Rec.2020 after WB and the camera matrix. Two-or-more-channel clipping
becomes exactly achromatic while retaining its reconstructed intensity and HDR
headroom; a single clipped channel keeps the softer local-ratio fade. The GPU
regression reproduced the old pixel as `[173, 155, 164]` and now verifies
neutral output through SDR preview, EDR preview, the 1:1 EDR tile, SDR export,
and HDR PQ export. The full workspace suite (265 tests) and strict Clippy pass.
Rebuilt universal artifact:
`dist/ShopPhoto-0.2.2.dmg`, 519,233,028 bytes, SHA-256
`88513ef9193c7000249788937151faeed700ed1ada6995f90c80ed37e174a9fd`.

**Session 19, second pass (follow-up plan):** The fringe picker now reports
image-normalized coordinates on the 1:1 tile path (the sampler's contract).
`SP_SCREENSHOT` works on the float EDR surface by reading the renderer's
scRGB display texture (`read_scrgb_display_srgb8`; tile when zoomed).
That capture exposed a real defect: the vendored decoder had no X2D II
definition and treated the full 11904×8842 frame as image, so viewer,
export and denoise tiling carried the masked black border (124/92/116/0
px measured on three files). `x2d_ii_100c.toml` now defines the active
area as 11656×8742 at (128, 96), Phocus's framing; the X2D 100C entry is
untouched because its border is unmeasured. With real image at the frame
edge, the Malvar demosaic and raw NR shaders' clamped taps painted a
two-pixel magenta line; taps now mirror without repeating the edge so the
Bayer phase survives. Plan and measurements:
https://claude.ai/code/artifact/c461d9d0-5674-4c50-85ca-ca45062a3856
Rebuilt artifact with all of the above: `dist/ShopPhoto-0.2.1.dmg`,
519,880,198 bytes, SHA-256
`2339671ab6b1fdd858d89703160252acb8c3b216c03423f53d9b133bf259818d`.

**Prior update:** 2026-09-01 (session 18: eclipse-scale bloom and XCD lens
identity). The fixed 5 px defringe neighborhood is now a two-scale system:
conservative narrow CA plus an explicit wide/clipped-highlight mode reaching
256 full-resolution pixels. Purple and green have independent editable hue
centers/ranges and real on-image pickers; Auto evaluates five proxy radii and
returns per-color full-resolution width evidence. Repair remains highlight-,
edge-, hue-, and saturation-outlier gated, transfers neutral chromaticity, and
adds perceived-luminance compensation for broad halos. A 22 px synthetic
eclipse fixture verifies material cleanup, brightness safety, and automatic
wide-mode selection. The proxy stage now actually executes after post-tone
color, full-resolution tile radii no longer shrink by tile width, and the tile
apron covers the larger of local-detail and fringe needs.

The lens UI now preloads 22 XCD identity names, including historical maker-note
variants such as `XCD 45 II`, while keeping identity separate from correction
profiles. The installed Phocus bundle's 59 GMap files were audited but remain
opaque private binary payloads; ShopPhoto does not copy them or mislabel an
identity grid as applied calibration. New documentation is in
`docs/research-2026-08-31/lens-defringe.html` and
`lens-phocus-profiles.html`. Release validation passes with 250 workspace
tests, strict Clippy, locked all-target check, warning-denied rustdoc, a live
51 MP Hasselblad UI run, universal arm64/x86_64 build, deep signature check,
and verified DMG. Artifact: `dist/ShopPhoto-0.2.0.dmg`, 47,541,350 bytes,
SHA-256 `b4307562058c1071446010f82343d78bf1307141bdebde04e7451cba439ef072`.

**Prior update:** session 17: reliable automatic/manual narrow lens defringe.
ShopPhoto added an independently written post-color GPU stage for purple and
green longitudinal chromatic aberration with conservative automatic analysis.

**Earlier update:** session 16: imaging and Studio Console release pass. A
sustained four-hour research/implementation loop used OpenCode
reviews from GLM 5.3 Flash (math/pipeline), Muse Spark 1.2 (three UI
directions), and Kimi K3 (Phocus/Hasselblad and clean-room boundary), followed
by local implementation and repeat audits.

- HDR now protects reference white and uses one smooth, monotonic near-white
  `tanh` shoulder in CPU, SDR, PQ, and EDR paths. Luminance-derived RGB scaling
  preserves hue, keeps useful highlight separation, and avoids a hard
  peak-white plateau. The native `--hdr-check` passes on this XDR machine with
  an `Rgba16Float` surface, extended-linear-sRGB layer, and ×16 potential EDR
  headroom.
- Full Auto Balance now owns a fresh true-luminance histogram, measures robust
  p0.5/p10/p50/p90/p99.5 statistics, keeps scene-key intent, and caps exposure
  at ±1.75 EV. A bounded mid-shadow lift helps genuinely backlit frames. Its
  private neutral analysis cannot flash on screen or be consumed by a dirty
  viewport render, and readback failures retry before reporting visibly.
- All 11 built-in presets were retuned into restrained, end-use-ready recipes.
  Applying one preserves exposure and image-specific state unless the recipe
  explicitly owns an endpoint. Six owned Hasselblad 3FR/FFF files (51 and
  105 MP, including backlight and saturated-color cases) were rendered and
  visually compared across every preset.
- The UI follows Muse's selected “Studio Console” direction: clearer hierarchy,
  a primary Auto action, selected/modified preset state, quieter labels, a View
  menu, proportional HDR-headroom meter, stronger empty state, and verified
  normal-text contrast tokens.
- `Reference Match` accepts the user's own neutral TIFFs for colour calibration.
- The self-contained HTML notebook under `docs/research-2026-08-31/` groups HDR,
  Auto, presets/grading, Phocus/Hasselblad, Reference Match, UI, and
  implementation knowledge. It includes real-file comparisons and an
  interactive shoulder chart; every page was checked at 390 px and 1440 px
  with no overflow, hidden cells, missing images, or duplicate IDs.
- Release validation: all 245 workspace tests, strict Clippy, and a locked
  all-target workspace check pass on the corrected Rust 1.92 MSRV; strict
  GPU performance budgets, rustdoc with warnings denied, six-file decode/GPU
  acceptance, arm64 and x86_64 release compilation, plist/script linting, and
  real 2× Bayer export. The universal DMG stages the app, Applications shortcut,
  research notebook, and a one-click notebook opener; the local build is ad-hoc
  signed because no Developer ID/notary credentials were supplied.
- Final artifact: `dist/ShopPhoto-0.2.0.dmg`, 48,527,383 bytes, SHA-256
  `579559fc1eac43ef22d31f2363b78d7f43ec0cfa1ae536ae666a3ff316ca098f`.
  `hdiutil verify` passed; a read-only mount audit confirmed both architecture
  slices, the deep code signature, the Applications symlink, all 24 notebook
  files/internal links, and the relative one-click notebook opener. The bundle
  is intentionally not notarized because this machine has no Developer ID or
  notary profile.

**Earlier update:** 2026-07-26 (session 15: the open-improvements pass — the
three BUILD.md §10 Apple Silicon items that were specified but never built
(mapped staging ring, SHADER_F16 with a validated f32 fallback, workgroup
shared-memory tiling in NR), the pre-demosaic NR shader Phase 4 deferred,
criterion benches plus a two-tier perf-budget test, XMP ratings interop,
Ultra HDR JPEG export with gain maps, a README and dual license, the
vendored rawler changes exported as an upstreamable patch series, and a
cargo-fuzz run that found a real OOM on truncated RAWs — now guarded by a
structural pre-flight check. See `docs/platform-apple-silicon.md` and
`docs/interop-xmp-ultrahdr.md`. Session 14: preset/color/export expansion —
11 one-click develop presets now fill the Develop rail and apply as explicit
undo steps while preserving image-specific WB/profile/NR/crop/output state.
The color pass adds three-way Oklab grading plus an 8-band monochrome mixer,
with GPU shader and pixel-level golden coverage. Export adds five built-in
recipes, short-edge and percentage sizing, JPEG 4:4:4 control, and
low/standard/high final-size output sharpening with 8/16-bit and SDR/PQ
support. Version advanced to 0.2.0. Session 13: tone-upgrade plan A–G implemented —
midtone Brightness and histogram Auto, parametric plus RGB curves, culled
Oklab local contrast, post-crop vignette, HDR scene white, clipping/histogram
interactions, persisted proof spaces with GPU/export parity, and five built-in
looks. Schema remains v1 with old-curve migration. Added CPU/GPU parity and
golden coverage; real-file local-contrast proxy cost measured 9 ms (X2D II)
and 8 ms (X1D II). The planned quarter-res Gaussian was replaced by a bounded
multi-radius kernel; macOS DisplayNative continues to use the correctly tagged
scRGB layer rather than retagging the shared egui layer per proof selection.
Session 12: `--hdr-check` diagnostic added —
prints surface format / colorspace / panel capability / verdict and exits.
The user's "still SDR" report was a STALE dist bundle + release binary from
before the EDR work; both rebuilt (universal, needs `nasm` via brew for the
x86_64 AVIF assembly) and now verify PASS with ×16 potential headroom on
the MacBook Pro 14 XDR panel. Badge/verdict use POTENTIAL headroom
(`maximumPotentialExtendedDynamicRangeColorComponentValue`) — the CURRENT
value sits at 1.0 until EDR pixels actually present. Session 11: EDR colorspace shim — the layer
colorspace was nil so macOS clamped at 1.0; now set to extended linear
sRGB via macos_edr.rs, live panel-headroom badge, and the float viewport
path runs for SDR edits too = 16-bit-precision viewing) · **Phases 0–9 complete AND
macOS EDR (HDR) display is now live** — vendored egui-wgpu float-surface
patch + scRGB blit callback (see docs/phase-6.md for the full design and
deviations). Viewport gestures: pinch / ⌘·Ctrl+scroll zoom anchored at the
cursor; plain scroll pans when zoomed. External acceptance remaining: real
Phocus TIFF calibration input, by-eye EDR confirmation on an XDR panel,
cross-platform hardware runs, signing credentials.**

---

## Environment facts (this machine)

- macOS (Apple Silicon, aarch64). Rust installed via **Homebrew rustup**:
  toolchain proxies live at `/opt/homebrew/opt/rustup/bin` — **not** on the
  default PATH in non-interactive shells. Every build command needs:
  `export PATH="/opt/homebrew/opt/rustup/bin:$PATH"`.
- Toolchain: stable 1.97.0 (aarch64-apple-darwin). `just` is NOT installed;
  use plain cargo commands or `brew install just`.
- Repo is a git repository (branch `main`), committed locally through
  Phase 2 (no remote yet). Commit at the end of each session.

## Verify-everything command (run before and after your changes)

```sh
export PATH="/opt/homebrew/opt/rustup/bin:$PATH"
cargo fmt --all --check \
  && cargo clippy --all-targets --workspace -- -D warnings \
  && cargo test --workspace \
  && cargo run -p sp-app     # window must open, themed, three-pane
```

Additional checks worth running before recording timings or cutting a build:

```sh
SP_PERF_STRICT=1 cargo test -p sp-pipeline --test perf_budget -- --nocapture
cargo bench -p sp-pipeline
SP_TEST_RAWS=~/Documents/shopphoto-test-raws cargo test --workspace -- --nocapture
cargo +nightly fuzz run raw_decode -- -max_total_time=3600 -rss_limit_mb=4096
```

The fuzz corpus is seeded from the real RAW headers
(`fuzz/corpus/raw_decode/`, gitignored); seeding it matters — a run from
random bytes never reaches the decoder's TIFF layer, which is where every
finding so far has been.

Current state: the session-18 release checks above pass (250 workspace tests;
the app opens, decodes and renders real 51 MP and 105 MP Hasselblad fixtures).

---

## Phase 0 — DONE (this session)

What exists and can be relied on:

### Workspace (`Cargo.toml`, 7 crates under `crates/`)
Dependency rule enforced by manifest layout:
`sp-app → {sp-pipeline, sp-catalog, sp-export} → {sp-gpu, sp-raw} → sp-core`.
Workspace-level dep versions; `eframe 0.35` / `egui 0.35` pinned in `sp-app`
(wgpu backend only — glow is disabled via `--no-default-features`).

> ⚠ **egui 0.35 API note (differs from older docs/models' training):**
> `eframe::App` has `fn ui(&mut self, ui: &mut egui::Ui, …)` (NOT
> `update(ctx, …)`) plus optional `fn logic(&mut self, ctx, …)` for input;
> panels are `egui::Panel::top/bottom/left/right(id).exact_size(px).show(ui, …)`
> (NOT `TopBottomPanel`/`SidePanel` on ctx), `CentralPanel` still exists and
> must come last; `ctx.all_styles_mut(...)` replaces `ctx.style_mut(...)`;
> `ctx.egui_wants_keyboard_input()` replaces `wants_keyboard_input()`.

### `sp-core` — REAL, tested, the contract everything builds on
- `error.rs`: `SpError` (thiserror) — the workspace error type.
- `color.rs`: Rec.2020/XYZ/709 matrices with round-trip + D65 white-point
  tests, sRGB OETF/EOTF with reference-value tests, `Mat3` helpers.
- `raw.rs`: `RawImage` (CFA u16 + black/white levels + WB + cam→XYZ matrix +
  active area + orientation), `CfaPattern` with `color_at` tests, `ImageMeta`.
- `edit.rs`: **complete `EditState`** for ALL phases (wb, tone, curve, HSL,
  sharpen, `NrParams`, `ResolutionParams` incl. Super2x2/3x3/4x4, `HdrParams`,
  geometry, `ProfileChoice`), `EDIT_SCHEMA_VERSION = 1`, `Default` = neutral
  (tested), `clamped()` range enforcement (tested), serde round-trip (tested).
- `profile.rs`: `CameraProfile` with illuminant-matrix set + `resolve_matrix`
  (standard dual-illuminant interpolation in reciprocal CCT, BUILD.md §3.4),
  tested including clamping at endpoints.

### `sp-catalog` — sidecar IO REAL and tested (DB part is Phase 7)
`Sidecar { version, edit, annotations }` ↔ `<original>.spedit` JSON;
atomic save (tmp+rename); corrupt file → `SpError::CorruptData` (caller
quarantines, never deletes); missing → `Ok(None)`. All tested.

### `sp-gpu` — vocabulary only (device init is Phase 2)
`PassId` enum for every pipeline pass with `shader_file()` mapping into
`shaders/`, `WORKGROUP_DIM = 16`, `dispatch_size` (tested incl. X2D dims).

### `sp-pipeline` — pass planning REAL and tested (execution is Phase 2)
`plan_passes(edit, target)`: pure pass-selection — neutral-param culling,
Super2x2 ⇒ superpixel demosaic replaces Malvar (no extra pass), 3×3/4×4 ⇒
Malvar + box `Supersample` pass, export target has no UI composite. Tested.
This function is the single source of truth for pass selection.

### `sp-export` — settings vocabulary REAL and tested (encoders are Phase 5)
`ExportFormat/ExportColorSpace/SizeMode/ExportSettings`; `SizeMode::output_dims`
(never upscales, megapixel targeting) tested.

### `sp-raw` — API shape only
`decode(path) -> DecodeOutput{raw, meta, preview}` returns a clean
"not implemented until Phase 1" `SpError::Decode`; `is_supported` extension
gate (3fr/fff/dng) tested. **Signature is final** — Phase 1 fills the body
with rawler.

### `sp-app` — full GUI shell, themed, running
- `theme.rs`: ALL design tokens from BUILD.md §5.1 (colors, radii, 8px grid,
  panel dimensions). Only place colors may be defined.
- `app.rs`: layout composition + shortcuts (Tab rails, Z zoom, D/G mode).
- `panels.rs`: top bar (Library/Develop toggle, Export btn, zoom), left rail,
  right develop panel with ALL tool groups (Basic/Curve/Color/Detail/
  Resolution/Geometry/HDR) — sliders genuinely bound to `EditState` with
  double-click-reset via `param_slider` helper; Resolution group shows
  computed output sizes (X2D placeholder dims until Phase 1); filmstrip and
  viewport are labeled placeholders.
- `state.rs`: `AppState`/`Mode`/`PanelOpen` — no egui types, unit-testable.
- CI (`.github/workflows/ci.yml`): fmt + clippy -D warnings + tests on
  macOS/Windows/Ubuntu. `CONTRIBUTING.md`: binding code standards.

---

## Phase 1 — RAW Decoding Core: CODE COMPLETE (this session)

Implemented:
- `sp-raw` now wraps **rawler 0.7.2** (nice discovery: it bundles a JPEG XL
  decoder — newer X2D firmware compresses raws with JXL, so that path is
  covered). `decode()` is real: `RawSource → get_decoder → raw_image +
  raw_metadata + preview_image/thumbnail_image`, all mapped into `sp-core`
  types in `crates/sp-raw/src/convert.rs` (rawler types never escape).
  - CFA name → `CfaPattern` (2×2 Bayer only; X-Trans etc. → clean error).
  - Levels: rawler's flexible-length RGBE vectors normalized via `spread4`.
  - WB: RGBE with NaN/zero 4th slot mirrored from G; all-zero WB → neutral.
  - Color: rawler gives XYZ→cam; inverted with new `sp_core::color::
    mat3_inverse` (tested) into our `cam_to_xyz`.
  - Float DNGs normalized to u16 via rawler's `force_integer`.
  - Preview decoded to RGBA8 (`Preview{width,height,rgba8}`) so downstream
    needs no image crates. Orientation mapped (flips → nearest rotation,
    documented TODO for Phase 3).
- Verification harness `crates/sp-raw/tests/hasselblad.rs`: reads
  `$SP_TEST_RAWS` folder; asserts MP plausibility, level ordering, WB
  positivity, non-degenerate matrix, metadata presence; prints a per-file
  summary. Skips cleanly (with note) when the env var is unset.
- `sp-app`: `jobs.rs` decode worker thread (crossbeam channels, results
  drained in `App::logic`); ⌘O + "Open…" button (rfd dialog on its own
  thread) + drag-and-drop; viewport shows spinner while decoding, then
  aspect-fit embedded preview (`egui::TextureHandle`); Library right panel
  shows real metadata rows (camera/lens/ISO/shutter/aperture/focal/date);
  error toasts bottom-right (TTL 6 s), stale-result guard for superseded
  opens.

### VERIFIED on real files (session 2)

User's sample files live in `~/Documents/shopphoto-test-raws/`
(run: `SP_TEST_RAWS=~/Documents/shopphoto-test-raws cargo test -p sp-raw --test hasselblad -- --nocapture`):

- `8578321983.3fr` — **Hasselblad X2D II 100C** (the user's camera is the
  Mark II!): 11904×8842 (105.3 MP), RGGB, ISO 200, 3888×2918 preview. ✔
- `x1d-ii-xcd45p-01.fff` — X1D II 50C via Phocus: 8280×6208 (51.4 MP),
  RGGB, ISO 400, 1181×885 preview. ✔

Getting there required **vendoring rawler 0.7.2** into `vendor/rawler`
(wired via `[patch.crates-io]` in the workspace Cargo.toml) with three
minimal patches — KEEP THESE if the vendored copy is ever refreshed, and
drop the vendor dir entirely once upstream dnglab has native X2D II support:

1. `data/cameras/hasselblad/x2d_100c.toml`: model aliases for
   `X2D II 100C` (upstream has no definition for the 2025 body; the X2D
   100C sensor definition + color matrix is a close stand-in — a dedicated
   matrix would come from an Adobe DCP or user-created Reference Match profile in Phase 3B).
2. `data/cameras/hasselblad/x1d2.toml`: alias `"Hasselblad X1D II 50C"` —
   Phocus-written .fff files carry the make-prefixed model string.
3. `src/decoders/tfr.rs`: decide compressed-vs-uncompressed from the raw
   IFD's TIFF Compression tag instead of the per-camera "uncompressed" hint
   (tag absent → old hint behavior). Phocus .fff files from these cameras
   are LJPEG-compressed while their 3FRs are not; upstream's hint-only logic
   panic-slices on such files. Also: `sp-raw/src/convert.rs` now takes the
   camera matrix from rawler's per-illuminant `color_matrix` map (D65
   preferred) because the legacy `xyz_to_cam` field is zeroed on some paths,
   and falls back to `full_image()` for the embedded preview (the 3FR/fff
   decoder only exposes its JPEG there).

Worth doing eventually: upstream patches 1–3 to dnglab as a PR.

Still open from Phase 1 spec: filmstrip should list opened files (cosmetic,
folds into Phase 2).

## Phase 2 — GPU Pipeline Foundation: CORE COMPLETE (session 3)

The app now renders real RAW sensor data on the GPU. Measured on the user's
files (dev build, Apple Silicon, headless harness
`SP_TEST_RAWS=~/Documents/shopphoto-test-raws cargo test -p sp-pipeline --test real_files -- --nocapture`):

- X2D II 100C (105.3 MP): decode 78 ms + GPU open+render 83 ms → proxy 2048×1521
- X1D II .fff (51.4 MP): decode 331 ms + GPU 24 ms
  (both far inside the < 500 ms first-frame budget)

What exists:
- `sp-gpu::context::GpuContext` — device/queue handles; `headless()` for
  tests (pollster). **Both device paths request
  `max_texture_dimension_2d = adapter limit`** — eframe's default 8192 is
  too small for 100 MP sensors (11904 px); the app overrides
  `NativeOptions.wgpu_options.wgpu_setup` (see `sp-app/src/main.rs`).
- `sp-gpu::pipelines` — compiled compute passes (superpixel, wb_color,
  downsample, output) + shared bind-group shape + uniform structs
  (bytemuck, layouts match WGSL exactly) + `make_texture`/`bind`/`run`
  helpers. wgpu 29 API notes: `PipelineLayoutDescriptor.bind_group_layouts:
  &[Some(&layout)]`, `immediate_size: 0` (no push_constant_ranges),
  `push_error_scope` returns a guard with `.pop()`, `device.poll(PollType)`.
- Shaders in `shaders/` (all validate on Metal; CI test
  `sp-gpu/tests/shaders_compile.rs`): `demosaic_superpixel.wgsl` (FUSED
  raw-prepare + 2×2 quad→pixel; also the Super2x2 supersample path),
  `wb_color.wgsl` (WB gains → cam→Rec2020 matrix → exposure; scene-linear,
  no highlight clamp), `downsample.wgsl` (integer box, linear), 
  `output_transform.wgsl` (Rec2020→709 + sRGB OETF into **Rgba8Unorm** —
  NOT -Srgb, because egui's `register_native_texture` requires Rgba8Unorm;
  encoding happens in-shader), plus `raw_prepare.wgsl` +
  `demosaic_malvar.wgsl` (canonical Malvar 5×5) written & validated but not
  yet executed (full-res path).
- `sp-pipeline::GpuRenderer` — `open_image` (CFA upload → superpixel →
  box-downsample to ≤2048-long-edge proxy of linear camera RGB) and
  `render(edit)` (wb_color + output at proxy res; re-runs per edit change).
  Exposure slider is live end-to-end. Golden tests
  (`sp-pipeline/tests/golden.rs`): synthetic CFA through the REAL GPU vs
  CPU reference, ±2/255; exposure +1 EV verified.
- `sp-app`: `GpuState` in app.rs — creates `GpuRenderer` on eframe's shared
  device, registers the output texture with egui
  (`renderer.write().register_native_texture`), re-renders when `EditState`
  changes (compare in `logic`), frees/re-registers per image. Viewport
  prefers the GPU render over the embedded preview.

### Phase-2 remainder (fold into next sessions)
1. Wire WB temp/tint (needs CCT→gains model; Phase 3 alongside tone LUT).
2. Full-res Malvar path + 1:1 zoom tiling (BUILD.md §3.5) + pan/zoom input.
3. Live histogram (GPU reduction) — right panel placeholder waiting.
4. Blue-noise dither in output transform.
5. Filmstrip listing of opened files.
6. Non-exposure sliders currently do nothing (tone pipeline is Phase 3) —
   expected, do not "fix" by faking on CPU.

## Phase 3 — Develop Controls & Non-Destructive Editing: CORE COMPLETE (session 4)

Everything below is implemented, tested (50 tests total), clippy-clean, and
verified against the real sample files (105 MP: decode 106 ms + GPU 59 ms
with the full tone chain).

- **WB temp/tint are real**: `sp-core/src/wb.rs` — Kim et al. Planckian-locus
  CCT→xy, tint = y-offset (Lightroom convention: +tint assumes greener
  illuminant ⇒ renders magenta; sign is TESTED, don't flip it), gains =
  neutralize illuminant white in camera space via inverse forward matrix.
  As-shot remains the default (`temp_kelvin: None`).
- **Tone LUT**: `sp-pipeline/src/tone.rs` — exposure/contrast/highlights/
  shadows/whites/blacks + user Catmull-Rom curve compiled into ONE 1024-entry
  LUT (shaper domain x/(x+1), cap 64.0 — must match `tone_color.wgsl`
  SHAPER_MAX_T). Neutral = identity (tested); monotonicity enforced.
  7 unit tests incl. region isolation (shadows don't move highlights etc).
- **tone_color.wgsl**: LUT per channel + 8-band HSL in Oklab LCh (raised-
  cosine bands, centers 25/55/100/145/200/260/300/345°), vibrance (muted-
  weighted, orange-band skin protection), saturation. `color_neutral`
  uniform skips the Oklab detour. Extra binding(3) = LUT storage buffer
  (`make_lut_pass` in sp-gpu).
- **Histogram**: `histogram.wgsl` (atomic 3×256 bins over the output tex) →
  3 KB blocking readback per render → filled RGB curves in the panel
  (99.5-percentile normalization).
- **Curve editor UI**: interactive Catmull-Rom canvas (drag/add/double-click-
  remove, endpoint x pinned, neighbor clamping) in panels.rs; sampling via
  `sp_pipeline::tone::sample_curve`.
- **HSL UI**: 8 color chips + per-band H/S/L sliders (`state.hsl_selected`),
  edited-band dot indicators.
- **Sidecars live**: loaded on open (corrupt → quarantined `.spedit.corrupt`
  + toast, never deleted), saved debounced 500 ms after last change and
  flushed before opening the next image. `.spedit` is gitignored.
- **Undo/redo**: gesture-coalesced snapshots (0.5 s gap rule, cap 200) in
  `AppState::record_edit_gesture` (unit-tested), ⌘Z / ⇧⌘Z. Plain Z (zoom)
  now requires no ⌘ modifier.
- **Before/after**: `\` toggles `show_original` (renders EditState::default,
  "BEFORE" badge in top bar). Render dirty-key is (edit, show_original).
- **Reset all edits** button at the develop panel bottom.
- MSRV was bumped to 1.85 (`Option::is_none_or`) in that earlier pass; it is
  now 1.92 because the current egui/eframe 0.35 dependency requires it.

### Phase 3 remainder — DONE (session 5)
1. **1:1 Malvar tiles + pan/zoom**: `GpuRenderer::render_tile(edit, x,y,w,h)`
   runs raw_prepare → Malvar 5×5 → wb → tone → output over the visible
   region + 4 px margins (UV-cropped at display). CFA parity handled via new
   `CfaPattern::shifted` (tested); tile correctness golden-tested at odd
   origins (flat-color invariance). Viewport: scroll = zoom at cursor
   (clamped fit·0.5‥6×), drag = pan, Z / double-click = fit↔100%, zoom badge
   bottom-right. `AppState::visible_region/effective_zoom` do the math;
   tile renders are keyed on (region, edit, show_original) and reuse
   textures until the viewport size changes. Proxy shows meanwhile.
2. **Dither**: interleaved-gradient-noise ±0.5 LSB in output_transform.
3. **Filmstrip**: session list with embedded-preview thumbnails (nearest-
   downscaled ~192 px), click to switch (sidecar flushed first), current
   file outlined. Folder browsing remains Phase 7.
4. **Crop/rotate wired**: "Crop" toggle in Geometry → overlay with corner/
   move drags on the fit view (min 5% size); fit view displays the cropped
   region re-fitted; flip = UV mirror; straighten rotates the fit display
   (small-angle preview via egui Image::rotate). NOTE: geometry is display-
   only until Phase 5's export resample; 1:1 view inspects the full
   uncropped frame by design.
Remaining editing gaps are OWNED BY LATER PHASES: sharpen/NR sliders
(Phase 4), ratings/flags UI (Phase 7), export-side geometry (Phase 5).

## Phase 3B — Camera Color Profiles & Reference Match: CODE COMPLETE (session 6)

- Complete `CameraProfile` resolution including preferred DNG
  ForwardMatrix1/2, inverse-CCT interpolation, dual HueSat maps, DNG look
  table, base curve, and fitted 33³ scene-linear RGB LUT.
- Standalone bounded DCP TIFF/EP parser in `sp-raw`; imported profiles are
  copied to the app profile store and filtered by normalized camera model.
- Dedicated `huesat_lut.wgsl` pass uses `rgba16float` 3-D textures and
  explicit trilinear sampling before and after tone. The same chain is live
  in proxy and Malvar tile renders.
- Full non-blocking Reference Match wizard: export checklist, RAW multi-picker,
  TIFF-folder stem pairing, ICC-aware 16-bit decode, low-gradient sampling,
  3×3 + curve + smooth 33³ fit, true mean/p95 ΔE00 holdout report, and
  `.spprofile` JSON + `.splut` binary persistence.
- Synthetic fitter acceptance is < 0.5 mean holdout ΔE00; CIEDE2000 checked
  against the published Sharma reference pair. GPU golden test proves the
  DCP map changes output. Real 105 MP open+render remains 49 ms on this
  machine with the extra profile pass scaffold.
- **External-data validation still needed:** the fixture folder has RAWs but
  no neutral Phocus TIFF exports. Run the wizard on 3–10 user exports and
  confirm its reported mean holdout ΔE00 is < 2.0. The implementation is
  complete; this is the real calibration acceptance run.

See `docs/phase-3b.md` for the maintained Phase 3B inventory and deviation.

## Session 7 — render correctness + GUI modernization

Visual QA against the camera's own embedded JPEG exposed three REAL
pipeline bugs the synthetic golden tests could not see (they never clip,
never rotate, and use identity camera matrices):

1. **Magenta cast / wrong colors (critical)**: we applied WB gains AND the
   raw inverse ColorMatrix — double illuminant correction. Fixed with the
   standard dcraw/DNG white-preserving matrix:
   `sp_core::profile::display_cam_to_xyz` scales the inverse-ColorMatrix
   columns by the camera-space D65 white so the balanced neutral axis maps
   exactly to D65; ForwardMatrix-based profiles instead get Bradford
   D50→D65 (`matrix_is_forward` on `ResolvedColorParams`). Unit-tested with
   the real X2D matrix; verified visually against the embedded JPEG.
2. **Orientation ignored**: Hasselblad decoders report Normal; truth is in
   EXIF tag 274. `sp-raw` now reads it and PHYSICALLY rotates the CFA at
   decode (`apply_orientation`, tested for 90/180/270 + pattern/active-area
   transforms) — downstream (proxy/tiles/crop/export) always sees an
   upright image. Embedded previews rotate too (`rotate_preview`).
3. **Blown highlights rendered magenta**: post-WB channels now clamp at the
   normalized saturation point in `wb_color.wgsl` (dcraw-style neutral
   clip). Proper highlight reconstruction remains a future phase.
4. **Default render looked flat/dark**: `tone::standard_base_curve()` — a
   gentle filmic lift applied as the Standard profile's base curve
   (profiles with their own curve replace it). Golden tests mirror it via
   the same LUT builder.

GUI modernization (the app previously rendered in LIGHT theme on light-mode
Macs — egui 0.35 follows the system unless `ctx.set_theme(Dark)` is called;
that one line was the "stale look" root cause):
- theme.rs rebuilt: deeper cool-tinted palette, card/hover/faint tokens,
  accent `#5394FF`, slider trailing fill, popup shadows, thin scrollbars,
  `card_frame()` + `section_text()` helpers. Dark forced.
- panels.rs: segmented Library/Develop control, pill buttons (accent-filled
  Export), two-tone logo, card-based tool groups, adaptive slider rows
  (label 76pt + flexing slider + value chip — the old fixed widths
  OVERFLOWED the panel and clipped labels), filled RGB histogram, padded
  left rail, right-panel width guard.
- Dev QA loop: `SP_SCREENSHOT=/path.png shopphoto <raw>` captures the real
  window ~4.5 s after launch via egui's viewport Screenshot command and
  exits — used headlessly by agents/CI since macOS screencapture needs
  permissions. CLI arg = open file at startup.
- Workspace dev profile: sp-core/sp-raw/sp-pipeline pinned to opt-level 2
  (the `"*"` override covers dependencies only — the 105 MP rotation loop
  at opt 0 took >10 s).
- render_dump test (`SP_TEST_RAWS` + `SP_DUMP_DIR`) writes proxy + embedded
  preview PNGs for visual comparison.

## Phase 4 — Noise Control Suite: COMPLETE (session 8)

- **`nr_main.wgsl`** — one pass, scene-linear Rec.2020, inserted between the
  profile pre-pass and tone (extra `work_nr` texture in proxy AND tile
  paths; pass culled when both strengths are 0 — determinism covered by
  `tests/nr.rs`). Luminance: NLM-lite (3×3 patches over a 5×5 search on
  cube-root luma; `luminance_detail` blends residual back). Chroma:
  edge-aware bilateral on Oklab a/b over 7×7 (luma-gated; `chroma_detail`
  tightens the edge sigma). Applied as a luma-ratio × chroma-rebuild so the
  two axes stay independent.
- **Hot pixels**: `raw_prepare.wgsl` clamps photosites exceeding
  1.5× their same-color axis neighbors (+0.02) — toggled by
  `edit.nr.hot_pixels` (tile/1:1/loupe path). The superpixel/proxy path
  applies a fixed mild 2.0× clamp ALWAYS (it renders once at open, before
  any edit exists; a hot site would survive the 2×2 average as a colored
  dot). RawParams grew `hot_enable`/`hot_thresh` (both WGSL structs + Rust
  kept in sync).
- **ISO seeding**: `NrParams::seeded_for_iso` (tested table, chroma
  10→50 / luminance 0→30 across ISO bands) applied ONLY when an image has
  no sidecar; struct `Default` stays neutral for undo/reset.
- **Noise loupe**: Detail panel shows a true 1:1 crop at the view center
  while the viewport is in fit view. It reuses the tile machinery —
  IMPORTANT: tile and loupe share one texture, so each render invalidates
  the other's cache key (`tile_key`/`loupe_key` cross-clear in app.rs).
  When zoomed, the loupe hides ("viewing 1:1 already").
- **Tests** (`sp-pipeline/tests/nr.rs`, GPU, skip-without-adapter):
  MSE vs clean reference drops >45% on the seeded-noise gradient
  (measured 65%); NR-off renders bit-identical/deterministic; hot-pixel
  suppressed under default edit AND provably leaks with the toggle off.
- 75 tests total; X2D II real file: 51 ms GPU open+render with the chain.
- Deviations: NR runs post-demosaic only (no pre-demosaic `nr_raw.wgsl`
  yet — the fused superpixel path benefits little; revisit with Phase 5
  full-res exports if needed). No shared-memory tiling in the NLM loop
  (Phase 8 perf work if profiling demands).

## Superseded: original Phase 2 attack plan (kept for context)

Order of attack:
1. `sp-gpu`: device/queue init (prefer high-performance adapter), texture
   pool, uniform helper, compute-pass runner around the existing `PassId`
   vocabulary. eframe exposes the shared wgpu device via
   `eframe::CreationContext::wgpu_render_state` (egui 0.35 / eframe wgpu
   backend) — use that device rather than creating a second one.
2. Shaders per BUILD.md Phase 2 step 2: `raw_prepare.wgsl`,
   `demosaic_superpixel.wgsl` (first frame + Super2x2), `demosaic_malvar.wgsl`,
   `wb_color.wgsl` (via `CameraProfile::standard` Tier 1), `output_transform.wgsl`
   (sRGB + dither). Shader files go in `shaders/`; names already bound in
   `sp_gpu::PassId::shader_file`. 16×16 workgroups (`WORKGROUP_DIM`).
3. `sp-pipeline` executor consuming `plan_passes` output; proxy strategy per
   BUILD.md §3.5 (superpixel instant frame → refined proxy; 1:1 tile render).
4. Viewport: replace `egui::Image` preview path with a custom paint callback
   (egui_wgpu `Callback`) sampling the pipeline output texture; keep the
   preview as the pre-render placeholder. Pan/zoom input.
5. Golden-image tests with synthetic CFA fixtures (no camera files in repo).
6. CPU upload: use a mapped staging ring per BUILD.md §10.1 (Apple Silicon).

Acceptance (BUILD.md): color-correct image < 500 ms (superpixel) and < 2 s
full quality for 100 MP; 60 fps pan/zoom; golden tests ΔE < 1.0.

## Notes / decisions made that aren't in BUILD.md

- Slider widths: `style.spacing.slider_width = RIGHT_PANEL_W - 140` keeps
  label+slider+value on one line in the 320px panel.
- `PanelOpen` is cloned in `right_panel` and written back at the end (borrow
  split); fine at this scale.
- `profile.rs::resolve_matrix` uses `unreachable!` on empty matrix list —
  documented invariant of `CameraProfile` construction; keep constructors the
  only way to build one.
- Placeholder X2D dims used in Resolution group: 11656×8742 (matches
  `dispatch_size` test).

## Standing reminders for future sessions

- Keep BUILD.md as the spec; if implementation must deviate, record it here
  and in `docs/phase-N.md`.
- Never render egui to an HDR/float surface later — composite architecture is
  mandatory (BUILD.md Phase 6 step 1 / risk #3).
- The user's cameras: Hasselblad, 50 & 100 MP (supersampling 100→25, 50→12.5
  is a headline feature). Primary OS: macOS (Apple Silicon).
- Commit series pending user approval; suggest committing Phase 0 as
  "Phase 0: workspace scaffolding, sp-core types, GUI shell".

## Session 15 — the open-improvements pass (2026-07-26)

Everything here started from "what is still open that makes sense", not
from a phase. Full detail in `docs/platform-apple-silicon.md` and
`docs/interop-xmp-ultrahdr.md`; deviations are recorded there.

1. **BUILD.md §10.1/10.2/10.3 implemented.** They were written during
   Phase 2 planning and never built. Mapped staging ring for the ~200 MB
   CFA upload; `SHADER_F16` with an f32 fallback compiled from the same
   source and BOTH variants validated in CI through naga; workgroup
   shared-memory tiling in the NR pass. Warm X2D II open+render 51–60 ms
   (63 ms before).
2. **Pre-demosaic NR** (`nr_raw.wgsl`) — the last Phase 4 spec item.
   `plan_passes` had scheduled `RawDenoise` since Phase 0 with no shader
   behind it. Luminance-only NR now cuts MSE to 0.318 of the un-denoised
   render, against 0.535 with the pass bypassed, and a step edge keeps its
   contrast.
3. **Benches and budgets** (BUILD.md §7). `cargo bench -p sp-pipeline`
   plus `tests/perf_budget.rs` with a relaxed and a strict tier. Both wait
   on the GPU rather than on submit — without that they measure command
   encoding and read as 0 ms.
4. **XMP ratings interop** and **Ultra HDR JPEG export**, answering two of
   the four §9 open questions by building them.
5. **README, LICENSE-MIT/APACHE**, and `vendor/rawler-patches/` — the
   three vendored rawler changes isolated against pristine 0.7.2 and
   path-rewritten for a dnglab checkout.
6. **Fuzzing found six OOM aborts, one panic and one hang**, all reaching the same
   in the decoder's TIFF layer, all now blocked by `sp_raw::preflight` or
   fixed in the vendored decoder. Three were about *what* is declared
   (strip sizes, field counts, garbage field types); one was a cyclic IFD
   chain; one was a `todo!()` panic on a non-ASCII Make tag (fixed in
   vendor/rawler as patch 0004, plus `catch_unwind` for the paths not yet
   found). Two were about *recognition*: files beginning `II*'` and
   `II\x81\0` were not TIFF by the specification, so the guard skipped them
   — while rawler parsed them happily, because its magic-number check is
   commented out upstream. **A guard in front of a parser must accept
   exactly what that parser accepts; anywhere the two disagree is a hole.**
   `fuzz/` is also its own workspace and needed its own
   `[patch.crates-io]`, or it fuzzes a different rawler than the app runs.
   The hang is in a different container path — `Jfif::parse` spins forever
   on a truncated APP1 segment, five bytes are enough — and is fixed in
   the vendored decoder (patch 0005), not in the pre-flight check: a guard
   that validated every container rawler probes would be a second decoder.
   Fuzzing has NOT been exhausted and the check does not claim to cover
   adversarial input: findings still reach the same allocation through
   rawler's offset-correction paths for maker-note IFDs, where matching
   the decoder would mean reimplementing its offset arithmetic — with the
   chance of rejecting a real maker note. That residual case needs a
   crafted file and aborts rather than corrupting memory. Closing it
   belongs upstream (bounded allocations in rawler) or in process
   isolation, not in a bigger guard. See docs/phase-8.md.

### Still open (unchanged, and none of it is code)

- **Reference Match calibration on real data.** The fitter is complete; the
  fixture folder still has no neutral Phocus TIFF exports. Export 3–10 and
  run the wizard; acceptance is mean holdout ΔE00 < 2.0.
- **By-eye EDR confirmation** on the XDR panel. `--hdr-check` reports PASS
  with ×16 potential headroom, but nobody has looked at an HDR edit.
- **Whether an HDR display actually shows the Ultra HDR boost.** The file
  is structurally correct and macOS ImageIO reads the SDR rendition; that
  needs a phone or a browser to confirm the rest.
- **Cross-machine runs** (M1 Pro / RTX 3060 / Ryzen iGPU) and **signing
  credentials**.
- Fuzzing is not finished — it is a process, not a task. Each run so far
  has found something; the corpus in `fuzz/corpus/raw_decode/` is worth
  re-seeding and re-running when the decoder changes.
