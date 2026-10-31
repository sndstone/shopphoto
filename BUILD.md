# ShopPhoto — Build Document

A GPU-accelerated, cross-platform, Lightroom-style RAW photo editor written in Rust.
This document is the single source of truth for implementing the application. It is written
so that an engineer (or an AI model) can build each phase independently, in order, with
clear acceptance criteria for every phase.

---

## 1. Product Vision

ShopPhoto is a non-destructive RAW developer and photo editor with a modern, fast,
identical UI on macOS, Windows, and Linux. The primary user shoots **Hasselblad**
medium-format cameras (X/V/H systems, 50–100 MP), so first-class support for
Hasselblad `.3FR` and `.fff` files is a hard requirement, alongside common formats
(DNG, and other Bayer RAWs as a bonus).

Headline features:

1. **Hasselblad RAW decoding** (3FR / fff / DNG) with correct color.
2. **Fully GPU-accelerated develop pipeline** (demosaic → color → tone → detail) via `wgpu`.
3. **Noise control suite**: hot-pixel removal, luminance NR, chroma NR, with masking controls.
4. **Supersampling downscale**: render at reduced resolution by averaging sensor pixels —
   100 MP → 25 MP and 50 MP → 12.5 MP via 2×2 supersampling (with 3×3 and 4×4 options).
   This both shrinks files and gains ~1 stop of noise improvement.
5. **HDR editing and export**: scene-referred float pipeline end to end, EDR/HDR display
   output where the OS supports it, HDR export (AVIF / JPEG XL / 16-bit PNG with cICP).
6. **Flexible export**: file type (JPEG, TIFF, PNG, AVIF, JXL), size/resample options,
   quality, color space, bit depth, metadata handling.
7. **Non-destructive editing** with sidecar files and a lightweight library/catalog.
8. **Fast and memory-lean**: tiled processing, f16 GPU textures, preview pyramid,
   aggressive caching with strict budgets. Target: < 2 s from double-click to first
   full-quality render of a 100 MP file on an Apple M-series or midrange dGPU.

Non-goals (v1): tethering, panorama/HDR-merge stacking, local AI masking models,
plugin systems, mobile builds, printing.

---

## 2. Technology Stack (fixed decisions — do not re-litigate)

| Concern | Choice | Rationale |
|---|---|---|
| Language | Rust (stable, edition 2021+) | Memory safety, performance, cross-platform |
| GPU | `wgpu` | One API → Metal / Vulkan / DX12; compute shaders in WGSL |
| GUI | `egui` via `eframe` with the **wgpu backend** | Pixel-identical UI on all platforms; trivially embeds our wgpu viewport; immediate mode suits a controls-heavy editor |
| RAW decode | `rawler` (pure Rust) primary; optional `libraw` FFI behind a cargo feature as fallback | rawler decodes Hasselblad 3FR/fff and DNG; pure Rust keeps builds simple. libraw feature is the safety net for odd files |
| Demosaic/pipeline | Custom WGSL compute shaders | Full control, GPU speed |
| Image encode | `image` (JPEG/PNG), `tiff`, `ravif` (AVIF), `gamut-jxl`/libjxl (JXL) | Coverage of SDR + HDR containers in the default build |
| Color math | Custom, with `moxcms` for ICC export and imported proof profiles | The internal pipeline stays scene-linear; ICC is used only at export/display boundaries |
| Catalog | SQLite via `rusqlite` | Zero-admin embedded DB |
| Sidecars | JSON (`serde`) `.spedit` files next to originals | Human-readable, non-destructive |
| Async / jobs | `std::thread` + `crossbeam-channel` worker pools (no tokio) | The app is compute-bound, not IO-bound; keep it simple |
| Error handling | `thiserror` in library crates, `anyhow` at binary boundary | Standard practice |
| Logging | `tracing` + `tracing-subscriber` | Structured, filterable |

Minimum GPU requirement: any adapter wgpu can drive with compute shader support and
`rgba16float` storage textures (this covers essentially all 2015+ hardware). CPU fallback
is **not** built; if no adapter is found, show a clear error dialog.

---

## 3. Architecture

### 3.1 Workspace layout

```
shopphoto/
├── Cargo.toml                 # [workspace]
├── crates/
│   ├── sp-core/               # shared types: image buffers, color types, edit params, errors
│   ├── sp-raw/                # RAW decoding, metadata, camera profiles (wraps rawler)
│   ├── sp-gpu/                # wgpu device mgmt, texture pool, shader pipeline graph
│   ├── sp-pipeline/           # the develop pipeline: passes, scheduling, tiling, caching
│   ├── sp-export/             # encoders, resamplers, export jobs
│   ├── sp-catalog/            # SQLite catalog, sidecar IO, file watching
│   └── sp-app/                # the eframe binary: UI, panels, viewport, state
├── shaders/                   # .wgsl files, included via include_str! from sp-gpu
├── assets/                    # icons, fonts, test images
└── docs/                      # per-phase notes written as phases complete
```

Dependency direction is strictly downward: `sp-app → {sp-pipeline, sp-catalog, sp-export} →
{sp-gpu, sp-raw} → sp-core`. No crate may depend on `sp-app`. `egui` types must never
appear outside `sp-app`.

### 3.2 Data flow (the develop pipeline)

```
 .3FR/.fff/.dng file
        │  sp-raw (CPU, worker thread)
        ▼
 RawImage { u16 CFA data, black/white levels, WB coefficients,
            color matrix (CamRGB→XYZ), CFA pattern, crop, metadata }
        │  upload as R16Uint texture
        ▼            ────────────  GPU from here on  ────────────
 [1] Raw prepare      black-level subtract, normalize to 0..1, hot-pixel suppression
 [2] Raw denoise      optional pre-demosaic NR (Phase 4)
 [3] Demosaic         Malvar/He-Cutler 5×5 (quality) or superpixel 2×2 (speed / supersample)
 [4] White balance    per-channel gains
 [5] Color transform  CamRGB → working space: Linear Rec.2020, f16, scene-referred
 [6] Tone & color     exposure, contrast, highlights/shadows, whites/blacks,
                      tone curve LUT, HSL, vibrance/saturation, white balance fine (temp/tint)
 [7] Detail           luminance/chroma NR (post-demosaic parts), sharpening (unsharp/deconv-lite)
 [8] Geometry         crop, rotate/straighten, flip
 [9] Output transform display: Rec.2020-linear → display space (SDR sRGB or HDR PQ/EDR) + dither
                      export: → target space/gamma, resample, encode
```

Every pass is a WGSL compute shader operating on `rgba16float` storage textures
(raw stages use `r16uint`/`r32float`). Pass parameters live in one uniform buffer per pass.
The pipeline is expressed as a small DAG in `sp-pipeline` so passes can be skipped when
their parameters are neutral (e.g. NR off → pass culled).

**Working space (fixed):** Linear Rec.2020, scene-referred, f16 textures, values may
exceed 1.0 (headroom preserved until the output transform). This one decision is what
makes HDR export "free" later — never bake a display gamma into intermediate stages.

### 3.3 Threading model

- **Main thread**: egui UI + wgpu presentation. Never blocks on IO or decode.
- **Decode pool** (N = physical cores / 2): RAW decode, thumbnail extraction.
- **Export pool** (N = 2): export jobs, each streaming tiles through the GPU.
- **Catalog thread**: SQLite writes, folder scanning.
- Communication via `crossbeam-channel`; UI receives results as messages drained each frame.
  No locks held across GPU submissions.

### 3.4 Camera color & the HNCS question (read before Phase 2)

Hasselblad's color rendition — **HNCS (Hasselblad Natural Colour Solution)** — is a
proprietary *render-time* pipeline that exists only inside Phocus. Verified facts
(sources in §11): the 3FR file contains raw sensor data and metadata only, **no HNCS
data**; at render time Phocus (1) selects one of several illuminant-specific color
matrices (Tungsten, Low Tungsten, Flash, Flash-Daylight) based on the white-balance
setting, (2) applies a chroma-correction LUT paired with that matrix, and (3) applies
the unpublished "Hasselblad Film Curve". Steps 1–3 re-run whenever WB changes. Neither
the matrices, the LUTs, nor the film curve are published as ICC/LUT, so **no third-party
processor can reproduce HNCS exactly** — Lightroom and Capture One don't either.

We therefore do not claim "HNCS support". We implement the same *mechanism* and offer
three profile tiers, selectable per image in a "Profile" dropdown at the top of the
Basic panel:

- **Tier 1 — ShopPhoto Standard (default, Phase 2).** Embedded camera matrix from the
  3FR/DNG metadata (CamRGB→XYZ) plus our own gentle base tone curve. Accurate, neutral.
- **Tier 2 — DCP profiles (Phase 3B).** Full support for Adobe DNG Camera Profiles
  (`.dcp`): dual-illuminant matrices (StdA + D65) interpolated by the current WB, plus
  HueSatDelta 3-D LUTs and embedded base curve. This is structurally the same
  illuminant-adaptive design as HNCS (matrix choice + chroma LUT + curve re-evaluated
  on every WB change), so profiled color "tracks" light sources the way Phocus does.
  Users can load Adobe's Hasselblad DCPs from their own Adobe installs, or any
  third-party/self-made DCP (e.g. from a ColorChecker + dcamprof). We do not
  redistribute Adobe's profiles.
- **Tier 3 — Reference Match (Phase 3B, the differentiator).** A calibration wizard: the
  user exports a handful of their own images from Phocus as 16-bit TIFF with neutral
  settings; ShopPhoto renders the same RAWs neutrally, then fits a correction
  (3×3 matrix + 33³ 3-D LUT + tone curve, optimized for minimum ΔE₀₀ across sampled
  patches) and saves it as a per-camera "Reference Match" profile. Legally clean (fits
  against the user's own renders, ships nothing of Hasselblad's) and gets visually
  very close to HNCS on real images. Fit quality is reported to the user (mean/p95 ΔE).

Pipeline consequence: pass [5] in §3.2 is not "one matrix" — it consumes a
`CameraProfile { matrices per illuminant, huesat_lut, look_lut, base_curve }` resolved
from the active tier, and its inputs are recomputed on every WB change.

### 3.5 Memory strategy (rules, enforced from Phase 2 onward)

1. Full-resolution f16 RGBA of 100 MP is ~800 MB — never keep more than **one**
   full-res working set on the GPU. Everything else uses the proxy.
2. **Proxy pipeline**: all interactive editing runs on a proxy sized to ≤ 2× the viewport
   (cheap because passes are resolution-independent). Full-res render happens only for
   export and for 1:1 zoom inspection, and 1:1 renders only the visible tile + margin.
3. **Tiling**: export and 1:1 rendering process 1024×1024 tiles (with 32 px overlap for
   spatial filters) so peak GPU memory stays < 1.5 GB even on 100 MP files.
4. CPU-side: decoded CFA (u16, ~200 MB for 100 MP) is kept for the active image only,
   in an LRU of at most 2 entries; neighbors keep only embedded JPEG previews.
5. Texture pool in `sp-gpu` recycles allocations by (size, format) — no per-frame allocs.

---

## 4. Code Quality & Style (applies to every phase)

- `cargo fmt` clean; `cargo clippy --all-targets -- -D warnings` clean. CI enforces both.
- **No `unwrap()`/`expect()`** in library crates except for provably-infallible cases with
  a comment; binaries may `expect` during startup only. All fallible APIs return
  `Result<T, SpError>` (per-crate `thiserror` enums, converted at boundaries).
- Public items documented with `///` including units and value ranges
  (e.g. `exposure_ev: f32 — stops, -5.0..=5.0, default 0.0`).
- **Units in names**: `_ev`, `_kelvin`, `_px`, `_norm` (0..1). Color values always carry
  their space in the type: `LinearRec2020(Vec3)`, never bare `[f32; 3]` across crate lines.
- Edit parameters are plain-old-data structs in `sp-core` with `serde` + `Default`;
  the `Default` impl **is** the neutral edit. Version every serialized struct
  (`version: u32`) from day one — sidecars must remain forward-readable.
- WGSL style: one entry point per file, `snake_case`, constants at top, every shader has a
  header comment stating input/output formats and color space. Shaders are validated in CI
  by a test that creates a headless device and compiles every file in `shaders/`.
- Tests: unit tests for all color math (round-trip and known-vector tests), golden-image
  tests for the pipeline (render test RAWs, compare to committed PNGs with per-pixel
  tolerance ΔE < 1.0), benchmark suite via `criterion` for decode and full-pipeline render.
- Commits per phase land as a reviewable series; each phase ends with `docs/phase-N.md`
  summarizing what was built and any deviations from this document.

---

## 5. GUI Design System

The UI must look modern, calm, and identical on all three platforms. egui is themed once,
centrally, in `sp-app/src/theme.rs`. No per-widget ad-hoc colors.

### 5.1 Design tokens

```
Background window   #1B1D1F      Panel bg          #232527
Panel raised        #2A2D30      Hairline/border   #3A3D40
Text primary        #E8E8E6      Text secondary    #9A9C9E
Accent (selection)  #4C8DFF      Accent hover      #6BA2FF
Warning             #E5A23C      Error             #E5563C
Radius: 6 px panels, 4 px controls.  Spacing unit: 8 px grid.
Font: Inter (bundled in assets/), 13 px UI, 11 px labels, tabular numerals for values.
```

Dark theme only for v1 (this is a photo editor; a light theme is a later option).
The viewport background is pure `#161718` so it never biases color judgement.

### 5.2 Layout (single main window)

```
┌─────────────────────────────────────────────────────────────────────┐
│ Top bar: app name · Library|Develop toggle · export btn · zoom ctrl │ 40 px
├───────────┬─────────────────────────────────────────┬───────────────┤
│ Left rail │                                         │  Right panel  │
│ (Library: │           VIEWPORT (wgpu texture)       │  (Develop:    │
│  folders, │   checkerboard-free, color-managed,     │   histogram,  │
│  collec-  │   pan/zoom, 1:1 toggle, before/after    │   tool groups)│
│  tions)   │                                         │   320 px      │
│  240 px   │                                         │               │
├───────────┴─────────────────────────────────────────┴───────────────┤
│ Filmstrip: thumbnails of current folder, ratings under each  96 px  │
└─────────────────────────────────────────────────────────────────────┘
```

- **Library mode**: viewport becomes a thumbnail grid; right panel shows metadata.
- **Develop mode**: right panel hosts collapsible tool groups, top to bottom:
  *Histogram* (live, RGB overlay) → *Basic* (WB temp/tint, exposure, contrast, highlights,
  shadows, whites, blacks) → *Tone Curve* → *Color* (HSL 8-band, vibrance, saturation) →
  *Detail* (sharpening, noise reduction) → *Resolution* (supersampling mode) →
  *Geometry* (crop/rotate) → *HDR* (Phase 6).
- Sliders: label left, value right (editable on click), double-click resets to default,
  ⌥/Alt-drag = fine step. Every parameter change is applied live (< 100 ms on proxy).
- Left/right rails are collapsible (`Tab` hides both, Lightroom-style). `\` toggles
  before/after. `F` full-screens the viewport.
- Keyboard: 0–5 rating, ←/→ navigate filmstrip, `E` export dialog, `D`/`G` develop/grid.

---

## 6. Phases

Build strictly in order. Each phase must compile, pass CI, and meet its acceptance
criteria before the next begins. Estimated sizes are for scoping, not deadlines.

---

### Phase 0 — Scaffolding & Standards  (small)

**Objective:** A building, testing, releasable skeleton on all three OSes.

**Build steps:**
1. Create the cargo workspace and all seven crates with the dependency graph of §3.1
   (empty modules, `lib.rs` doc comments stating each crate's purpose).
2. `sp-app`: eframe window with the wgpu backend, dark theme from §5.1 applied,
   empty three-pane layout of §5.2 with placeholder panels.
3. CI (GitHub Actions): fmt + clippy + test on macOS, Windows, Ubuntu; release build
   artifacts per OS.
4. `justfile` (or cargo aliases): `just run`, `just test`, `just lint`, `just bench`.
5. Commit the code-style rules of §4 as `CONTRIBUTING.md`.

**Acceptance:** `cargo run` opens a themed, resizable window with the three-pane layout
on all OSes; CI is green; clippy `-D warnings` passes.

---

### Phase 1 — RAW Decoding Core  (medium)

**Objective:** Decode Hasselblad and DNG files to a well-defined `RawImage` on the CPU,
with verified correctness. No GPU work yet — display the embedded JPEG preview.

**Build steps:**
1. `sp-core`: define `RawImage` (CFA `Vec<u16>`, width/height, CFA pattern enum,
   black/white levels per channel, `wb_coeffs: [f32; 4]`, `cam_to_xyz: [[f32;3];3]`,
   active-area crop, orientation) and `ImageMeta` (camera, lens, ISO, shutter, aperture,
   capture time, GPS optional).
2. `sp-raw`: wrap `rawler`. One entry point:
   `decode(path) -> Result<(RawImage, ImageMeta, Option<EmbeddedPreview>)>`.
   Map rawler's per-camera data into our types; normalize CFA pattern naming.
3. **Hasselblad verification harness** (critical): a `sp-raw/tests/` suite that runs
   against a local folder of sample files (env var `SP_TEST_RAWS`, not committed).
   Acquire samples for: X2D 100C (.3FR, 100 MP), X1D II (.3FR, 50 MP), a Phocus-processed
   `.fff`, and a Hasselblad-exported DNG. Assert: dimensions, CFA pattern, sane black/white
   levels, WB coeffs present, color matrix present. If rawler fails on any Hasselblad file,
   enable the `libraw` cargo feature and route Hasselblad extensions through libraw FFI —
   the `sp-raw` public API must not change either way.
4. Async decode: decode pool (§3.3) + message back to UI. Open-file dialog (`rfd` crate)
   and drag-and-drop onto the window.
5. Show the embedded preview JPEG in the viewport (as an egui texture for now), with
   metadata (camera, ISO, shutter, f-stop, dimensions) in the right panel.

**Acceptance:** Dropping an X2D `.3FR` shows its preview and correct metadata in < 300 ms;
the verification suite passes on all sample files; decode errors surface as a readable
toast, never a panic.

---

### Phase 2 — GPU Pipeline Foundation  (large; the heart of the app)

**Objective:** Real demosaiced, color-correct, scene-referred rendering on the GPU,
displayed in a pannable/zoomable color-managed viewport. After this phase the app is a
correct (if minimal) RAW viewer.

**Build steps:**
1. `sp-gpu`: device/queue init (prefer high-performance adapter), texture pool
   (recycling by size+format), uniform-buffer helper, compute-pass runner, and a
   `PipelineGraph` type: ordered list of passes, each `{ shader, inputs, output, params }`,
   with pass culling when params are neutral.
2. Shaders (in `shaders/`), in pipeline order §3.2:
   - `raw_prepare.wgsl`: R16Uint CFA → R32Float normalized (black-level subtract,
     white-level scale), plus simple hot-pixel clamp (median-of-neighbors threshold).
   - `demosaic_malvar.wgsl`: Malvar–He–Cutler 5×5 linear demosaic → rgba16float.
     (Chosen over AMaZE for v1: excellent quality/complexity ratio, GPU-friendly.)
   - `demosaic_superpixel.wgsl`: 2×2 Bayer quad → 1 RGB pixel (G averaged). Used for
     fast previews **and** as the 2×2 supersampling path in Phase 5.
   - `wb_color.wgsl`: WB gains → 3×3 CamRGB→XYZ→Linear Rec.2020 matrix (matrices
     multiplied on CPU, uploaded as one mat3). Phase 2 implements profile **Tier 1**
     only (§3.4: embedded matrix + base curve), but the CPU-side code must already go
     through a `CameraProfile` abstraction so Phase 3B can add DCP/Reference Match tiers
     without touching shaders other than adding the optional HueSat-LUT pass.
   - `output_transform.wgsl`: Linear Rec.2020 → display. v1: sRGB with proper OETF +
     blue-noise dither to 8 bit. Structure it so Phase 6 adds PQ/EDR variants.
3. Proxy logic in `sp-pipeline`: on image open, run superpixel demosaic at quad res
   (100 MP → 25 MP) as the instant first frame, then refine with Malvar full-res-
   downsampled proxy in the background. Viewport at 1:1 zoom renders the visible tile
   full-res (tiling per §3.5).
4. Viewport widget in `sp-app`: draws the pipeline's output texture via a custom egui
   paint callback (no CPU readback). Scroll-wheel zoom about cursor, drag pan, `Z` toggles
   fit/1:1, smooth animated transitions (150 ms ease-out).
5. Color management of the viewport: query the surface; on macOS request extended-sRGB
   f16 surface; elsewhere sRGB 8-bit. The output transform shader is the only place
   display encoding happens.
6. Golden-image tests: 3 committed small RAWs (or synthetic CFA fixtures) rendered
   through the full pipeline vs. committed reference PNGs, ΔE tolerance < 1.0.
7. Benchmarks: time-to-first-frame and full-quality render for 100 MP input.

**Acceptance:** X2D file → recognizable, color-correct image in < 500 ms (superpixel),
full quality < 2 s; pan/zoom at 60 fps; golden tests pass; GPU memory for a 100 MP file
in fit view < 600 MB.

---

### Phase 3 — Develop Controls & Non-Destructive Editing  (large)

**Objective:** The core editing experience: Basic panel, tone curve, HSL, crop —
all live, all non-destructive, saved to sidecars with undo/redo.

**Build steps:**
1. `sp-core`: `EditState` struct — every parameter with documented range and neutral
   default (§4). Include: wb (temp 2000–50000 K, tint −150..150), exposure_ev (−5..5),
   contrast, highlights, shadows, whites, blacks (−100..100), tone curve (Catmull-Rom
   control points), hsl (8 hue bands × hue-shift/sat/lum), vibrance, saturation,
   sharpening (amount/radius/detail), crop/rotate, plus placeholders (`nr: NrParams`,
   `resolution: ResolutionParams`, `hdr: HdrParams`) so the sidecar schema is stable.
2. Shaders: `tone.wgsl` (exposure/contrast/H-S-W-B implemented as a single analytically-
   built 1-D LUT computed on CPU per param change, 1024-entry f32, applied in shader —
   this makes six sliders cost one texture lookup), `curve_hsl.wgsl` (curve LUT + HSL in
   a hue-linear space: use Oklab LCh for band weighting to avoid hue skew),
   `sharpen.wgsl` (unsharp mask on luminance only). Geometry (crop/rotate) is a vertex-
   level transform in the viewport + an export-time resample, not a filter pass.
3. Wire the Develop right panel per §5.2: collapsible groups, slider spec (double-click
   reset, fine drag, live value edit). Histogram widget: compute shader reduces the
   output texture to a 256-bin RGB histogram each render; draw as filled curves.
4. Edit engine in `sp-app`: single `EditState` for the open image; every widget mutation
   → schedule proxy re-render (coalesced to one render per frame). Undo/redo stack of
   `EditState` snapshots (they're tiny), `Cmd/Ctrl-Z / Shift-Z`, coalescing continuous
   slider drags into one undo step on release.
5. Sidecars in `sp-catalog`: `photo.3FR.spedit` JSON `{ version, edit_state, rating,
   flags }`, saved debounced (500 ms) after changes, loaded on open. Never touch the
   original file.
6. Before/after (`\` key: render with neutral EditState to a second texture and swap)
   and a "Reset all" button with confirmation.

**Acceptance:** All listed controls work live at ≥ 30 fps on the proxy for a 100 MP file;
edits survive app restart via sidecar; undo/redo correct across 100+ steps; golden tests
extended with one "edited look" reference.

---

### Phase 3B — Camera Color Profiles & Reference Match  (medium)

**Objective:** Implement profile Tiers 2 and 3 of §3.4 so Hasselblad files can be
rendered with illuminant-adaptive, HNCS-like color instead of a single static matrix.

**Build steps:**
1. `sp-core`: `CameraProfile` full definition — `{ name, camera_id, matrices:
   Vec<(illuminant, mat3)>, forward_matrices, huesat_lut: Option<HueSatLut>,
   look_lut: Option<Lut3d>, base_curve: Option<Curve> }` + a resolver
   `profile.resolve(wb_temp_kelvin) -> ResolvedColorParams` that interpolates between
   the two nearest illuminant matrices by inverse correlated color temperature
   (the DNG-spec method) and returns the concatenated mat3 + LUT handles. Re-resolved
   on **every** WB change — this reproduces HNCS's illuminant-adaptive behavior.
2. **DCP reader** in `sp-raw`: parse Adobe DNG Camera Profile files (.dcp is a TIFF/EP
   container of DNG tags: ColorMatrix1/2, ForwardMatrix1/2, ProfileHueSatMapData1/2,
   ProfileLookTableData, ProfileToneCurve, CalibrationIlluminant1/2). Map into
   `CameraProfile`. Unit-test against a known public DCP with reference values.
3. New shader `huesat_lut.wgsl`: applies the DNG HueSatDelta transform (hue rotation,
   sat scale, value scale, in the DNG-prescribed HSV space) + optional 3-D look LUT
   (trilinear sample of a 33³ rgba16f 3D texture). Runs inside pass [5]/[6] order per
   the DNG spec (huesat before tone curve, look table after).
4. **Profile UI**: dropdown at the top of the Basic panel — "ShopPhoto Standard",
   any imported .dcp ("Import profile…" file picker; profiles copied into app data,
   listed per matching camera model), and any fitted "Reference Match" profiles. Selection
   stored in `EditState` (versioned; missing profile falls back to Standard + warning
   badge, never an error).
5. **Reference Match wizard** (modal, launched from the profile dropdown):
   a. Instructs the user: pick 3–10 varied RAWs, export each from Phocus as 16-bit TIFF,
      settings zeroed/neutral, into one folder. (Show a screenshot-style illustrated
      checklist of exact Phocus settings.)
   b. User points the wizard at the RAWs + TIFF folder; files are paired by name.
   c. Fit: render each RAW through our neutral Tier-1 pipeline to linear Rec.2020;
      decode each Phocus TIFF to the same space (honor its embedded ICC). Sample ~50k
      spatially-distributed pixel pairs per image (reject near-clip and high-gradient
      pixels). Optimize: first a 3×3 matrix (least squares), then a 33³ 3-D LUT on the
      residual (smoothness-regularized, e.g. Laplacian penalty), then a 1-D tone curve.
      Pure-CPU `nalgebra` implementation is fine; this is an offline step.
   d. Report mean / p95 ΔE₀₀ on a holdout subset; save as
      `ReferenceMatch-<camera>.spprofile` (serde JSON + binary LUT blob).
6. Golden tests: fixture RAW rendered under Tier 1 vs a synthetic DCP with known
   matrices — assert expected pixel values; Reference Match fitter tested on synthetic
   data (apply a known matrix+LUT distortion, fit must recover it, ΔE < 0.5).

**Acceptance:** An X2D file rendered with an imported Adobe Hasselblad DCP visibly and
measurably differs from Standard per the DCP's intent (hue-sat map applied, WB-dependent
matrix interpolation verified by test); the Reference Match wizard, run on real Phocus
exports, achieves mean ΔE₀₀ < 2.0 on holdout images and the result is selectable and
persisted like any profile. WB slider changes re-resolve the profile live with no
visible lag.

---

### Phase 4 — Noise Control Suite  (medium-large)

**Objective:** Serious noise reduction that flatters medium-format files at high ISO,
controllable and honest (no plastic skin by default).

**Build steps:**
1. `NrParams`: hot_pixels (bool, default on), luminance (0–100), luminance_detail
   (0–100, default 50), chroma (0–100, default 25), chroma_detail (0–100).
   Defaults auto-seeded from ISO metadata (e.g. chroma = f(ISO), documented table).
2. Shaders:
   - Hot-pixel removal already in `raw_prepare` (Phase 2) — extend with ISO-scaled
     threshold and make it toggleable.
   - `nr_chroma.wgsl`: convert to Oklab, edge-aware bilateral blur on a/b channels only,
     radius scaled by chroma param. Cheap, extremely effective, zero detail loss in L.
   - `nr_luma.wgsl`: non-local-means *lite* on L: 5×5 patch, 9×9 search window, strength
     from luminance param; `luminance_detail` blends back high-frequency residual.
     Implement as two passes (weights, then blend) with workgroup-shared memory tiles.
   - Run NR **after** demosaic + color transform, **before** tone (NR in scene-linear
     Oklab is more predictable than post-curve).
3. UI: Detail group gets NR sliders + a 100% "noise loupe" preview square that always
   renders 1:1 regardless of viewport zoom, so the user judges NR at pixel level.
4. Tests: synthetic noise fixture (clean gradient + seeded Gaussian/Poisson noise);
   assert PSNR improvement bounds and that NR-off is bit-identical to Phase 3 output.
   Benchmark: NR passes add < 150 ms to a full-quality 100 MP render on baseline GPU.

**Acceptance:** ISO 3200 X2D file: chroma noise removable with no visible edge damage;
luminance slider trades noise/detail smoothly; loupe updates live; perf budget met.

---

### Phase 5 — Supersampling & Export Engine  (medium-large)

**Objective:** The signature resolution feature plus a complete, robust exporter.

**Supersampling design:**
- `ResolutionParams { mode: Full | Super2x2 | Super3x3 | Super4x4 }`.
- **Super2x2** (the headline: 100 MP → 25 MP, 50 MP → 12.5 MP): use the *superpixel
  demosaic* path — each Bayer 2×2 quad (R, G, G, B) becomes exactly one RGB pixel with
  the two greens averaged. No interpolation, no demosaic artifacts, and averaging gives
  ~0.5–1 stop noise benefit. This is mathematically the cleanest possible 2× downscale
  from a Bayer sensor and is *faster* than full demosaic.
- **Super3x3 / Super4x4**: full demosaic first, then box-average 3×3 / 4×4 in linear
  space (box is correct here because it models sensor binning; no ringing).
- The chosen mode applies to the **working resolution**: preview, NR, sharpening, and
  export all operate at the supersampled size, so NR/sharpen params behave consistently.
  Changing mode invalidates caches and re-renders.
- UI: "Resolution" group with four radio options, each showing the computed output size
  for the current file ("2×2 — 5760 × 4320 · 24.9 MP").

**Export engine (`sp-export`):**
1. Formats: JPEG (quality 1–100, 4:4:4/4:2:0), TIFF (8/16-bit, zip/none), PNG (8/16),
   AVIF (SDR/HDR) and 16-bit JXL (lossy/lossless SDR and PQ HDR).
2. Sizing: original (post-supersampling), fit-within long-edge px, exact WxH, or
   megapixel target; Lanczos-3 resample in linear space (GPU pass) when export size ≠
   working size.
3. Color: sRGB / Display P3 / Rec.2020 output with correct ICC profile embedded
   (generate static profiles with `moxcms`/`lcms2`).
4. Metadata: copy EXIF from source (camera/lens/exposure), strip-GPS option, add
   software tag; via `kamadak-exif` writing or `little_exif`.
5. Export dialog (modal): format, quality, size mode, color space, bit depth, metadata
   toggles, destination folder, filename pattern (`{name}`, `{date}`, `{seq}`).
   Presets: save/recall named export presets (stored in catalog DB).
6. Jobs: export runs on the export pool, tiled through the GPU (§3.5), with a progress
   toast (per-file progress, cancel). Batch: export all selected filmstrip images.
7. Tests: round-trip dimension/ICC assertions per format; golden export of the test
   image per format; 16-bit TIFF must round-trip pipeline output within 1 LSB.

**Acceptance:** 100 MP → 25 MP Super2x2 JPEG export in < 4 s; all format/size/space
combinations produce files that open correctly in macOS Preview, Windows Photos, and
Affinity/Photoshop with correct color; batch of 50 files completes with accurate
progress and cancel works cleanly.

---

#### Phase 5 refinements (post-Phase-4 learnings — read before starting)

1. **A full-resolution render path does not exist yet.** The interactive
   pipeline is proxy-only (superpixel → 2048 px) plus single visible-region
   tiles. Export must add a TILED full-image loop: iterate 1024² regions via
   the existing `render_tile` machinery (margins already handled), read each
   tile back, and assemble into a CPU image buffer. Reuse `render_tile`
   verbatim — it is golden-tested including odd-origin CFA parity; do NOT
   write a second demosaic path.
2. **Resolution modes**: Super2x2 export = tile over the QUARTER-res
   superpixel image (the `quarter_tex` kept from open) through the color
   chain — no Malvar. Full/3×3/4×4 = Malvar tile loop (+ box downsample for
   3×3/4×4 via the existing `downsample.wgsl`). The Resolution radio
   currently only affects planning, not the proxy — that stays true;
   working-res applies to EXPORT and the loupe/1:1 paths in v1
   (document in-UI: "applies to export & 1:1").
3. **Bit depth**: the display output transform writes dithered 8-bit sRGB.
   16-bit TIFF/PNG need a NEW output-transform variant writing
   `rgba16uint` (or readback of the f16 working texture + CPU encode) with
   NO dither and NO 8-bit rounding. Add a `mode` uniform to
   `output_transform.wgsl` rather than a new file; keep the display path
   byte-identical (golden tests must not change).
4. **Export color must equal viewport color.** Same passes, same LUTs, same
   white-preserving matrix (`display_cam_to_xyz`). Only the LAST encode
   step differs per color space: add Rec2020→P3 and identity (Rec2020)
   variants next to the existing →709 matrix, each with its matching ICC
   profile embedded (use `moxcms` — already a dependency via the Reference
   Match TIFF reader).
5. **Geometry becomes real here.** Crop/rotate/flip are display-only today.
   Export order: render working-res image → single GPU resample pass
   (bilinear, rotation + flip + crop in one affine map) → sizing resample
   (Lanczos-3; separable two-pass shader) → encode. The 1:1 viewport and
   loupe stay unrotated by design.
6. **Sharpening gap (carried from Phase 3):** `SharpenParams` exist and the
   sliders are live, but there is NO sharpen shader. Implement
   `sharpen.wgsl` (luminance unsharp mask, radius in working-res px,
   detail-mask on flat areas) in THIS phase so exports honor the sliders;
   insert after NR, before tone? No — after tone, before output (sharpen on
   display-referred luma matches user expectation); cull at amount == 0.
7. **Concurrency**: run exports on a second `GpuRenderer` created from the
   SAME wgpu device (device/queue are cloneable and thread-safe) on the
   export worker thread; do not time-slice the UI renderer. Peak VRAM =
   proxy set + one export tile set — within budget. Progress via the
   existing crossbeam message pattern; cancel via `Arc<AtomicBool>` checked
   between tiles.
8. **Encoders — verified availability**: `image` 0.25 is already in-tree
   (JPEG/PNG/TIFF encode features must be enabled). Its JPEG encoder does
   NOT expose chroma-subsampling control; either accept 4:2:0 in v1 (drop
   the `chroma_444` toggle from the UI, keep it in `ExportFormat` for
   later) or bring in `mozjpeg` behind a feature. AVIF via `ravif` is
   CPU-slow at 100 MP — encode Super2x2-sized output by default and warn in
   the dialog for Full-res AVIF.
9. **EXIF**: copy via `little_exif` (rawler's parsed `Exif` → tag writes:
   camera, lens, exposure triangle, capture time; honor `strip_gps`). Add
   `Software = ShopPhoto`. Verify byte-level with a re-parse test.
10. **Presets live in the config dir for now** (`profiles_dir()`'s parent /
    `export_presets.json`), migrating into the catalog DB in Phase 7 —
    do not block on the DB.
11. **Tests**: per-format re-decode round-trips (dimensions, ICC tag
    presence via moxcms parse, 16-bit TIFF within 1 LSB of the f16
    readback); a real-file export smoke test behind `SP_TEST_RAWS` writing
    into `SP_DUMP_DIR` for visual review (extend `render_dump.rs` pattern);
    geometry test (synthetic asymmetric image + crop/flip → pixel-mapped
    expectations).

### Phase 6 — HDR Editing, Display & Export  (medium-large)

**Objective:** True HDR: edit with highlight headroom visible on EDR/HDR displays,
export standard HDR files that look right in Chrome/Photos/Instagram-class consumers.

**Build steps:**
1. **Display — use wgpu's native HDR surface API** (verified available as of wgpu 30;
   pin the workspace to a wgpu version whose `SurfaceConfiguration` has the
   `color_space` field and `Surface::display_hdr_info` — check at Phase 0 and record
   the pin in `docs/phase-0.md`):
   - Configure the surface `Rgba16Float` + `SurfaceColorSpace::ExtendedSrgbLinear`
     (scRGB: linear, 1.0 = SDR white, values > 1.0 = HDR). On macOS this maps directly
     to Metal EDR and works on all XDR/miniLED Macs. On Windows this is the scRGB
     HDR swapchain path; `SurfaceColorSpace::Bt2100Pq` on `Rgb10a2Unorm` is the
     fallback if a driver lacks float scRGB. Query
     `SurfaceCapabilities::format_capabilities` and pick the best supported pair at
     startup and on monitor change. Linux: SDR only for v1 (note in UI).
   - Read `Surface::display_hdr_info` (populated on Metal; nits, EDR headroom,
     primaries) for the tone mapper's live headroom value — do **not** hand-roll
     `CAMetalLayer`/DXGI queries.
   - **egui composite architecture (required).** egui's renderer has known breakage
     and gamma mistakes when targeting an `Rgba16Float`/HDR surface (egui #7484,
     #3168). Do not render egui directly to the HDR surface. Instead: egui renders the
     whole UI to an offscreen `Rgba8UnormSrgb` texture (viewport region transparent);
     our final `composite.wgsl` pass writes the HDR surface directly — samples the
     develop-pipeline output (already display-linear from the output transform) for
     the viewport rect, converts the egui layer sRGB→linear clamped to ≤ 1.0
     (UI chrome must never exceed SDR white), and alpha-blends UI over image. This
     isolates us from egui's surface-format assumptions entirely and keeps UI
     brightness constant next to HDR imagery.
2. `HdrParams { enabled: bool, target: Sdr | Hdr, headroom_stops: f32 (0..4),
   highlight_rolloff: f32 }`. When HDR is enabled and the display supports it, the
   output transform maps scene-linear → display-linear preserving up to
   `headroom_stops` above SDR white with a smooth shoulder; otherwise it tone-maps to
   SDR so the edit still looks intentional on SDR screens (single fixed Reinhard-style
   shoulder — do not invent a look system).
3. UI: HDR group with an enable toggle, headroom slider, and a persistent badge in the
   top bar showing "HDR: display 4.0 stops" or "SDR display". Histogram gains a >1.0
   region rendered beyond a marked SDR-white line.
4. **Export**:
   - AVIF HDR: 10-bit, PQ (Rec.2100), cICP tagged — the most compatible HDR output today.
   - JPEG XL HDR and 16-bit PNG with cICP (PQ) as secondary options.
   - SDR exports of HDR edits use the same SDR fallback mapping as the display path
     (WYSIWYG guarantee).
   - Optional stretch goal (only if schedule allows): Ultra HDR JPEG (gain map) for
     maximum compatibility — isolate in its own module.
5. Tests: PQ OETF/EOTF unit vectors (BT.2100 reference values); exported AVIF verified
   HDR by re-decoding and checking cICP + >1.0 content survives; manual test matrix
   documented in `docs/phase-6.md` (macOS XDR, Windows HDR monitor, SDR laptop).

**Acceptance:** On an XDR MacBook display, specular highlights in a sunset X2D file
visibly exceed SDR white while UI chrome stays at SDR levels; exported AVIF shows HDR
in Chrome on the same machine; the same edit exported as sRGB JPEG looks correct, not
washed out.

---

#### Phase 6 refinements (post-Phase-4 learnings)

1. **CRITICAL prerequisite — highlight handling rework.** Phase 4's
   `wb_color.wgsl` clamps post-WB channels at 1.0 (the magenta-highlight
   fix). That clamp DESTROYS the scene-referred headroom HDR needs. Replace
   it with proper highlight reconstruction before any HDR work:
   per-pixel, if only some channels are sensor-clipped, reconstruct the
   clipped ones from unclipped neighbors' ratios up to `min(gain_i)`
   headroom, desaturating toward neutral as all channels approach clip
   (dcraw "blend" mode is the reference algorithm). Keep the SDR result
   visually identical for fully-clipped whites (golden: flat-clip fixture
   must stay neutral white in SDR).
2. **Histogram source must move.** It currently reduces the 8-bit sRGB
   output texture; for HDR it must read the pre-output working texture
   (`work2`) so >1.0 content lands in the promised over-white region.
   Bins: 0..1 sRGB-encoded in 224 bins + 1.0..headroom in the last 32
   (documented mapping), SDR-white tick drawn by the UI.
3. **Composite architecture specifics** (extends the existing plan): the
   HDR viewport quad is drawn via `egui_wgpu::Callback` (paint callback)
   sampling the f16 working texture — NOT via `register_native_texture`
   (that API requires `Rgba8Unorm`; it remains the SDR/loupe/thumbnail
   path). egui renders to its own SDR pass; the callback draws beneath it
   in the same render pass with scRGB values. Verify the callback runs
   inside eframe's surface pass (egui 0.35 `Callback` executes in-pass —
   no separate composite shader needed on macOS as long as UI stays ≤ 1.0;
   revisit the offscreen-UI plan ONLY if egui's own pipeline mangles float
   surfaces in practice).
4. **The SP_SCREENSHOT hook captures via egui = SDR-only.** HDR QA must be
   export-based: decode the AVIF/PQ output, verify cICP tags and that
   >SDR-white content survives numerically (extend the export round-trip
   tests). Manual matrix on the user's displays remains required.
5. `standard_base_curve()` and the tone LUT operate below 1.0 and
   pass headroom through multiplicatively — audit `build_tone_lut`'s
   above-white branches (curve endpoint scaling) against the chosen
   `headroom_stops` so HDR white doesn't double-scale.
6. Surface reconfiguration on monitor change: eframe owns the surface;
   toggling `SurfaceConfiguration.color_space` at runtime goes through
   `RenderState.surface_config` — test window-dragged-between-displays.

### Phase 7 — Library & Catalog  (medium)

**Objective:** Enough DAM to work a shoot: browse folders, grid, ratings, filters,
collections. Not a Lightroom-catalog replacement.

**Build steps:**
1. `sp-catalog` schema (SQLite): `photos` (path, hash, meta columns, rating, flags,
   sidecar mtime), `folders`, `collections`, `collection_photos`, `export_presets`.
   Migrations via `rusqlite_migration` from v1.
2. Folder import: user adds a folder → background scan (recursive), thumbnail extraction
   (embedded JPEGs decoded at ~512 px, cached on disk in app-data dir with an LRU size
   cap of 2 GB), incremental rescan on app focus + `notify` file watching.
3. Library mode UI: left rail lists folders + collections; viewport becomes a virtualized
   thumbnail grid (only visible thumbs decoded/uploaded; recycle textures on scroll);
   grid cells show rating stars + edited badge. Filter bar: rating ≥ N, flagged, file
   type. Sort: capture time, name, rating.
4. Filmstrip (both modes) driven by the same virtualized model; ←/→ moves selection and
   opens in Develop; ratings 0–5 and flag `P`/`X` work in both modes and persist to
   sidecar + DB.
5. Collections: manual only (drag thumbs onto a collection). No smart collections in v1.

**Acceptance:** A folder of 2,000 RAWs scans with a progress indicator and remains
scrollable at 60 fps during scan; ratings/filter/sort work and persist; grid memory
stays < 500 MB with 2,000 items via virtualization.

---

#### Phase 7 refinements (post-Phase-4 learnings)

1. **The decode "pool" is one thread** (`sp-app/src/jobs.rs`). Grow it into
   a real pool here: N = cores/2 workers, a priority queue (visible
   thumbnails first, then prefetch neighbors), and cancellation tokens for
   scrolled-away requests. Keep the existing message-drain pattern.
2. **Thumbnails**: embedded previews are already decoded, EXIF-rotated
   RGBA (Phase 4 fixed rotation) — the thumbnail path only needs the
   nearest-downscale + disk cache (content-hash keyed .webp/.png files in
   the app cache dir, 2 GB LRU cap). Filmstrip session behavior becomes a
   "Recent" collection; the strip itself reads the catalog model.
3. **Sidecar `Annotations` already round-trip** through save/load — the DB
   only mirrors them for query speed. Sidecars remain the source of truth;
   DB rebuilds must be lossless from folder + sidecars alone (test this).
4. `notify` on macOS uses FSEvents with path-level granularity — debounce
   (500 ms) and re-stat rather than trusting event kinds; rescan-on-focus
   stays the reliability backstop.
5. Ratings/flags keyboard handling: the global-shortcut guard
   (`egui_wants_keyboard_input`) already exists; add 0–5/P/X there, write
   through to sidecar + DB, and show stars in filmstrip cells and grid.
6. Export presets migrate from the Phase 5 JSON file into the DB with a
   one-time import (keep reading the JSON as fallback for a version).

### Phase 8 — Performance, Memory & Polish Hardening  (medium)

**Objective:** Make the §1 performance promises true on all platforms, then freeze.

**Build steps:**
1. Instrument: `tracing` spans around decode/upload/each pass/present; hidden debug
   overlay (`F12`) with frame time, VRAM estimate, cache hit rates, pass timings.
2. Optimize against budgets (baseline: M1 Pro / RTX 3060 / Ryzen iGPU floor):
   - open → first frame < 500 ms (100 MP), full quality < 2 s
   - slider → proxy update < 100 ms end-to-end
   - export 25 MP JPEG < 4 s; app idle RAM < 400 MB with one image open
   Techniques as needed: f16 everywhere post-demosaic, pass fusion (WB+color+tone into
   one shader when NR off), async texture upload, precompiled pipeline cache, LUT sizes
   tuned, avoid egui full-repaint on slider drag (request repaint of viewport only).
3. Memory audit: run the §3.5 rules with 100 MP files under a VRAM-capped adapter test;
   fix any full-res residency violations.
4. Robustness: fuzz the RAW loader with `cargo-fuzz` on truncated/corrupt files
   (must error, never crash); GPU device-lost recovery (recreate device, re-render);
   sidecar corruption → quarantine + neutral edit, never data loss.
5. UX polish pass: loading skeletons, empty states, error toasts reviewed for tone,
   all interactions keyboard-reachable, in-app shortcut cheat-sheet (`?`).

**Acceptance:** All budget numbers hit on the three baseline machines and recorded in
`docs/phase-8.md`; fuzzer runs 1 h without crash; 30-minute editing session shows no
RAM/VRAM growth trend.

---

#### Phase 8 refinements — measured hotspot list (from sessions 3–8)

Profile first, but these are already KNOWN and sized:

1. **Histogram readback is blocking** (~1 ms map + poll per edit change,
   on the UI thread). Make it async: map in `logic`, consume next frame.
2. **Per-render buffer churn**: every render creates ~8 uniform/storage
   buffers via `create_buffer_init` (wb, LUT, tone, 2× profile, NR, out,
   hist) plus 3-D LUT textures on profile changes. Pool them: persistent
   buffers + `queue.write_buffer`; rebuild LUT textures only when the
   resolved profile changes (hash the resolved params).
3. **Tone LUT rebuilds on every render** even when only WB moved. Cache by
   (ToneParams, curve, base-curve) hash.
4. **Decode-time rotation costs ~250 ms on 105 MP** (single-threaded
   transpose in `apply_orientation`). Parallelize with rayon
   (`par_chunks_mut` over output rows) or move to a GPU pre-pass; also
   skip the copy entirely for `Orientation::Normal` (already done) and
   consider rotating during CFA upload instead of a separate buffer.
5. **NLM shared-memory tiling**: `nr_main.wgsl` re-loads each texel ~9×
   per patch comparison. If NR tile renders exceed ~25 ms at 100%, cache
   the 20×20 luma neighborhood in `var<workgroup>` (BUILD.md §10.3).
6. **`PROXY_LONG_EDGE` is fixed at 2048** — make it viewport-driven
   (≤ 2× viewport, BUILD.md §3.5) now that `state.viewport_size` exists.
7. **`quarter_tex` stays resident** (~210 MB for 105 MP f16). After
   Phase 5 wires Super modes, drop it post-proxy-build for `Full` mode and
   rebuild on demand.
8. **Visual regression harness exists — formalize it**: `render_dump.rs`
   (`SP_TEST_RAWS`/`SP_DUMP_DIR`) + the `SP_SCREENSHOT` app hook. Add a
   hash-with-tolerance compare against committed references so UI and
   pipeline regressions fail CI, not user QA. (The screenshot hook found
   the light-theme bug; treat it as a first-class tool.)
9. Startup: profile store + pipeline compilation happen on the UI thread
   at launch — measure; move profile loading to a worker if > 50 ms.
10. Keep the sp-core/sp-raw/sp-pipeline `opt-level = 2` dev overrides in
    mind when interpreting dev-build timings (workspace `"*"` covers
    dependencies only — a recurring trap).

### Phase 9 — Packaging & Release  (small-medium)

**Objective:** Installable, signed, updatable builds.

**Build steps:**
1. macOS: universal binary (arm64 + x86_64), `.app` bundle, codesign + notarize, DMG.
2. Windows: MSIX or NSIS installer, code-signed; file associations for .3FR/.fff/.dng.
3. Linux: AppImage + Flatpak manifest.
4. CI release workflow: tag → build/sign/upload all artifacts; version from git tag;
   in-app "check for updates" (simple version-check against a static JSON URL; no
   auto-download in v1).
5. First-run experience: welcome screen → pick a folder → guided first edit tooltip pass.
6. `docs/` finalized: architecture (updated to as-built), shortcut list, file-format
   support matrix, known limitations.

**Acceptance:** A clean machine per OS: download → install → open an X2D file → edit →
export, with no dev tools installed and no security warnings beyond first-run norms.

---

#### Phase 9 refinements (post-Phase-4 learnings)

1. A hand-rolled `dist/ShopPhoto.app` bundle already exists in the repo
   from ad-hoc testing — REPLACE it with a scripted pipeline
   (`cargo bundle` or a `just bundle` recipe generating Info.plist,
   icons via `iconutil`, codesign, notarytool) and gitignore `dist/`;
   binaries must never be committed.
2. Universal macOS binary: build `aarch64-apple-darwin` +
   `x86_64-apple-darwin`, `lipo -create`; CI needs both targets installed
   (`rustup target add x86_64-apple-darwin` on the arm64 runner).
3. File associations (3fr/fff/dng) via CFBundleDocumentTypes; the CLI
   file-open path (`shopphoto <file>`) already works — macOS "Open with"
   arrives as an Apple Event, NOT argv: handle
   `egui::Event`/eframe file-open events (`raw.dropped_files` covers
   drag-to-dock-icon on macOS; verify, else add objc glue exception to the
   §10.6 ban with a tracking issue).
4. First-run: request no permissions (no camera/photos entitlements);
   hardened runtime + `com.apple.security.cs.allow-jit` NOT required
   (no JIT); sandbox off in v1 (file access breadth), revisit for App
   Store later.
5. Update check JSON: `{ "latest": "x.y.z", "url": "...", "notes": "..." }`
   fetched at most daily, comparing semver against
   `env!("CARGO_PKG_VERSION")`; no auto-download.
6. The vendored rawler (X2D II patches) is a fork liability at release
   time: before 1.0, upstream the three patches to dnglab (PR text is
   pre-written in the commit message of 5d1bd5d) and drop `vendor/` if
   accepted.

## 7. Testing Strategy Summary

| Layer | Method |
|---|---|
| Color math | Unit tests with published reference vectors (BT.709/2020/2100, Oklab) |
| RAW decode | Sample-file harness (env-pointed folder) + fuzzing |
| Pipeline | Golden-image tests, ΔE < 1.0 tolerance, per-phase references |
| Shaders | CI compile-check of every WGSL file on a headless adapter |
| Performance | criterion benches + budget assertions in Phase 8 |
| Export | Re-decode round-trips (dimensions, ICC/cICP, bit-depth fidelity) |
| UI | Manual test scripts per phase in `docs/`; egui state unit tests where practical |

## 8. Risk Register

0. *(retired)* **rawler Hasselblad support unverified** → now verified against the
   dnglab/rawler supported-camera list: X2D 100C, X1D II 50C, H6D-100cMS, H5D-50c/40,
   H4D-50/40, H3D, CFV-50, CFV 100C (907X) are all listed as supported. The Phase 1
   harness remains mandatory (list ≠ our files), but risk is downgraded.
1. **rawler Hasselblad coverage gaps** → Phase 1 verification harness runs first; libraw
   FFI fallback is pre-planned behind a feature flag with an unchanged public API.
2. **wgpu HDR surface support varies** → HDR display is capability-gated per platform
   via `SurfaceCapabilities::format_capabilities`; SDR fallback mapping is defined,
   Linux explicitly SDR in v1. Verified: wgpu ships `SurfaceColorSpace` +
   `Surface::display_hdr_info` (macOS Metal EDR path confirmed) as of v30.
3. **egui breaks on HDR surfaces** (egui #7484: black screen on `Rgba16Float` surface;
   #3168: gamma-space conversion quirks) → designed around from the start: egui always
   renders to an SDR offscreen target; our own composite pass owns the surface
   (Phase 6 step 1). This must be respected even in Phases 0–5 groundwork (the
   viewport is already a custom pass, so the refactor cost later is one composite
   shader, not a rework).
4. **egui look ceiling** → mitigated by the strict token/theme system in §5; if a widget
   can't be styled acceptably, custom-paint it rather than switching frameworks.
5. **HNCS expectations** → Hasselblad's color pipeline is proprietary and unpublishable
   (§3.4); exact replication is impossible in any third-party app. Mitigated by DCP
   support + the Reference Match fitter (Phase 3B) with measured ΔE reporting, and by
   honest UI naming ("Reference Match", never proprietary-equivalence language).
4. **100 MP memory pressure on 4 GB GPUs** → tiling + proxy rules in §3.5 are mandatory
   from Phase 2, not an afterthought; Phase 8 verifies under a capped adapter.
5. *(retired)* **JXL/AVIF crate maturity** → AVIF remains the broad-compatibility HDR target; the default JXL path uses permissively licensed libjxl through `gamut-jxl` and has 16-bit/ICC/PQ round-trip coverage.

## 9. Open Questions for the Product Owner (answer before Phase 6/7)

- Which Hasselblad bodies must be in the Phase 1 sample set beyond X2D 100C / X1D II?
- Is Linux HDR display worth chasing in a v1.x (Wayland color-management protocol), or is
  HDR *export* on Linux sufficient?
- *(answered by building it, 2026-07-26)* **Ultra HDR JPEG (gain maps)** — implemented
  rather than debated: AVIF is not what phones and browsers display, and an Ultra HDR
  file degrades to an ordinary JPEG everywhere else, so it costs nothing to offer.
  See `docs/interop-xmp-ultrahdr.md`.
- *(answered by building it, 2026-07-26)* **XMP for ratings/flags** — implemented,
  write opt-in and read always-on. `.spedit` remains the source of truth; XMP carries
  only rating and flag. See `docs/interop-xmp-ultrahdr.md`.

---

## 10. Platform Notes — macOS / Apple Silicon Optimizations

The primary development and target machine is assumed to be Apple Silicon. These notes
bind the generic architecture (§3) to Metal-specific realities; apply them in Phase 2
and verify in Phase 8.

> **Status (2026-07-26): items 1–3 are implemented; see
> `docs/platform-apple-silicon.md` for what was built and where it deviates.**
> Item 4 (EDR polling/smoothing) shipped with the Phase 6 EDR work. Items 5–6
> hold as written.

1. **Unified memory — avoid double-copies on upload.** On Apple Silicon, CPU and GPU
   share memory. The 100 MP CFA upload (~200 MB u16) must not go through an extra
   staging hop: use a persistently mapped staging buffer ring (`MAP_WRITE | COPY_SRC`)
   and write decoded rows directly into it from the decode thread, then a single
   `copy_buffer_to_texture`. Never `queue.write_texture` a 200 MB slice from a `Vec`
   that was itself copied out of the decoder.
2. **Native f16 arithmetic.** Enable `wgpu::Features::SHADER_F16` when the adapter
   offers it (all Apple GPUs do) and use `enable f16;` in WGSL for the tone/color/NR
   inner loops — Apple GPUs run f16 at double rate and it halves register pressure,
   which directly raises occupancy for the NLM noise pass. Keep an f32 fallback path
   compiled for adapters without the feature (checked in CI via naga validation of
   both variants).
3. **TBDR-friendly compute.** Apple GPUs are tile-based: prefer 16×16 workgroups,
   keep NLM/bilateral tiles + aprons in `var<workgroup>` shared memory, and fuse
   passes (WB+matrix+tone in one shader when NR is off — Phase 8) to avoid
   memory-bandwidth-bound round-trips, which are the dominant cost on M-series.
4. **EDR specifics.** With `ExtendedSrgbLinear`, 1.0 = SDR white ("reference white"),
   and the usable headroom is dynamic — it changes with display brightness/ambient.
   Poll `Surface::display_hdr_info` per frame-ish (it's cheap) and smooth the headroom
   value (~300 ms lerp) before feeding the tone mapper, or highlights visibly pump
   when macOS re-maps EDR. UI chrome stays ≤ 1.0 always (§Phase 6).
5. **ProMotion / power.** egui repaints on demand; ensure slider drags request repaint
   of the viewport only and idle app requests no frames — this is the difference
   between 2 W and 15 W on a MacBook and is a Phase 8 acceptance item.
6. **Do not hand-roll `CAMetalLayer`.** Everything needed (EDR surface, headroom,
   display info) is exposed through wgpu's surface API as of v30 — platform-specific
   `objc` code is banned from the codebase unless a capability is provably missing
   from wgpu, and then only behind a `#[cfg(target_os = "macos")]` shim in `sp-gpu`
   with a tracking issue to remove it.

## 12. Implementation Learnings Ledger (binding on later phases)

Discovered while building Phases 0–4; later phases MUST honor these.

**Color doctrine**
- WB'd camera values must go through the WHITE-PRESERVING matrix
  (`sp_core::profile::display_cam_to_xyz`): inverse ColorMatrix columns ×
  camera-space D65 white, or Bradford D50→D65 for ForwardMatrix profiles.
  Applying a raw inverse ColorMatrix after WB double-corrects the
  illuminant (global magenta cast). Never bypass this helper.
- Hasselblad orientation lives ONLY in EXIF tag 274; decoders report
  Normal. Orientation is BAKED into the CFA at decode
  (`sp-raw::convert::apply_orientation`) — everything downstream assumes
  upright pixels. New ingestion paths (DNG variants, future cameras) must
  route through the same function.
- Blown-highlight policy is currently an SDR clamp in `wb_color.wgsl`;
  Phase 6 replaces it with reconstruction (see Phase 6 refinement 1).
- The Standard profile applies `tone::standard_base_curve()`; any CPU
  reference (tests, fitters, exporters) must mirror it through
  `build_tone_lut` — never reimplement tone math.

**egui 0.35 / eframe (differs from most training data)**
- `App::ui(&mut self, ui, …)` + optional `App::logic(ctx, …)`;
  `egui::Panel::top/bottom/left/right(id).exact_size(px).show(ui, …)`;
  `CentralPanel` last; `ctx.all_styles_mut`; `ctx.egui_wants_keyboard_input`.
- **`ctx.set_theme(ThemePreference::Dark)` is mandatory** — without it the
  OS light mode silently discards the entire theme (root cause of the
  "stale UI" report).
- `register_native_texture` requires `Rgba8Unorm` (encode sRGB in-shader);
  float/HDR content needs paint callbacks instead.
- Missing-glyph trap: egui's default fonts lack many fullwidth/decorative
  glyphs (e.g. "＋" renders as a box). Stick to ASCII + common symbols or
  bundle a font.

**wgpu 29**
- `PipelineLayoutDescriptor { bind_group_layouts: &[Some(&l)],
  immediate_size: 0 }`; `push_error_scope` returns a guard with `.pop()`;
  `device.poll(PollType::wait_indefinitely())`; `TexelCopy*` naming.
- Default device limit `max_texture_dimension_2d = 8192` is TOO SMALL for
  100 MP sensors (11904 px) — both device-creation sites request the
  adapter limit; keep any new device path in sync.
- `Queue::write_texture` has no 256-byte row alignment requirement;
  buffer copies do (`div_ceil(256) * 256` in every readback).

**GPU device and shader invariants (added 2026-07-26)**
- Device features go through `GpuContext::preferred_features` at BOTH
  creation sites. Requesting `SHADER_F16` at one and not the other silently
  compiles a different shader variant in tests than in the app.
- A shader opts into the f16/f32 precision prelude with `//#SP_PRECISION`
  before its first declaration; `shaders_compile.rs` validates both variants
  through naga with no device. Never hand-write `enable f16;` — the fallback
  would stop being compiled.
- In any workgroup-tiled shader, the out-of-bounds early return must come
  AFTER `workgroupBarrier()`. Every thread fills its share of the tile, then
  the barrier, then the bounds check. Reversing them is undefined behavior
  that usually looks fine.
- Large CPU→GPU uploads go through `sp_gpu::StagingRing`, never
  `queue.write_texture` (§10.1). Buffer→texture copies need 256-byte row
  alignment; the ring handles the padding.

**Decoder robustness (added 2026-07-26)**
- Every decode runs `sp_raw::preflight::check` first. Fuzzing found that a
  truncated or mutated TIFF makes rawler allocate from unvalidated header
  fields — hundreds of megabytes, or gigabytes, before touching pixel data.
  New ingestion paths must route through the same check.
- The invariant to extend, if more classes turn up: nothing declared inside
  a file may be larger than the file. Do not add heuristics that a real
  camera file could trip.

**Process / harness**
- Dev-profile opt-levels: workspace `"*"` override covers DEPENDENCIES
  only; hot member crates are listed explicitly. A 105 MP loop at opt 0
  is 10×+ slower — check before profiling conclusions.
- QA loops that exist and must stay green: `cargo test --workspace`
  (GPU tests skip cleanly headless), `SP_TEST_RAWS` harnesses,
  `SP_DUMP_DIR` render dumps, `SP_SCREENSHOT` UI self-capture, and the
  `verify-everything` command in PROGRESS.md.
- Vendored rawler patches (X2D II alias, .fff model alias, Compression-tag
  decode) live in `vendor/rawler` via `[patch.crates-io]` — refresh
  carefully, upstream before 1.0.
- The tile renderer is shared by viewport 1:1 AND the noise loupe with
  cross-invalidated cache keys — any third consumer must join that
  invalidation scheme or own its resources.

## 11. Sources & Research Notes (verified 2026-07)

HDR / wgpu:
- wgpu `SurfaceColorSpace` docs (v30): variants incl. `ExtendedSrgbLinear` ("corresponds
  to Metal's EDR, typically used with Rgba16Float"), `Bt2100Pq`, `DisplayP3`;
  `SurfaceConfiguration.color_space`, `SurfaceCapabilities.format_capabilities`,
  `Surface::display_hdr_info` (populated on Metal) —
  https://docs.rs/wgpu/latest/wgpu/enum.SurfaceColorSpace.html
- HDR surface tracking issue: https://github.com/gfx-rs/wgpu/issues/2920
- Apple, "Explore HDR rendering with EDR" (WWDC21) —
  https://developer.apple.com/videos/play/wwdc2021/10161/
- egui on float/HDR surfaces breakage: https://github.com/emilk/egui/issues/7484 ;
  gamma-space conversion details: https://github.com/emilk/egui/issues/3168

Hasselblad color science:
- Hasselblad, "Natural Colour Solution" —
  https://www.hasselblad.com/learn/hasselblad-natural-colour-solution/
- "How HNCS Actually Works" (illuminant matrices, chroma LUTs, Film Curve, render-time
  pipeline, nothing exportable, 3FR carries no HNCS data) —
  https://blog.tonalphoto.com/hasselblad-hncs-color-science-explained/
- HNCS HDR behavior in Phocus — https://blog.tonalphoto.com/hasselblad-hncs-hdr-explained/

RAW decoding:
- rawler/dnglab supported cameras (Hasselblad X2D 100C, X1D II 50C, H6D-100cMS,
  H5D-50c/40, H4D-50/40, H3D, CFV-50, CFV 100C/907X listed supported) —
  https://github.com/dnglab/dnglab/blob/main/SUPPORTED_CAMERAS.md
