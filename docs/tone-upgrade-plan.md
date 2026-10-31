# Tone & Brightness Tools — Upgrade Plan

Status: **implemented** (2026-07-22). Completion notes and measured deviations
are in `docs/phase-tone-a.md` through `docs/phase-tone-g.md`.

## Motivation

The Basic panel currently has one global brightness control (Exposure, linear
gain) plus contrast/highlights/shadows/whites/blacks and the point curve.
That is enough to set overall level, but it lacks:

- a **midtone brightness** control that moves the middle without shifting
  black/white points (Exposure moves everything, including HDR headroom);
- **local/spatial** brightness tools (Clarity, Texture, Dehaze) — the biggest
  perceived-quality gap vs Lightroom/Phocus;
- **automatic** starting exposure ("Auto" button);
- **per-channel curves** and parametric curve regions;
- **radial falloff** exposure (vignette);
- clipping feedback while dragging.

Everything below preserves the standing invariants:

1. `EditState::default()` renders the untouched image (neutral-edit tests).
2. New sidecar fields use `#[serde(default)]` so schema version 1 stays
   forward-readable — no `EDIT_SCHEMA_VERSION` bump is needed for additive
   fields (bump only if a field's meaning changes).
3. LUT-only controls stay LUT-only: no new GPU passes unless the tool is
   genuinely spatial. Neutral params must cull the pass (existing pattern).
4. Every shader change has a CPU mirror + parity test, and monotonicity of
   the tone LUT stays enforced.

Recommended order: A → B → C → D → E, with F and G (color: proof spaces and
look profiles) schedulable independently — F only touches the output
transform, G only the profile stage, so either can run in parallel with the
tone phases. A and E are small; C is the big one.

---

## Phase A — Midtone Brightness + Auto Tone (small)

The quickest win: a true "brightness" slider and an Auto button.

### A1. `brightness` field

- `sp-core/src/edit.rs`: add `pub brightness: f32` to `ToneParams`
  (`-100.0..=100.0`, 0 neutral, `#[serde(default)]`), clamp in `clamped()`.
- `sp-pipeline/src/tone.rs::tone_map`: implement as a **gamma move in the
  encoded (sRGB-OETF) domain**, applied after contrast, before the
  region masks:

  ```text
  e  = srgb_oetf(y.min(1.0))
  g  = 2^(-brightness/100 * 0.75)        // ±100 → gamma 0.59..1.68
  y' = srgb_eotf(e^g) + (y - y.min(1.0)) // preserve >1.0 headroom offset
  ```

  Gamma in encoded space pins 0 and 1 exactly: black point, white point and
  HDR headroom are untouched — precisely what Exposure cannot do.
- No shader change (it's inside the LUT). No new pass.

### A2. Auto tone button

- Reuse the existing histogram pass (`histogram.wgsl`) statistics; add a
  small `sp-pipeline/src/auto_tone.rs`:
  `pub fn auto_tone(hist: &HistogramStats) -> ToneParams`.
- Algorithm (deterministic, testable): target median luminance at encoded
  0.42, 0.1% percentile at encoded 0.02, 99.9% at 0.97. Solve exposure from
  the median, then whites/blacks from the percentiles, cap highlights
  recovery at -60. Pure function of the histogram → unit-test with synthetic
  histograms (low-key, high-key, flat).
- UI (`sp-app/src/panels.rs`): "Auto" button in the Basic header; writes the
  result through the normal edit path so undo captures it as one step.

### Tests / acceptance

- `tone.rs`: brightness 0 = identity; +100 lifts encoded 0.5 above 0.62
  while `apply_tone_lut` at 0.0 and 1.0 stays within 1e-3; LUT stays
  monotonic at extremes combined with all other sliders.
- `auto_tone`: synthetic histograms land median within ±0.03 of target.
- Manual: X2D II test RAWs in `~/Documents/shopphoto-test-raws` — Auto gives
  a usable starting point on the dark interiors set.

---

## Phase B — Curve upgrades: parametric regions + RGB channels (medium)

### B1. Parametric curve sliders

Lightroom's "Highlights / Lights / Darks / Shadows" region sliders on the
curve panel. These are *different* masks from the Basic panel's
highlights/shadows (quarter-tone regions, gentler falloff).

- `sp-core`: new `pub struct ParametricCurve { highlights, lights, darks,
  shadows: f32 }` (all -100..=100, neutral 0, `Default` + clamp), added to
  `EditState.curve` — restructure `ToneCurve` into
  `pub struct CurveParams { pub parametric: ParametricCurve, pub luma: ToneCurve, pub red: ToneCurve, pub green: ToneCurve, pub blue: ToneCurve }`
  with a custom `Deserialize` that accepts the old bare-`points` shape
  (sidecar compat test required — load a v1 sidecar fixture).
- `tone.rs`: evaluate parametric deltas in the encoded domain with four
  raised-cosine region weights centered at e = 0.85 / 0.62 / 0.38 / 0.15
  (half-width 0.35), max shift ±0.12 encoded per slider, folded into the
  same LUT. Monotonicity enforcement already guards extremes.

### B2. Per-channel point curves

- The single tone LUT becomes 4 curves compressed into the LUT stage:
  luma curve stays in the combined 1-D LUT; R/G/B curves become a second
  small LUT (3×256, storage buffer) applied per channel in
  `tone_color.wgsl` **after** the main LUT, in the encoded domain,
  preserving >1.0 headroom by endpoint-gain (same pattern as the user
  curve today). Skip entirely (uniform flag) when all three are neutral.
- UI: channel selector (Y/R/G/B) on the existing curve editor; colored
  curve stroke per channel.

### Tests / acceptance

- Old sidecars load unchanged (fixture test).
- Neutral CurveParams ⇒ bit-identical render (extend the golden test).
- Red curve pulled down affects only R channel on a gray ramp fixture.

---

## Phase C — Local contrast: Clarity, Texture, Dehaze (large)

The genuinely new machinery: these need spatial context, so this is a new
GPU pass, not a LUT.

### C1. New pass

- `PassId::LocalContrast` → `local_contrast.wgsl`, scheduled **after
  NoiseReduction, before Sharpen** (operate on denoised data; sharpening
  last keeps halos independent). Culled when all three params are 0.
- Params: `pub struct LocalContrastParams { clarity: f32, texture: f32, dehaze: f32 }`
  (-100..=100 each, neutral 0), new `EditState.local` field.
- Implementation on Oklab L only (reuse the matrix constants — factor them
  into a WGSL include or shared snippet if duplication bothers; today
  shaders are standalone files, duplicating 4 matrices is acceptable and
  should be noted in the shader header, with a parity test against
  `tone_color.wgsl` constants).
  - **Texture**: unsharp on a small Gaussian (σ≈2 px at working res),
    ±100 → ±0.6 gain, with a gradient mask that excludes strong edges
    (the inverse of sharpen's detail mask — affects micro-texture only).
  - **Clarity**: unsharp on a large blur (σ≈ working-width/40) with a
    midtone weight `w = 1 - |2e-1|^1.5`; soft-clip the boost so it cannot
    push L past [0, 1.1].
  - **Dehaze**: estimate airlight as the 99.9% percentile of min-RGB from
    the histogram stats (CPU, per frame), then apply transmission-style
    contrast: `y = (y - a*t)/(1-t)` with t from the slider (±100 →
    ∓0.35), plus a mild saturation compensation forwarded to the tone
    pass uniforms. Negative = add haze.
- Blur source: two-level downsample chain reusing `downsample.wgsl` into a
  ¼-res mip, separable Gaussian there, bilinear upsample — keeps the large
  blur cheap at 100 MP. Textures come from the existing pool; sizes keyed
  by working resolution (respect the tile machinery — the pass must work
  per-tile with an apron; apron radius = blur radius, follow the NR pass's
  apron pattern).

### C2. Quality gates

- No visible halos at Clarity +50 on the backlit test RAW (visual check +
  a numeric guard: max overshoot at a step edge fixture < 5% of step).
- Tile seams: render tiled vs untiled on the synthetic gradient fixture,
  max abs diff < 1e-3.
- Perf: full chain on the 100 MP X2D II file stays < 80 ms GPU (current
  51 ms; budget +25 ms for the blur chain).

---

## Phase D — Vignette + HDR brightness refinements (small-medium)

### D1. Post-crop vignette

- `pub struct VignetteParams { amount: f32, midpoint: f32, roundness: f32, feather: f32 }`
  (amount -100..=100 neutral 0; others 0..=100 with LR-style defaults 50).
- Applied in **scene-linear** space as radial EV gain (±1.5 EV at the
  corners), computed in the output transform pass (it already knows the
  crop geometry) — no new pass. Highlights-priority blend: attenuate the
  gain where y > 1 so HDR speculars don't gray out.

### D2. HDR brightness

Ties into "HDR works but needs more brightness control":

- Add `pub scene_white_ev: f32` to `HdrParams` (-2.0..=2.0, 0 neutral):
  shifts what maps to SDR reference white in the output transform,
  independent of Exposure (which also moves midtones). This is the "how
  bright is paper-white vs the HDR range" control that HDR grading needs.
- Surface the current EDR headroom next to the slider (already available
  from the `--hdr-check` work) so the user sees how many stops the display
  can actually show.

---

## Phase E — Feedback & UX polish (small)

- **Clipping indicators**: J/O-style toggles + hold-Alt-while-dragging on
  Exposure/Whites/Blacks shows the threshold view (new debug mode in
  `output_transform.wgsl` behind a uniform flag: red = any channel ≥ white
  target, blue = ≤ 0.001). HDR-aware: white target = SDR white or current
  headroom depending on `HdrParams`.
- Histogram becomes interactive: drag regions of the histogram to drive
  blacks/shadows/exposure/highlights/whites (map histogram x-region →
  slider, same encoded regions as Phase B parametric).
- Double-click-with-Shift on Exposure = auto-exposure only (subset of
  Phase A's auto).

---

## Phase F — Preview color space & soft proofing (medium)

Today the SDR preview is always sRGB: `output_transform.wgsl` converts
working Rec.2020 → Rec.709 primaries and applies the sRGB OETF,
unconditionally. Goal: let the user choose what the preview simulates.

### F1. Proof space selection

- `sp-core`: new `pub enum ProofSpace { #[default] DisplayNative, Srgb, DisplayP3, Rec2020 }`.
  This is **app/view state, not edit state** — it must NOT live in
  `EditState` (soft proofing doesn't change the edit or the sidecar; two
  machines viewing one sidecar may proof differently). Put it in
  `sp-app::state` next to the zoom/HDR-view settings, persisted in app
  prefs, surfaced as a dropdown in the viewport toolbar plus a badge when
  not DisplayNative.
- `output_transform.wgsl`: replace the hard-coded `REC2020_TO_REC709` with
  a `gamut_mat: mat3x3` uniform + an `oetf_mode` flag supplied per frame:
  - `Srgb`: 2020→709 matrix, sRGB OETF (current behavior; stays default
    when the display is untagged/unknown).
  - `DisplayP3`: 2020→P3-D65 matrix, sRGB OETF (P3 uses the sRGB transfer).
  - `Rec2020`: identity matrix, sRGB OETF for preview purposes (a true
    BT.1886 preview on an SDR panel adds little; document the choice).
  - `DisplayNative` (macOS): tag the CAMetalLayer with the matching
    colorspace and send pixels in the display's space — the existing EDR
    path (`macos_edr.rs`) already does colorspace tagging for HDR; reuse
    that plumbing for SDR.
- Matrices come from `sp_core::color` (add `REC2020_TO_P3D65` with a
  round-trip unit test against the published primaries, same style as the
  existing constants — and mirror the "must match" comment convention in
  the shader).
- **Gamut clip indicator** (cheap, high value): a proof-mode uniform flag
  that paints pixels falling outside the proof gamut (any channel < 0
  after the matrix, pre-clamp) in a marker color. This is what makes
  sRGB-proofing genuinely useful before an sRGB export.

### F2. Export parity

- The export encoders (Phase 5) already write ICC/cICP-tagged sRGB output;
  extend the export dialog's color-space choice to match the proof list
  (sRGB, Display P3, Rec.2020) so "what you proofed is what you export".
  One shared enum → both UIs; conversion happens in the existing
  `output_transform_16.wgsl` path with the same uniform matrix.

### Tests / acceptance

- Round-trip: 2020→P3→2020 identity within 1e-5 (CPU).
- Golden test per proof space on the synthetic gradient fixture (three
  small goldens, not per-camera).
- Saturated blue/green test patch shows gamut-clip markers in sRGB proof
  but not in Rec.2020 proof.
- Manual: preview on the MacBook (P3 panel) — sRGB proof visibly desaturates
  saturated reds vs DisplayNative; exported sRGB JPEG matches the sRGB
  proof side-by-side.

---

## Phase G — Built-in look profiles ("Landscape" etc.) (medium)

Lightroom's Adobe Landscape/Portrait/Vibrant are **look profiles**: a
color-only transform + base curve applied at the profile stage, before any
user sliders, so all sliders still start at 0. Our Tier-2 DCP machinery
already supports exactly this (`HueSatLut` look tables + base `ToneCurve`
in `CameraProfile`) — Phase G is mostly authoring + selection UX, not new
pipeline machinery.

### G1. Look definition

- `sp-core`: `pub struct Look { pub name: String, pub huesat: Option<HueSatLut>, pub base_curve: Option<ToneCurve>, pub base_saturation: f32 }`.
  Built-in looks compile in as constructors (`Look::landscape()` etc.) —
  no asset files to lose; user-imported looks can come later via the
  existing profile store.
- `EditState`: add `pub look: Option<String>` (`#[serde(default)]`,
  `None` = no look — neutral invariant holds). Resolved by name like
  `ProfileChoice::Named`; unknown name ⇒ fall back to none + warning badge
  (same pattern, same reason: sidecars must never error).
- Pipeline: a look's huesat table rides the existing
  `ProfileColorPost`/`huesat_lut.wgsl` pass (compose look table with the
  profile's own look table if both exist — compose on the CPU at resolve
  time, keeping one GPU pass); its base curve **replaces**
  `standard_base_curve()` in `build_tone_lut`. Look saturation folds into
  the tone pass's existing global-saturation uniform (multiplied, not
  added, with the user slider).

### G2. The initial set (author these, tune on the test RAWs)

- **Standard** — current behavior (no-op look; the `None` state, listed
  first so the UI reads like Lightroom's profile picker).
- **Landscape** — +sat on greens/aquas/blues (band-targeted huesat table,
  ~+15%), slight hue rotation of yellow-greens toward green, punchier
  base curve (deeper quarter-tones, protected skies via reduced lift in
  high-luminance blue entries).
- **Portrait** — gentler base curve in the orange band's luminance range,
  −8% saturation on oranges, overall contrast slightly reduced.
- **Vibrant** — global +20% sat table with muted-color weighting, stronger
  S-curve.
- **Muted / Flat** — reduced sat, lifted blacks (useful starting point for
  matte looks).
- Author each as a small Rust constructor generating the `HueSatLut`
  programmatically (band-weight math already exists conceptually in
  `tone_color.wgsl`'s raised-cosine bands — reimplement the weighting in
  Rust, don't parse handmade tables).

### G3. UI

- Profile picker at the top of the Basic panel becomes two rows:
  camera profile (existing `ProfileChoice`) and Look (new dropdown with
  hover-to-preview: on hover, render with the hovered look using the
  normal invalidation path — cheap since it's LUT/uniform-only).
- Looks are edits: selecting one is an undo step and writes the sidecar.

### Tests / acceptance

- `look: None` and `Look::standard()` render bit-identical (golden).
- Sidecar with an unknown look name loads, renders as Standard-equivalent,
  shows the badge (mirror the existing missing-profile test).
- Landscape on a green/blue synthetic patch raises measured chroma in
  Oklab by 10–20% while a skin-tone patch moves < 3%.
- Each look composes with a Tier-2/3 camera profile without double-applying
  base curves (explicit test: profile-with-curve + look-with-curve ⇒ look
  curve wins; document the precedence in the `Look` doc comment).

---

## Cross-cutting code-quality checklist (every phase)

1. **Neutral invariant**: extend `default_is_neutral_and_stable_under_clamp`
   and the golden neutral-render test for every new field.
2. **Clamp**: every field added to `EditState::clamped` with its documented
   range in the doc comment; UI never relies on clamping.
3. **Sidecar compat**: `#[serde(default)]` on all new fields; a fixture test
   that deserializes a pre-upgrade sidecar JSON string; no schema bump for
   additive changes.
4. **Pass culling**: neutral spatial params ⇒ pass not planned (assert in
   `plan_passes` tests), so existing perf numbers hold for old edits.
5. **CPU/GPU parity**: any math in a shader exists as a tested CPU mirror
   (`tone.rs` pattern); constants shared between .wgsl and .rs carry a
   "must match" comment naming the counterpart, plus a parity test where
   feasible.
6. **Docs**: each phase gets `docs/phase-tone-<letter>.md` on completion,
   deviations recorded in PROGRESS.md (standing rule).
7. **UI**: reuse `param_slider` (stable rows, double-click reset); group
   new sliders under existing collapsing headers (Basic gets Brightness
   under Exposure; Effects header is new for Clarity/Texture/Dehaze/
   Vignette).
8. **Perf budget**: after each phase, re-run the X2D II open+render timing;
   record in the phase doc. Any regression > 10% needs a note and a plan.
