# Photographic corrective editing

Film negative conversion runs on linear source channels before white balance and camera-matrix development. It divides transmission by a per-channel clear-film base, converts to optical density with `-log10(transmission/base)`, subtracts black density, and maps the chosen density range/gamma to a positive image. The clear-film-border picker averages a 7×7 patch of the actual shading-corrected proxy source before inversion. Highlight reconstruction is bypassed during inversion because clipped transparent negative regions do not represent positive scene highlights. Film controls are off in old sidecars, included in Basic reset/bypass, and copied only with the image-specific white-balance preset group. This is a controllable density conversion, not a calibrated emulation of a particular film stock.

Each local adjustment now has red-eye and color-moiré controls. Red-eye attenuates red-dominant reflections while retaining neutral catchlights and pixels outside the selection. Moiré averages chroma with a luminance-guided filter, applies only the chroma residual, and keeps fine luminance detail. Radius is specified in source pixels; tile padding includes its neighborhood. This treats false-color moiré; it cannot reconstruct detail lost to luminance aliasing.

Global and local Color controls add a 3×3 working-RGB channel mixer, relative/absolute selective CMYK adjustments for six hue ranges and whites/neutrals/blacks, and targeted skin hue/chroma uniformity. Skin controls operate on a configurable Oklab hue range, preserve lightness, and can be restricted with an ordinary local mask. They do not identify people or lighten complexions automatically. Selective controls use smooth hue/saturation/tonal weights; their parameterization is independent of proprietary editors.

The global and local GPU paths compile the same `photographic_color.wgsl` helper. Neutral controls bypass color conversion exactly. `sp_core::correction` provides CPU reference implementations and bounds checks. Existing sidecars load neutral defaults; local presets include these controls.

Validation: CPU film-density/neutral/backward-compatibility and photographic-color reference tests; real-GPU film density and source-picker checks; preview/export agreement; red-eye catchlight and unselected-pixel preservation; moiré chroma suppression with luminance-detail retention; global/local GPU parity against CPU references for channel mixing, selective color and skin uniformity; shader validation.

## Sample points and scopes

Color-panel sample points persist in sidecars and can be named, moved using numbered viewport pins, or removed. Their positions use the same canonical lens/geometry mapping as masks. Readouts show scene-linear Rec.2020 RGB, CIE Lab under D65, and exposure relative to 18% gray; values above diffuse white are retained. They measure areas in the developed proxy rather than claiming sensor-pixel precision.

Optional whole-image luminance waveform, RGB parade and Lab a*/b* vectorscope use a dedicated GPU analysis image capped at 512 pixels on its long edge. The waveform/parade ceiling can be raised four stops above diffuse white. Analysis is taken after local edits and sharpening, before display/proof encoding and lens geometry. GPU generation is optional, CPU readback waits until pointer interaction ends, and changing only sample positions avoids redeveloping the image. Scope textures are cached between changes.

Tests cover neutral/gray/HDR numeric references, sample-point old-sidecar compatibility and roundtrip, complete scope-bin accounting and neutral vectorscope position, and bounded GPU analysis with retained HDR values and identical displayed output.

## Texture healing, donor suggestions, and dust review

Heal corrections now have **Match texture** (0–100). Zero preserves existing
sidecars' ring-mean healing. At 100, a bounded Gaussian low-frequency source estimate is
removed and the corresponding target estimate is restored: donor detail follows
the target's local illumination. It is frequency separation, not a neural
content-aware fill. Source and target sampling use the immutable pre-retouch
scene. Independent donor patches include the filter halo; distant sources never
make the target tile span the distance between source and target.

**Find matching source** searches a bounded 64×64 candidate grid using 16 samples
around the correction, compares centered texture and mean color, excludes the
defect and checks the translated stroke fits. The editable source handle remains
visible for review. **Find possible dust** analyzes a scene-linear proxy (at most
1024 pixels on its long edge), looks for compact dark minima with smooth, bright
surroundings at four scales, and proposes at most 100 nonoverlapping candidates.
Each proposal must be accepted with **Heal** or dismissed; accepting creates an
ordinary undoable, serializable correction with an editable suggested donor.
This detector cannot identify sensor dust conclusively from one image or find
all fine defects after downsampling. Manual 100% inspection remains useful.

Regression tests cover donor exclusion/background matching, Gaussian dust on a
gradient with rejection of a straight edge, source-texture transfer while keeping
target illumination, restoring the displayed edit after pre-retouch analysis,
and distant GPU source patches compared to an independent full-scene render.

## Conservative blur restoration

Detail adds **Restore blur**, a 1–2 pixel blur radius and a noise guard.
It applies a damped two-term inverse of a separable binomial blur kernel:
`y + a(I-K)y + a²(I-K)²y`. A noise gate and relative correction bound limit
amplification; chroma follows the restored luminance ratio. This is intended for
small Gaussian-like softness, not strong motion blur or recovery of missing
detail. Inspect the result at 100% before increasing strength.

Restoration is neutral in old sidecars, independent of ordinary sharpening, and
included in Detail reset, presets, selective copying, preview, tile and export
paths. Tile padding includes the finite filter support. CPU tests demonstrate
reduced error on a known blurred signal and preservation of flat HDR/noise;
real-GPU tests compare the CPU reference, retain values above diffuse white and
verify preview/export agreement.
