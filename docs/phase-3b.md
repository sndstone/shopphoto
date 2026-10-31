# Phase 3B — Camera Color Profiles & Reference Match

Implementation completed 2026-07-20 (session 6).

## Delivered

- `CameraProfile` now carries inverse ColorMatrix values, preferred DNG
  ForwardMatrix values, dual-illuminant HueSat maps, a DNG look table, base
  tone curve, and a general scene-linear RGB 3-D LUT. `resolve(temp)`
  re-resolves the complete parameter set in inverse-CCT space on every WB
  change.
- `sp-raw::dcp` is a bounded standalone TIFF/EP reader for little- and
  big-endian DCP files. It parses ColorMatrix1/2, ForwardMatrix1/2,
  CalibrationIlluminant1/2, ProfileHueSatMapData1/2,
  ProfileLookTableData, ProfileToneCurve, ProfileName, and
  UniqueCameraModel. Malformed profiles return `SpError::CorruptData`.
- `huesat_lut.wgsl` is a dedicated profile pass using real `rgba16float`
  3-D textures and explicit trilinear interpolation. It runs twice: the
  WB-resolved HueSat map before tone, then the DNG look table and optional
  Reference Match 33³ RGB LUT after tone. Proxy and 1:1 tile paths use the
  identical chain.
- The Basic panel profile dropdown lists only profiles matching the open
  camera, imports/copies DCP files into the application profile directory,
  persists selection in `EditState`, and visibly marks missing profiles
  while rendering with Standard.
- The Reference Match modal implements the complete user workflow: neutral
  Phocus export checklist, 3–10 RAW multi-selection, TIFF-folder selection,
  case-insensitive stem pairing, background decode/sampling/fitting, and a
  mean/p95 CIEDE2000 report.
- TIFF ingestion preserves 16-bit data, applies orientation, honors an
  embedded ICC profile through `moxcms` into linear Rec.2020, rejects
  clipped and high-gradient locations, and draws up to about 50k spatial
  samples across the image set.
- The fitter reserves a deterministic 20% holdout, fits a least-squares
  3×3 matrix, a monotonic encoded-domain 1-D curve, and a smoothness-
  regularized 33³ residual LUT. Fitted profiles are saved as versioned,
  inspectable `.spprofile` JSON plus a sibling binary `.splut` blob.

## Verification

- Strict format + clippy and all workspace tests pass.
- GPU golden coverage proves a synthetic DCP HueSat map measurably changes
  output; core tests cover inverse-CCT matrix/HueSat interpolation and
  ForwardMatrix preference.
- Synthetic matrix/tone/look recovery achieves mean holdout ΔE00 below
  0.5. The CIEDE2000 implementation is checked against the Sharma reference
  pair (2.0425).
- A generated 16-bit TIFF exercises the real decoder/ICC conversion/sample
  path, and profile JSON/binary-LUT persistence round-trips.
- Both local Hasselblad files still decode and render: X2D II 100C 105 MP
  decoded in 74 ms and GPU-opened/rendered in 49 ms; X1D II 50C decoded in
  311 ms and GPU-opened/rendered in 25 ms on this machine.

## Validation note

No neutral 16-bit TIFF exports from Phocus are present in the local fixture
folder, so the final real-world `< 2.0` holdout threshold cannot be asserted
in CI. The wizard always reports the measured value, saves a result with a
warning when it misses the target, and clearly reports when a TIFF lacks an
embedded ICC profile. Running the wizard on the user's actual exports is
the remaining calibration-data validation, not missing implementation.

Likewise, this machine has no installed Hasselblad DCP and the repository
does not redistribute Adobe profiles. The parser test builds a complete
reference-valued DCP fixture covering every consumed tag; importing one of
the user's Adobe/third-party Hasselblad DCPs is the remaining real-profile
compatibility run.

## Deliberate implementation detail

DNG HueSat/look HSV operations run over scene-linear Rec.2020 rather than a
temporary ProPhoto working space. This avoids another wide-gamut round trip;
the transform ordering, circular hue interpolation, and value/saturation
axes follow the DCP table semantics. Real-image comparison should decide
whether a ProPhoto detour is worth adding later.
