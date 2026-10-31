//! Non-destructive edit parameters.
//!
//! [`EditState`] is the complete description of an edit. It is:
//! - stored in `.spedit` sidecar JSON (versioned, forward-readable),
//! - the unit of undo/redo (snapshots are tiny),
//! - the sole input, besides the RAW data and camera profile, that determines
//!   a render. Same `EditState` + same file ⇒ same pixels, always.
//!
//! Invariant: `EditState::default()` is the **neutral edit** — rendering with
//! it must produce the untouched image. Tests enforce this for every field.
//! All ranges are documented per field and clamped by `EditState::clamped`;
//! the UI must not rely on clamping for slider limits.

use serde::{Deserialize, Serialize};

/// Current sidecar schema version. Bump on breaking change and add a
/// migration in `sp-catalog::sidecar`.
pub const EDIT_SCHEMA_VERSION: u32 = 1;

/// White balance. Neutral means "as shot" (use `RawImage::wb_coeffs`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WhiteBalance {
    /// Correlated color temperature in kelvin, 2_000..=50_000.
    /// `None` = as shot.
    pub temp_kelvin: Option<f32>,
    /// Green–magenta tint, -150.0..=150.0, 0 neutral. Ignored when
    /// `temp_kelvin` is `None`.
    pub tint: f32,
}

impl Default for WhiteBalance {
    fn default() -> Self {
        Self {
            temp_kelvin: None,
            tint: 0.0,
        }
    }
}

/// Basic-panel tone controls. All -100.0..=100.0 with 0 neutral, except
/// exposure which is in EV stops.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ToneParams {
    /// Exposure in EV stops, -5.0..=5.0, 0 neutral.
    pub exposure_ev: f32,
    /// Midtone brightness, -100.0..=100.0, 0 neutral. Pins encoded black
    /// and white, unlike exposure.
    #[serde(default)]
    pub brightness: f32,
    pub contrast: f32,
    pub highlights: f32,
    pub shadows: f32,
    pub whites: f32,
    pub blacks: f32,
}

impl Default for ToneParams {
    fn default() -> Self {
        Self {
            exposure_ev: 0.0,
            brightness: 0.0,
            contrast: 0.0,
            highlights: 0.0,
            shadows: 0.0,
            whites: 0.0,
            blacks: 0.0,
        }
    }
}

/// Sensor-channel highlight reconstruction controls.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct HighlightRecoveryParams {
    pub enabled: bool,
    /// Blend from original sensor channels to reconstruction, 0..=100.
    pub strength: f32,
    /// Multi-scale neighbour search radius, 1..=32 working pixels.
    pub radius: u8,
}

impl Default for HighlightRecoveryParams {
    fn default() -> Self {
        Self {
            enabled: true,
            strength: 100.0,
            radius: 2,
        }
    }
}

/// A tone-curve control point in normalized coordinates (0..=1 both axes).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CurvePoint {
    pub x: f32,
    pub y: f32,
}

/// Catmull-Rom tone curve. Neutral = the two endpoints (identity).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToneCurve {
    /// Sorted by `x`; always includes endpoints at x=0 and x=1.
    pub points: Vec<CurvePoint>,
}

impl Default for ToneCurve {
    fn default() -> Self {
        Self {
            points: vec![CurvePoint { x: 0.0, y: 0.0 }, CurvePoint { x: 1.0, y: 1.0 }],
        }
    }
}

impl ToneCurve {
    /// True if this curve is the identity (neutral) curve.
    #[must_use]
    pub fn is_neutral(&self) -> bool {
        self.points.len() == 2
            && self.points[0] == CurvePoint { x: 0.0, y: 0.0 }
            && self.points[1] == CurvePoint { x: 1.0, y: 1.0 }
    }
}

/// Four broad, overlapping regions in the parametric tone curve.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct ParametricCurve {
    /// -100.0..=100.0, 0 neutral.
    pub highlights: f32,
    /// -100.0..=100.0, 0 neutral.
    pub lights: f32,
    /// -100.0..=100.0, 0 neutral.
    pub darks: f32,
    /// -100.0..=100.0, 0 neutral.
    pub shadows: f32,
}

/// Parametric and point curves. The luma curve is followed by optional
/// encoded-domain red, green, and blue curves.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct CurveParams {
    pub parametric: ParametricCurve,
    pub luma: ToneCurve,
    pub red: ToneCurve,
    pub green: ToneCurve,
    pub blue: ToneCurve,
}

impl CurveParams {
    #[must_use]
    pub fn channels_are_neutral(&self) -> bool {
        self.red.is_neutral() && self.green.is_neutral() && self.blue.is_neutral()
    }
}

// Accept both the pre-upgrade bare `{ "points": [...] }` shape and the
// expanded object. This keeps schema-v1 sidecars forward-readable.
impl<'de> Deserialize<'de> for CurveParams {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Compat {
            Old(ToneCurve),
            New {
                #[serde(default)]
                parametric: ParametricCurve,
                #[serde(default)]
                luma: ToneCurve,
                #[serde(default)]
                red: ToneCurve,
                #[serde(default)]
                green: ToneCurve,
                #[serde(default)]
                blue: ToneCurve,
            },
        }
        Ok(match Compat::deserialize(deserializer)? {
            Compat::New {
                parametric,
                luma,
                red,
                green,
                blue,
            } => Self {
                parametric,
                luma,
                red,
                green,
                blue,
            },
            Compat::Old(luma) => Self {
                luma,
                ..Self::default()
            },
        })
    }
}

/// Spatial contrast controls. Every value is -100.0..=100.0; all zero
/// culls the local-contrast pass.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct LocalContrastParams {
    pub clarity: f32,
    pub texture: f32,
    pub dehaze: f32,
}

impl LocalContrastParams {
    #[must_use]
    pub fn is_neutral(self) -> bool {
        self.clarity == 0.0 && self.texture == 0.0 && self.dehaze == 0.0
    }
}

/// Post-crop radial vignette.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct VignetteParams {
    /// Corner gain in -100.0..=100.0; 0 is off.
    pub amount: f32,
    /// Falloff midpoint, 0.0..=100.0.
    pub midpoint: f32,
    /// Shape from rectangular to circular, 0.0..=100.0.
    pub roundness: f32,
    /// Edge softness, 0.0..=100.0.
    pub feather: f32,
}

impl Default for VignetteParams {
    fn default() -> Self {
        Self {
            amount: 0.0,
            midpoint: 50.0,
            roundness: 50.0,
            feather: 50.0,
        }
    }
}

/// The 8 HSL hue bands, in hue order.
pub const HSL_BAND_COUNT: usize = 8;

/// Per-band HSL adjustment; each field -100.0..=100.0, 0 neutral.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct HslBand {
    pub hue_shift: f32,
    pub saturation: f32,
    pub luminance: f32,
}

/// One tonal color-grading wheel.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct ColorGradeWheel {
    /// Hue angle in degrees, 0.0..=360.0. Ignored when saturation is zero.
    pub hue_degrees: f32,
    /// Color amount, 0.0..=100.0.
    pub saturation: f32,
}

/// Three-way color grading in Oklab, with a tonal balance control.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct ColorGradingParams {
    pub shadows: ColorGradeWheel,
    pub midtones: ColorGradeWheel,
    pub highlights: ColorGradeWheel,
    /// Moves the split toward shadows (-100) or highlights (+100).
    pub balance: f32,
}

impl ColorGradingParams {
    #[must_use]
    pub fn is_neutral(self) -> bool {
        self.shadows.saturation == 0.0
            && self.midtones.saturation == 0.0
            && self.highlights.saturation == 0.0
    }
}

/// Color-panel parameters.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct ColorParams {
    /// Bands in hue order: red, orange, yellow, green, aqua, blue, purple, magenta.
    pub hsl: [HslBand; HSL_BAND_COUNT],
    /// -100.0..=100.0, 0 neutral.
    pub vibrance: f32,
    /// -100.0..=100.0, 0 neutral.
    pub saturation: f32,
    /// Tonal color wheels, neutral when every wheel saturation is zero.
    #[serde(default)]
    pub grading: ColorGradingParams,
    /// Convert to monochrome after HSL/grading.
    #[serde(default)]
    pub monochrome: bool,
    /// Per-hue contribution to monochrome luminance, -100.0..=100.0.
    #[serde(default)]
    pub monochrome_mix: [f32; HSL_BAND_COUNT],
    #[serde(default)]
    pub photographic: crate::correction::PhotographicColor,
}

/// Sharpening (Phase 3: unsharp mask on luminance).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SharpenParams {
    /// 0.0..=150.0, 0 = off (pass culled).
    pub amount: f32,
    /// Radius in pixels at working resolution, 0.5..=3.0.
    pub radius_px: f32,
    /// Detail preservation 0.0..=100.0 (masks flat areas), default 25.
    pub detail: f32,
    /// Minimum local contrast admitted by the edge mask, 0.0..=100.0.
    /// Higher values keep sharpening out of noise and smooth surfaces.
    #[serde(default = "default_sharpen_threshold")]
    pub threshold: f32,
    /// Suppress bright/dark overshoot around strong edges, 0.0..=100.0.
    #[serde(default = "default_halo_suppression")]
    pub halo_suppression: f32,
    /// Overall sharpness 0.0..=100.0, 0 = off. This is a convenience master
    /// control: the render pipeline never reads it. Moving it rewrites
    /// `amount`, `radius_px`, `detail`, `threshold` and `halo_suppression`
    /// with a balanced recipe (see [`SharpenParams::from_sharpness`]); the
    /// photographer can still fine-tune those individually afterwards.
    #[serde(default)]
    pub sharpness: f32,
    /// Regularized two-term inverse of a small Gaussian blur. Neutral at 0.
    #[serde(default)]
    pub restore_blur: f32,
    /// PSF sample spacing at the working resolution, 1..=2 pixels.
    #[serde(default = "default_restore_radius")]
    pub restore_radius_px: f32,
    /// Noise gate and correction guard, 0..=100 (higher is more conservative).
    #[serde(default = "default_restore_regularization")]
    pub restore_regularization: f32,
}

const fn default_restore_radius() -> f32 {
    1.0
}
const fn default_restore_regularization() -> f32 {
    50.0
}

const fn default_sharpen_threshold() -> f32 {
    25.0
}
const fn default_halo_suppression() -> f32 {
    50.0
}

impl Default for SharpenParams {
    fn default() -> Self {
        Self {
            amount: 0.0,
            radius_px: 1.0,
            detail: 25.0,
            threshold: default_sharpen_threshold(),
            halo_suppression: default_halo_suppression(),
            sharpness: 0.0,
            restore_blur: 0.0,
            restore_radius_px: default_restore_radius(),
            restore_regularization: default_restore_regularization(),
        }
    }
}

impl SharpenParams {
    /// Tolerance used when deciding whether the individual controls still
    /// match what the master `sharpness` slider would have produced.
    const SHARPNESS_MATCH_EPS: f32 = 0.05;

    /// The balanced recipe behind the master **Sharpness** slider.
    ///
    /// At `0` every control sits at its neutral default so the pass is culled.
    /// As sharpness rises the unsharp-mask amount grows fastest, the radius
    /// widens slightly for visible acutance, more fine detail is admitted, and
    /// the edge mask plus halo guard tighten so noise and overshoot do not
    /// grow with it. All outputs stay inside the ranges enforced by
    /// [`EditState::clamped`].
    #[must_use]
    pub fn from_sharpness(sharpness: f32) -> Self {
        let sharpness = sharpness.clamp(0.0, 100.0);
        let t = sharpness / 100.0;
        Self {
            amount: 120.0 * t,
            radius_px: 1.0 + 0.3 * t,
            detail: 25.0 + 40.0 * t,
            threshold: default_sharpen_threshold() + 15.0 * t,
            halo_suppression: default_halo_suppression() + 30.0 * t,
            sharpness,
            ..Self::default()
        }
    }

    /// Set the master sharpness and rewrite every dependent control from
    /// the recipe in [`SharpenParams::from_sharpness`].
    pub fn apply_sharpness(&mut self, sharpness: f32) {
        let (amount, radius, regularization) = (
            self.restore_blur,
            self.restore_radius_px,
            self.restore_regularization,
        );
        *self = Self::from_sharpness(sharpness);
        self.restore_blur = amount;
        self.restore_radius_px = radius;
        self.restore_regularization = regularization;
    }

    /// True when the individual controls still equal the recipe for the
    /// current `sharpness`, i.e. nothing has been hand-tuned since the master
    /// slider was last moved.
    #[must_use]
    pub fn follows_sharpness(&self) -> bool {
        let recipe = Self::from_sharpness(self.sharpness);
        let close = |a: f32, b: f32| (a - b).abs() <= Self::SHARPNESS_MATCH_EPS;
        close(self.amount, recipe.amount)
            && close(self.radius_px, recipe.radius_px)
            && close(self.detail, recipe.detail)
            && close(self.threshold, recipe.threshold)
            && close(self.halo_suppression, recipe.halo_suppression)
    }
}

/// Noise reduction (Phase 4). Defaults may be auto-seeded from ISO at import
/// time by writing non-default values into the sidecar; the struct default
/// stays neutral.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum CameraDenoiseMode {
    /// Use the regular, manually controlled noise-reduction settings.
    #[default]
    Off,
    /// Prefer the cleanest result, with the complete model output.
    #[serde(alias = "Purity")]
    Cleanest,
    /// Retain more luminance texture and fine structure.
    #[serde(alias = "Detail")]
    PreserveDetail,
}

impl CameraDenoiseMode {
    /// Blend of the neural RAW denoise into the source.
    ///
    /// `Cleanest` is the model's complete result. `PreserveDetail` keeps
    /// 20–25 % of the source: 20 % at ISO 64 and below, easing to 25 % at
    /// ISO 100 and above, so edges stay crisp without mixing most high-ISO
    /// noise back in.
    #[must_use]
    pub fn strength(self, iso: Option<u32>) -> f32 {
        match self {
            Self::Off => 0.0,
            Self::Cleanest => 1.0,
            Self::PreserveDetail => {
                let iso = iso.unwrap_or(100).max(1) as f32;
                let t =
                    ((iso.log2() - 64f32.log2()) / (100f32.log2() - 64f32.log2())).clamp(0.0, 1.0);
                0.80 - 0.05 * t
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct NrParams {
    /// ShopPhoto neural RAW denoise mode, offered for every Bayer RAW.
    #[serde(default)]
    pub camera_denoise_mode: CameraDenoiseMode,
    /// Hot-pixel suppression in raw prepare. On by default: it has no visible
    /// cost on healthy pixels and is considered part of "neutral" decoding.
    pub hot_pixels: bool,
    /// Luminance NR strength 0.0..=100.0, 0 = off (pass culled).
    pub luminance: f32,
    /// Luminance detail blend-back 0.0..=100.0, default 50.
    pub luminance_detail: f32,
    /// Chroma NR strength 0.0..=100.0, 0 = off (pass culled).
    pub chroma: f32,
    /// Chroma detail 0.0..=100.0, default 50.
    pub chroma_detail: f32,
}

impl NrParams {
    /// Sensible starting NR for a freshly imported image (BUILD.md Phase 4):
    /// chroma rises with ISO (cheap, no detail cost), luminance stays
    /// conservative until ISO 1600+. Only used when no sidecar exists —
    /// the struct `Default` stays neutral so undo/reset behave.
    #[must_use]
    pub fn seeded_for_iso(iso: Option<u32>) -> Self {
        let iso = iso.unwrap_or(0);
        let (luminance, chroma) = match iso {
            0..=400 => (0.0, 10.0),
            401..=800 => (0.0, 18.0),
            801..=1600 => (8.0, 25.0),
            1601..=3200 => (15.0, 32.0),
            3201..=6400 => (22.0, 40.0),
            _ => (30.0, 50.0),
        };
        Self {
            luminance,
            chroma,
            ..Default::default()
        }
    }
}

impl Default for NrParams {
    fn default() -> Self {
        Self {
            camera_denoise_mode: CameraDenoiseMode::Off,
            hot_pixels: true,
            luminance: 0.0,
            luminance_detail: 50.0,
            chroma: 0.0,
            chroma_detail: 50.0,
        }
    }
}

/// Lens shading (vignetting) correction.
///
/// Correction is applied pre-demosaic from a [`crate::lens::LensProfile`]
/// matched to the shot's lens. `enable` is on by default and costs nothing
/// when no profile matches — like `NrParams::hot_pixels`, removing a known
/// sensor/optics artifact is treated as part of neutral decoding rather than
/// as an edit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LensParams {
    /// Master toggle for lens corrections.
    pub enable: bool,
    /// Shading correction strength 0.0..=100.0. 100 applies the profile as
    /// measured; lower values fade toward the uncorrected frame, which is
    /// occasionally wanted because falloff can flatter a portrait.
    pub vignetting: f32,
    /// Explicit profile name. `None` auto-matches on the lens recorded in
    /// EXIF; an unknown name falls back to auto-match rather than erroring.
    #[serde(default)]
    pub profile: Option<String>,
    /// Manual lens identification used when the RAW omits a usable lens
    /// model. This is deliberately separate from `profile`: identifying an
    /// XCD lens must not pretend that proprietary Phocus calibration data is
    /// available to ShopPhoto.
    #[serde(default)]
    pub lens_model_override: Option<String>,
    /// Selective longitudinal chromatic-aberration removal. Unlike the
    /// profile-backed shading correction, this operates on developed RGB and
    /// therefore also works when no calibrated lens profile is installed.
    #[serde(default)]
    pub defringe: bool,
    /// Purple/magenta fringe correction strength, 0.0..=100.0.
    #[serde(default)]
    pub defringe_purple: f32,
    /// Green fringe correction strength, 0.0..=100.0.
    #[serde(default)]
    pub defringe_green: f32,
    /// Neighborhood radius in full-resolution image pixels, 1.0..=256.0.
    /// Values above roughly six pixels are intended for wide longitudinal-CA
    /// bloom around extreme highlights rather than ordinary lateral CA.
    #[serde(default = "default_defringe_radius")]
    pub defringe_radius_px: f32,
    /// Edge selectivity, 0.0..=100.0. Higher values protect real color by
    /// requiring a stronger dark-to-highlight transition.
    #[serde(default = "default_defringe_threshold")]
    pub defringe_threshold: f32,
    /// Center and half-width of the purple target range, in hue degrees.
    #[serde(default = "default_defringe_purple_hue")]
    pub defringe_purple_hue: f32,
    #[serde(default = "default_defringe_hue_range")]
    pub defringe_purple_range: f32,
    /// Center and half-width of the green target range, in hue degrees.
    #[serde(default = "default_defringe_green_hue")]
    pub defringe_green_hue: f32,
    #[serde(default = "default_defringe_hue_range")]
    pub defringe_green_range: f32,
    /// Relax the narrow-fringe neighbour test for broad, clipped chromatic
    /// bloom. Still requires a highlight edge and a local saturation outlier.
    #[serde(default)]
    pub defringe_wide_bloom: bool,
    /// Clean-room radial distortion coefficients (Brown-Conrady k1..k3).
    /// Zero is identity; coefficients come from a user-calibrated `.splens`.
    #[serde(default)]
    pub distortion: [f32; 3],
    /// Manual lateral-CA radial scale for red and blue relative to green.
    /// Values are fractional scale deltas, normally within +/-0.005.
    #[serde(default)]
    pub lateral_ca: [f32; 2],
    /// Zoom enough to avoid sampling outside the corrected image.
    #[serde(default = "default_lens_crop_to_valid")]
    pub crop_to_valid: bool,
}

const fn default_lens_crop_to_valid() -> bool {
    true
}

const fn default_defringe_radius() -> f32 {
    2.0
}

const fn default_defringe_threshold() -> f32 {
    55.0
}

const fn default_defringe_purple_hue() -> f32 {
    300.0
}

const fn default_defringe_green_hue() -> f32 {
    120.0
}

const fn default_defringe_hue_range() -> f32 {
    35.0
}

impl Default for LensParams {
    fn default() -> Self {
        Self {
            enable: true,
            vignetting: 100.0,
            profile: None,
            lens_model_override: None,
            defringe: false,
            defringe_purple: 0.0,
            defringe_green: 0.0,
            defringe_radius_px: default_defringe_radius(),
            defringe_threshold: default_defringe_threshold(),
            defringe_purple_hue: default_defringe_purple_hue(),
            defringe_purple_range: default_defringe_hue_range(),
            defringe_green_hue: default_defringe_green_hue(),
            defringe_green_range: default_defringe_hue_range(),
            defringe_wide_bloom: false,
            distortion: [0.0; 3],
            lateral_ca: [0.0; 2],
            crop_to_valid: true,
        }
    }
}

/// Working-resolution mode (Phase 5). `Super2x2` maps each Bayer quad to one
/// RGB pixel: 100 MP → 25 MP, 50 MP → 12.5 MP, with ~1 stop noise benefit.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResolutionMode {
    #[default]
    Full,
    Super2x2,
    Super3x3,
    Super4x4,
}

impl ResolutionMode {
    /// Linear downscale factor relative to full resolution.
    #[must_use]
    pub fn factor(self) -> u32 {
        match self {
            ResolutionMode::Full => 1,
            ResolutionMode::Super2x2 => 2,
            ResolutionMode::Super3x3 => 3,
            ResolutionMode::Super4x4 => 4,
        }
    }
}

/// Resolution parameters (Phase 5).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolutionParams {
    pub mode: ResolutionMode,
}

/// HDR output intent (Phase 6).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum HdrTarget {
    #[default]
    Sdr,
    Hdr,
}

/// HDR output intent (Phase 6).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct HdrParams {
    /// When false the edit targets SDR; scene headroom is tone-mapped away
    /// in the output transform.
    pub enabled: bool,
    /// Whether supported exports/display should preserve HDR headroom.
    #[serde(default)]
    pub target: HdrTarget,
    /// Highlight headroom to preserve above SDR white, in stops, 0.0..=4.0.
    pub headroom_stops: f32,
    /// Shoulder softness of the highlight rolloff, 0.0..=100.0, default 50.
    pub highlight_rolloff: f32,
    /// Scene exposure assigned to SDR reference white, -2.0..=2.0 EV.
    #[serde(default = "default_scene_white_ev")]
    pub scene_white_ev: f32,
}

const fn default_scene_white_ev() -> f32 {
    -0.4
}

impl Default for HdrParams {
    fn default() -> Self {
        Self {
            enabled: false,
            target: HdrTarget::Sdr,
            headroom_stops: 2.0,
            highlight_rolloff: 50.0,
            scene_white_ev: default_scene_white_ev(),
        }
    }
}

/// Crop and rotation (Phase 3). Crop rect is normalized to the oriented
/// active-area image, so it survives resolution-mode changes.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct GeometryParams {
    /// Normalized crop [left, top, right, bottom], each 0..=1;
    /// neutral = [0, 0, 1, 1].
    pub crop: [f32; 4],
    /// Straighten angle in degrees, -45.0..=45.0, 0 neutral.
    pub rotate_deg: f32,
    pub flip_horizontal: bool,
    #[serde(default)]
    pub flip_vertical: bool,
    /// Clockwise quarter turns, normalized to 0..=3 when loaded.
    #[serde(default)]
    pub quarter_turns: u8,
    /// Horizontal and vertical keystone, -100.0..=100.0.
    #[serde(default)]
    pub perspective_horizontal: f32,
    #[serde(default)]
    pub perspective_vertical: f32,
    /// Geometry scale, 10.0..=400.0 percent.
    #[serde(default = "default_geometry_scale")]
    pub scale: f32,
}

/// Intended direction of a user-drawn upright guide.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UprightGuideKind {
    Vertical,
    Horizontal,
}

/// A line drawn over an edge that should be vertical or horizontal.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct UprightGuide {
    pub start: [f32; 2],
    pub end: [f32; 2],
    pub kind: UprightGuideKind,
}

/// Geometry values estimated from two or four guided-upright lines.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UprightSolution {
    pub rotate_deg: f32,
    pub perspective_horizontal: f32,
    pub perspective_vertical: f32,
}

/// Solve rotation and keystone from user-marked scene edges.
/// Coordinates are normalized image UVs. Two guides correct rotation; a
/// parallel pair also estimates convergence. Four guides (one pair per axis)
/// solve both keystone axes.
pub fn solve_guided_upright(guides: &[UprightGuide]) -> Option<UprightSolution> {
    let valid: Vec<_> = guides
        .iter()
        .copied()
        .filter(|guide| {
            guide.start.into_iter().chain(guide.end).all(f32::is_finite)
                && (guide.end[0] - guide.start[0]).hypot(guide.end[1] - guide.start[1]) > 0.03
        })
        .collect();
    if valid.len() < 2 {
        return None;
    }
    let vertical: Vec<_> = valid
        .iter()
        .filter(|guide| guide.kind == UprightGuideKind::Vertical)
        .collect();
    let horizontal: Vec<_> = valid
        .iter()
        .filter(|guide| guide.kind == UprightGuideKind::Horizontal)
        .collect();
    let vertical_angle = vertical.iter().map(|guide| {
        let dx = guide.end[0] - guide.start[0];
        let dy = guide.end[1] - guide.start[1];
        dx.atan2(dy).to_degrees()
    });
    let horizontal_angle = horizontal.iter().map(|guide| {
        let dx = guide.end[0] - guide.start[0];
        let dy = guide.end[1] - guide.start[1];
        dy.atan2(dx).to_degrees()
    });
    let angles: Vec<_> = vertical_angle.chain(horizontal_angle).collect();
    let rotate_deg = -(angles.iter().sum::<f32>() / angles.len() as f32).clamp(-45.0, 45.0);

    let perspective_vertical = if vertical.len() >= 2 {
        let edge_x = |guide: &UprightGuide, y: f32| {
            let dy = guide.end[1] - guide.start[1];
            if dy.abs() < 1e-5 {
                (guide.start[0] + guide.end[0]) * 0.5
            } else {
                guide.start[0] + (y - guide.start[1]) * (guide.end[0] - guide.start[0]) / dy
            }
        };
        let mut pair = [vertical[0], vertical[1]];
        pair.sort_by(|a, b| {
            ((a.start[0] + a.end[0]) * 0.5).total_cmp(&((b.start[0] + b.end[0]) * 0.5))
        });
        let top = (edge_x(pair[1], 0.0) - edge_x(pair[0], 0.0)).abs();
        let bottom = (edge_x(pair[1], 1.0) - edge_x(pair[0], 1.0)).abs();
        ((bottom - top) / ((bottom + top) * 0.5).max(0.05) * 100.0).clamp(-100.0, 100.0)
    } else {
        0.0
    };
    let perspective_horizontal = if horizontal.len() >= 2 {
        let edge_y = |guide: &UprightGuide, x: f32| {
            let dx = guide.end[0] - guide.start[0];
            if dx.abs() < 1e-5 {
                (guide.start[1] + guide.end[1]) * 0.5
            } else {
                guide.start[1] + (x - guide.start[0]) * (guide.end[1] - guide.start[1]) / dx
            }
        };
        let mut pair = [horizontal[0], horizontal[1]];
        pair.sort_by(|a, b| {
            ((a.start[1] + a.end[1]) * 0.5).total_cmp(&((b.start[1] + b.end[1]) * 0.5))
        });
        let left = (edge_y(pair[1], 0.0) - edge_y(pair[0], 0.0)).abs();
        let right = (edge_y(pair[1], 1.0) - edge_y(pair[0], 1.0)).abs();
        ((right - left) / ((right + left) * 0.5).max(0.05) * 100.0).clamp(-100.0, 100.0)
    } else {
        0.0
    };
    Some(UprightSolution {
        rotate_deg,
        perspective_horizontal,
        perspective_vertical,
    })
}

const fn default_geometry_scale() -> f32 {
    100.0
}

impl Default for GeometryParams {
    fn default() -> Self {
        Self {
            crop: [0.0, 0.0, 1.0, 1.0],
            rotate_deg: 0.0,
            flip_horizontal: false,
            flip_vertical: false,
            quarter_turns: 0,
            perspective_horizontal: 0.0,
            perspective_vertical: 0.0,
            scale: default_geometry_scale(),
        }
    }
}

/// Deterministic output-resolution film grain.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct GrainParams {
    /// Grain opacity, 0.0..=100.0. Zero culls the effect.
    pub amount: f32,
    /// Apparent grain diameter, 0.5..=4.0 output pixels.
    pub size: f32,
    /// Mix between fine Gaussian and clustered grain, 0.0..=100.0.
    pub roughness: f32,
    /// Stable per-edit seed.
    pub seed: u32,
}

impl Default for GrainParams {
    fn default() -> Self {
        Self {
            amount: 0.0,
            size: 1.0,
            roughness: 50.0,
            seed: 1,
        }
    }
}

/// One non-destructive heal/clone correction in normalized image coordinates.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpotCorrection {
    pub target: [f32; 2],
    pub source: [f32; 2],
    pub radius: f32,
    pub feather: f32,
    #[serde(default = "default_spot_opacity")]
    pub opacity: f32,
    #[serde(default = "default_spot_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub clone: bool,
    /// Texture-preserving healing blend, 0..=100; zero preserves legacy healing.
    #[serde(default)]
    pub heal_texture: f32,
    /// Optional stroke samples; empty means a circular correction at target.
    #[serde(default)]
    pub stroke: Vec<[f32; 2]>,
}

const fn default_spot_opacity() -> f32 {
    1.0
}
const fn default_spot_enabled() -> bool {
    true
}

impl Default for SpotCorrection {
    fn default() -> Self {
        Self {
            target: [0.5, 0.5],
            source: [0.4, 0.5],
            radius: 0.03,
            feather: 0.5,
            opacity: 1.0,
            enabled: true,
            clone: false,
            heal_texture: 0.0,
            stroke: Vec::new(),
        }
    }
}

/// Boolean operations preserve soft coverage (union, difference, intersection).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MaskOperation {
    Add,
    Subtract,
    Intersect,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum LocalMask {
    Paint {
        strokes: Vec<crate::mask::PaintStroke>,
        settings: crate::mask::BrushSettings,
    },
    Composite {
        operation: MaskOperation,
        left: Box<LocalMask>,
        right: Box<LocalMask>,
    },
    /// Used when composing a mask with its existing coverage modifiers.
    Modified {
        mask: Box<LocalMask>,
        inverted: bool,
        opacity: f32,
    },
    Alpha(crate::mask::AlphaMask),
    Brush {
        points: Vec<[f32; 2]>,
        size: f32,
        feather: f32,
    },
    Radial {
        center: [f32; 2],
        radius: [f32; 2],
        feather: f32,
    },
    LinearGradient {
        start: [f32; 2],
        end: [f32; 2],
    },
    LuminanceRange {
        minimum: f32,
        maximum: f32,
        feather: f32,
    },
    /// Perceptual color-distance selection. The target is Oklab a/b;
    /// `radius` is the full-strength chroma distance and `feather` is the
    /// additional falloff width.
    ColorRange {
        target_ab: [f32; 2],
        radius: f32,
        feather: f32,
    },
}

/// One masked local edit. The same tone/color semantics as global controls
/// keep copy/paste and future GPU evaluation deterministic.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LocalAdjustment {
    pub name: String,
    pub enabled: bool,
    pub mask: LocalMask,
    pub tone: ToneParams,
    pub color: ColorParams,
    #[serde(default)]
    pub mask_inverted: bool,
    /// Final effect coverage, 0..=1. Zero bypasses this adjustment.
    #[serde(default = "default_local_opacity")]
    pub opacity: f32,
    /// Relative developed-image white-balance offsets, -100..=100.
    #[serde(default)]
    pub temperature: f32,
    #[serde(default)]
    pub tint: f32,
    #[serde(default)]
    pub curve: CurveParams,
    #[serde(default)]
    pub local: LocalContrastParams,
    /// Masked unsharp detail enhancement, 0..=100.
    #[serde(default)]
    pub sharpness: f32,
    /// Masked edge-aware noise reduction, 0..=100.
    #[serde(default)]
    pub noise_reduction: f32,
    #[serde(default)]
    pub corrective: crate::correction::LocalCorrection,
}

const fn default_local_opacity() -> f32 {
    1.0
}

impl Default for LocalAdjustment {
    fn default() -> Self {
        Self {
            name: "Local adjustment".into(),
            enabled: true,
            mask: LocalMask::LinearGradient {
                start: [0.0, 0.0],
                end: [0.0, 0.0],
            },
            tone: ToneParams::default(),
            color: ColorParams::default(),
            mask_inverted: false,
            opacity: 1.0,
            temperature: 0.0,
            tint: 0.0,
            curve: CurveParams::default(),
            local: LocalContrastParams::default(),
            sharpness: 0.0,
            noise_reduction: 0.0,
            corrective: Default::default(),
        }
    }
}

impl LocalAdjustment {
    /// Capture coverage including inversion/opacity, for non-destructive composition.
    #[must_use]
    pub fn coverage_mask(&self) -> LocalMask {
        if !self.mask_inverted && self.opacity == 1.0 {
            self.mask.clone()
        } else {
            LocalMask::Modified {
                mask: Box::new(self.mask.clone()),
                inverted: self.mask_inverted,
                opacity: self.opacity,
            }
        }
    }
}

impl LocalMask {
    /// Bound composition depth and node count before GPU evaluation.
    pub fn clamp_valid(&mut self) -> bool {
        self.clamp_mask(0, &mut 1024)
    }

    fn clamp_mask(&mut self, depth: usize, remaining: &mut usize) -> bool {
        if depth >= 32 || *remaining == 0 {
            return false;
        }
        *remaining -= 1;
        let c = |v: f32, lo: f32, hi: f32| if v.is_finite() { v.clamp(lo, hi) } else { lo };
        match self {
            LocalMask::Paint { strokes, settings } => {
                settings.size = c(settings.size, 0.0001, 0.5);
                settings.feather = c(settings.feather, 0.0, 1.0);
                settings.flow = c(settings.flow, 0.0, 1.0);
                settings.spacing = c(settings.spacing, 0.01, 1.0);
                settings.smoothing = c(settings.smoothing, 0.0, 0.9);
                if strokes.iter().map(|s| s.dabs.len()).sum::<usize>() > 1_000_000 {
                    return false;
                }
                for stroke in strokes {
                    for dab in &mut stroke.dabs {
                        for v in &mut dab.point {
                            *v = c(*v, 0.0, 1.0);
                        }
                        dab.pressure = c(dab.pressure, 0.0, 1.0);
                    }
                }
            }
            LocalMask::Composite { left, right, .. } => {
                if !left.clamp_mask(depth + 1, remaining) || !right.clamp_mask(depth + 1, remaining)
                {
                    return false;
                }
            }
            LocalMask::Modified { mask, opacity, .. } => {
                *opacity = c(*opacity, 0.0, 1.0);
                if !mask.clamp_mask(depth + 1, remaining) {
                    return false;
                }
            }
            LocalMask::Alpha(mask) => {
                if !mask.valid() {
                    return false;
                }
            }
            LocalMask::Brush {
                points,
                size,
                feather,
            } => {
                for point in points {
                    point[0] = c(point[0], 0.0, 1.0);
                    point[1] = c(point[1], 0.0, 1.0);
                }
                *size = c(*size, 0.0001, 1.0);
                *feather = c(*feather, 0.0, 1.0);
            }
            LocalMask::Radial {
                center,
                radius,
                feather,
            } => {
                for value in center {
                    *value = c(*value, 0.0, 1.0);
                }
                for value in radius {
                    *value = c(*value, 0.0001, 1.0);
                }
                *feather = c(*feather, 0.0, 1.0);
            }
            LocalMask::LinearGradient { start, end } => {
                for value in start.iter_mut().chain(end.iter_mut()) {
                    *value = c(*value, 0.0, 1.0);
                }
            }
            LocalMask::LuminanceRange {
                minimum,
                maximum,
                feather,
            } => {
                *minimum = c(*minimum, 0.0, 1.0);
                *maximum = c(*maximum, *minimum, 1.0);
                *feather = c(*feather, 0.0, 1.0);
            }
            LocalMask::ColorRange {
                target_ab,
                radius,
                feather,
            } => {
                for value in target_ab {
                    *value = c(*value, -0.5, 0.5);
                }
                *radius = c(*radius, 0.001, 0.5);
                *feather = c(*feather, 0.0, 0.5);
            }
        }
        true
    }
}

/// Which camera color profile renders this image (BUILD.md §3.4).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProfileChoice {
    /// Tier 1: embedded camera matrix + ShopPhoto base curve.
    #[default]
    Standard,
    /// Tier 2/3: a named imported DCP or fitted Reference Match profile.
    /// Resolved by name in the profile store; missing ⇒ fall back to
    /// `Standard` and show a warning badge (never an error).
    Named(String),
}

/// Preview/proof target. This is view state, never serialized in EditState.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProofSpace {
    #[default]
    DisplayNative,
    Srgb,
    DisplayP3,
    Rec2020,
    /// An imported display-class RGB ICC profile, held in application state.
    Custom,
}

/// The complete, serializable description of one image's edit.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct EditState {
    pub profile: ProfileChoice,
    /// Built-in look name. Unknown names render as Standard.
    #[serde(default)]
    pub look: Option<String>,
    /// Creative look strength. `None` is 100% for backward compatibility.
    #[serde(default)]
    pub look_strength: Option<f32>,
    pub wb: WhiteBalance,
    #[serde(default)]
    pub negative: crate::correction::FilmNegative,
    pub tone: ToneParams,
    #[serde(default)]
    pub highlight_recovery: HighlightRecoveryParams,
    pub curve: CurveParams,
    pub color: ColorParams,
    #[serde(default)]
    pub local: LocalContrastParams,
    #[serde(default)]
    pub vignette: VignetteParams,
    pub sharpen: SharpenParams,
    /// Edge-directed AHD demosaic for 1:1 viewing and export. The interactive
    /// proxy deliberately remains on its fast superpixel path.
    #[serde(default)]
    pub high_detail_demosaic: bool,
    #[serde(default)]
    pub grain: GrainParams,
    /// Stored spot corrections. Rendering implementations may cap the live
    /// GPU set, but sidecars retain the complete list.
    #[serde(default)]
    pub spots: Vec<SpotCorrection>,
    #[serde(default)]
    pub local_adjustments: Vec<LocalAdjustment>,
    /// Persistent measurement annotations, with no effect on developed pixels.
    #[serde(default)]
    pub sample_points: Vec<crate::analysis::ColorSamplePoint>,
    pub nr: NrParams,
    #[serde(default)]
    pub lens: LensParams,
    pub resolution: ResolutionParams,
    pub hdr: HdrParams,
    pub geometry: GeometryParams,
}

impl EditState {
    /// Returns a copy with every field clamped to its documented range.
    /// Applied when loading sidecars (defense against hand-edited files).
    #[must_use]
    pub fn clamped(mut self) -> Self {
        let c = |v: f32, lo: f32, hi: f32| v.clamp(lo, hi);
        if let Some(t) = self.wb.temp_kelvin {
            self.wb.temp_kelvin = Some(c(t, 2_000.0, 50_000.0));
        }
        self.wb.tint = c(self.wb.tint, -150.0, 150.0);
        self.negative = self.negative.clamped();
        self.tone.exposure_ev = c(self.tone.exposure_ev, -5.0, 5.0);
        self.tone.brightness = c(self.tone.brightness, -100.0, 100.0);
        self.highlight_recovery.strength = c(self.highlight_recovery.strength, 0.0, 100.0);
        self.highlight_recovery.radius = self.highlight_recovery.radius.clamp(1, 32);
        if let Some(strength) = &mut self.look_strength {
            *strength = c(*strength, 0.0, 100.0);
        }
        for v in [
            &mut self.tone.contrast,
            &mut self.tone.highlights,
            &mut self.tone.shadows,
            &mut self.tone.whites,
            &mut self.tone.blacks,
            &mut self.color.vibrance,
            &mut self.color.saturation,
        ] {
            *v = c(*v, -100.0, 100.0);
        }
        for curve in [
            &mut self.curve.luma,
            &mut self.curve.red,
            &mut self.curve.green,
            &mut self.curve.blue,
        ] {
            for point in &mut curve.points {
                point.x = c(point.x, 0.0, 1.0);
                point.y = c(point.y, 0.0, 1.0);
            }
            curve
                .points
                .sort_by(|left, right| left.x.total_cmp(&right.x));
            if curve.points.len() < 2 {
                *curve = ToneCurve::default();
            } else {
                curve.points.first_mut().expect("two points").x = 0.0;
                curve.points.last_mut().expect("two points").x = 1.0;
            }
        }
        self.color.photographic = self.color.photographic.clamped();
        for band in &mut self.color.hsl {
            band.hue_shift = c(band.hue_shift, -100.0, 100.0);
            band.saturation = c(band.saturation, -100.0, 100.0);
            band.luminance = c(band.luminance, -100.0, 100.0);
        }
        for wheel in [
            &mut self.color.grading.shadows,
            &mut self.color.grading.midtones,
            &mut self.color.grading.highlights,
        ] {
            wheel.hue_degrees = c(wheel.hue_degrees, 0.0, 360.0);
            wheel.saturation = c(wheel.saturation, 0.0, 100.0);
        }
        self.color.grading.balance = c(self.color.grading.balance, -100.0, 100.0);
        for value in &mut self.color.monochrome_mix {
            *value = c(*value, -100.0, 100.0);
        }
        for v in [
            &mut self.curve.parametric.highlights,
            &mut self.curve.parametric.lights,
            &mut self.curve.parametric.darks,
            &mut self.curve.parametric.shadows,
            &mut self.local.clarity,
            &mut self.local.texture,
            &mut self.local.dehaze,
        ] {
            *v = c(*v, -100.0, 100.0);
        }
        self.vignette.amount = c(self.vignette.amount, -100.0, 100.0);
        self.vignette.midpoint = c(self.vignette.midpoint, 0.0, 100.0);
        self.vignette.roundness = c(self.vignette.roundness, 0.0, 100.0);
        self.vignette.feather = c(self.vignette.feather, 0.0, 100.0);
        self.sharpen.amount = c(self.sharpen.amount, 0.0, 150.0);
        self.sharpen.radius_px = c(self.sharpen.radius_px, 0.5, 3.0);
        self.sharpen.detail = c(self.sharpen.detail, 0.0, 100.0);
        self.sharpen.threshold = c(self.sharpen.threshold, 0.0, 100.0);
        self.sharpen.halo_suppression = c(self.sharpen.halo_suppression, 0.0, 100.0);
        self.sharpen.sharpness = c(self.sharpen.sharpness, 0.0, 100.0);
        self.sharpen.restore_blur = c(self.sharpen.restore_blur, 0.0, 100.0);
        self.sharpen.restore_radius_px = c(self.sharpen.restore_radius_px, 1.0, 2.0);
        self.sharpen.restore_regularization = c(self.sharpen.restore_regularization, 0.0, 100.0);
        self.grain.amount = c(self.grain.amount, 0.0, 100.0);
        self.grain.size = c(self.grain.size, 0.5, 4.0);
        self.grain.roughness = c(self.grain.roughness, 0.0, 100.0);
        for spot in &mut self.spots {
            spot.target[0] = c(spot.target[0], 0.0, 1.0);
            spot.target[1] = c(spot.target[1], 0.0, 1.0);
            spot.source[0] = c(spot.source[0], 0.0, 1.0);
            spot.source[1] = c(spot.source[1], 0.0, 1.0);
            spot.radius = c(spot.radius, 0.0001, 0.5);
            spot.feather = c(spot.feather, 0.0, 1.0);
            spot.opacity = c(spot.opacity, 0.0, 1.0);
            spot.heal_texture = c(spot.heal_texture, 0.0, 100.0);
            for point in &mut spot.stroke {
                point[0] = c(point[0], 0.0, 1.0);
                point[1] = c(point[1], 0.0, 1.0);
            }
        }
        for point in &mut self.sample_points {
            for value in &mut point.position {
                *value = if value.is_finite() {
                    value.clamp(0.0, 1.0)
                } else {
                    0.5
                };
            }
        }
        for adjustment in &mut self.local_adjustments {
            if !adjustment.mask.clamp_valid() {
                adjustment.enabled = false;
            }
            adjustment.opacity = c(adjustment.opacity, 0.0, 1.0);
            adjustment.corrective = adjustment.corrective.clamped();
            adjustment.temperature = c(adjustment.temperature, -100.0, 100.0);
            adjustment.tint = c(adjustment.tint, -100.0, 100.0);
            adjustment.sharpness = c(adjustment.sharpness, 0.0, 100.0);
            adjustment.noise_reduction = c(adjustment.noise_reduction, 0.0, 100.0);
            let controls = EditState {
                tone: adjustment.tone,
                color: adjustment.color,
                curve: adjustment.curve.clone(),
                local: adjustment.local,
                ..Default::default()
            }
            .clamped();
            adjustment.tone = controls.tone;
            adjustment.color = controls.color;
            adjustment.curve = controls.curve;
            adjustment.local = controls.local;
        }
        self.nr.luminance = c(self.nr.luminance, 0.0, 100.0);
        self.nr.luminance_detail = c(self.nr.luminance_detail, 0.0, 100.0);
        self.nr.chroma = c(self.nr.chroma, 0.0, 100.0);
        self.nr.chroma_detail = c(self.nr.chroma_detail, 0.0, 100.0);
        self.lens.vignetting = c(self.lens.vignetting, 0.0, 100.0);
        self.lens.defringe_purple = c(self.lens.defringe_purple, 0.0, 100.0);
        self.lens.defringe_green = c(self.lens.defringe_green, 0.0, 100.0);
        self.lens.defringe_radius_px = c(self.lens.defringe_radius_px, 1.0, 256.0);
        self.lens.defringe_threshold = c(self.lens.defringe_threshold, 0.0, 100.0);
        self.lens.defringe_purple_hue = c(self.lens.defringe_purple_hue, 0.0, 360.0);
        self.lens.defringe_purple_range = c(self.lens.defringe_purple_range, 2.0, 90.0);
        self.lens.defringe_green_hue = c(self.lens.defringe_green_hue, 0.0, 360.0);
        self.lens.defringe_green_range = c(self.lens.defringe_green_range, 2.0, 90.0);
        for value in &mut self.lens.distortion {
            *value = c(*value, -1.0, 1.0);
        }
        for value in &mut self.lens.lateral_ca {
            *value = c(*value, -0.02, 0.02);
        }
        self.hdr.headroom_stops = c(self.hdr.headroom_stops, 0.0, 4.0);
        self.hdr.highlight_rolloff = c(self.hdr.highlight_rolloff, 0.0, 100.0);
        self.hdr.scene_white_ev = c(self.hdr.scene_white_ev, -2.0, 2.0);
        for v in &mut self.geometry.crop {
            *v = c(*v, 0.0, 1.0);
        }
        self.geometry.rotate_deg = c(self.geometry.rotate_deg, -45.0, 45.0);
        self.geometry.quarter_turns %= 4;
        self.geometry.perspective_horizontal =
            c(self.geometry.perspective_horizontal, -100.0, 100.0);
        self.geometry.perspective_vertical = c(self.geometry.perspective_vertical, -100.0, 100.0);
        self.geometry.scale = c(self.geometry.scale, 10.0, 400.0);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_neutral_and_stable_under_clamp() {
        let d = EditState::default();
        assert_eq!(d.clone().clamped(), d);
        assert!(d.curve.luma.is_neutral());
        assert!(d.curve.channels_are_neutral());
        assert!(d.local.is_neutral());
        assert_eq!(d.vignette, VignetteParams::default());
        assert_eq!(d.hdr.scene_white_ev, -0.4);
        assert_eq!(d.tone, ToneParams::default());
        assert_eq!(d.color.grading, ColorGradingParams::default());
        let mut hue_only = d.color.grading;
        hue_only.shadows.hue_degrees = 240.0;
        hue_only.balance = 50.0;
        assert!(hue_only.is_neutral());
        assert!(!d.color.monochrome);
        assert_eq!(d.color.monochrome_mix, [0.0; HSL_BAND_COUNT]);
        assert_eq!(d.resolution.mode, ResolutionMode::Full);
        assert!(!d.hdr.enabled);
        assert_eq!(d.geometry.crop, [0.0, 0.0, 1.0, 1.0]);
    }

    #[test]
    fn guided_upright_solves_rotation_and_vertical_convergence() {
        let guides = [
            UprightGuide {
                start: [0.20, 0.10],
                end: [0.24, 0.90],
                kind: UprightGuideKind::Vertical,
            },
            UprightGuide {
                start: [0.80, 0.10],
                end: [0.72, 0.90],
                kind: UprightGuideKind::Vertical,
            },
        ];
        let solution = solve_guided_upright(&guides).unwrap();
        assert!(solution.rotate_deg.abs() < 2.0, "{solution:?}");
        assert!(solution.perspective_vertical < -10.0, "{solution:?}");
        assert_eq!(solution.perspective_horizontal, 0.0);
    }

    #[test]
    fn guided_upright_needs_two_real_lines() {
        assert!(solve_guided_upright(&[]).is_none());
        assert!(solve_guided_upright(&[UprightGuide {
            start: [0.5, 0.5],
            end: [0.5, 0.5],
            kind: UprightGuideKind::Vertical,
        }])
        .is_none());
    }

    #[test]
    fn legacy_local_adjustment_defaults_are_neutral() {
        let local = LocalAdjustment {
            mask: LocalMask::Radial {
                center: [0.5, 0.5],
                radius: [0.2, 0.2],
                feather: 0.5,
            },
            ..Default::default()
        };
        let mut value = serde_json::to_value(&local).unwrap();
        for key in [
            "mask_inverted",
            "opacity",
            "temperature",
            "tint",
            "curve",
            "local",
            "sharpness",
            "noise_reduction",
        ] {
            value.as_object_mut().unwrap().remove(key);
        }
        let restored: LocalAdjustment = serde_json::from_value(value).unwrap();
        assert_eq!(local, restored);
    }

    #[test]
    fn compound_masks_round_trip_and_reject_excessive_depth() {
        let mut mask = LocalMask::Radial {
            center: [0.5, 0.5],
            radius: [0.2, 0.2],
            feather: 0.5,
        };
        for _ in 0..10 {
            mask = LocalMask::Composite {
                operation: MaskOperation::Subtract,
                left: Box::new(mask),
                right: Box::new(LocalMask::LuminanceRange {
                    minimum: 0.1,
                    maximum: 0.3,
                    feather: 0.05,
                }),
            };
        }
        let json = serde_json::to_string(&mask).unwrap();
        assert_eq!(serde_json::from_str::<LocalMask>(&json).unwrap(), mask);
        assert!(mask.clamp_valid());
        for _ in 0..32 {
            mask = LocalMask::Modified {
                mask: Box::new(mask),
                inverted: false,
                opacity: 1.0,
            };
        }
        let mut edit = EditState::default();
        edit.local_adjustments.push(LocalAdjustment {
            mask,
            ..Default::default()
        });
        assert!(!edit.clamped().local_adjustments[0].enabled);
    }

    #[test]
    fn serde_round_trip() {
        let mut e = EditState::default();
        e.tone.exposure_ev = 1.25;
        e.profile = ProfileChoice::Named("Reference Match - X2D".into());
        e.nr.chroma = 30.0;
        e.nr.camera_denoise_mode = CameraDenoiseMode::PreserveDetail;
        let json = serde_json::to_string(&e).expect("serialize");
        let back: EditState = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(e, back);
    }

    #[test]
    fn neural_blend_modes_have_distinct_semantics() {
        use CameraDenoiseMode::{Cleanest, Off, PreserveDetail};
        assert_eq!(Cleanest.strength(Some(100)), 1.0);
        assert_eq!(Cleanest.strength(Some(12_800)), 1.0);
        assert_eq!(Cleanest.strength(None), 1.0);
        assert_eq!(Off.strength(Some(100)), 0.0);
        for (iso, expected) in [
            (Some(25), 0.80),
            (Some(64), 0.80),
            (Some(100), 0.75),
            (Some(283), 0.75),
            (Some(6400), 0.75),
            (Some(102_400), 0.75),
            (None, 0.75),
        ] {
            assert!(
                (PreserveDetail.strength(iso) - expected).abs() < 1e-6,
                "{iso:?}"
            );
        }
        let mut last = 1.0;
        for iso in [50, 64, 80, 100, 400, 3200, 25600] {
            let s = PreserveDetail.strength(Some(iso));
            assert!(s <= last + 1e-6, "iso {iso}: {s} > {last}");
            last = s;
        }
    }

    #[test]
    fn legacy_camera_denoise_mode_names_still_deserialize() {
        for (mode, legacy_name) in [
            (CameraDenoiseMode::Cleanest, "Purity"),
            (CameraDenoiseMode::PreserveDetail, "Detail"),
        ] {
            let nr = NrParams {
                camera_denoise_mode: mode,
                ..NrParams::default()
            };
            let current = serde_json::to_string(&nr).expect("serialize");
            assert!(current.contains("camera_denoise_mode"));
            let legacy = current.replace(
                match mode {
                    CameraDenoiseMode::Cleanest => "Cleanest",
                    CameraDenoiseMode::PreserveDetail => "PreserveDetail",
                    CameraDenoiseMode::Off => unreachable!(),
                },
                legacy_name,
            );
            let decoded: NrParams = serde_json::from_str(&legacy).expect("legacy aliases");
            assert_eq!(decoded, nr);
        }
    }

    #[test]
    fn pre_upgrade_curve_and_missing_fields_deserialize() {
        let json = r#"{
            "profile":"Standard",
            "wb":{"temp_kelvin":null,"tint":0.0},
            "tone":{"exposure_ev":0.0,"contrast":0.0,"highlights":0.0,"shadows":0.0,"whites":0.0,"blacks":0.0},
            "curve":{"points":[{"x":0.0,"y":0.0},{"x":0.5,"y":0.6},{"x":1.0,"y":1.0}]},
            "color":{"hsl":[{"hue_shift":0.0,"saturation":0.0,"luminance":0.0},{"hue_shift":0.0,"saturation":0.0,"luminance":0.0},{"hue_shift":0.0,"saturation":0.0,"luminance":0.0},{"hue_shift":0.0,"saturation":0.0,"luminance":0.0},{"hue_shift":0.0,"saturation":0.0,"luminance":0.0},{"hue_shift":0.0,"saturation":0.0,"luminance":0.0},{"hue_shift":0.0,"saturation":0.0,"luminance":0.0},{"hue_shift":0.0,"saturation":0.0,"luminance":0.0}],"vibrance":0.0,"saturation":0.0},
            "sharpen":{"amount":0.0,"radius_px":1.0,"detail":25.0},
            "nr":{"hot_pixels":true,"luminance":0.0,"luminance_detail":50.0,"chroma":0.0,"chroma_detail":50.0},
            "resolution":{"mode":"Full"},
            "hdr":{"enabled":false,"target":"Sdr","headroom_stops":2.0,"highlight_rolloff":50.0},
            "geometry":{"crop":[0.0,0.0,1.0,1.0],"rotate_deg":0.0,"flip_horizontal":false}
        }"#;
        let edit: EditState = serde_json::from_str(json).expect("old sidecar");
        assert_eq!(edit.curve.luma.points.len(), 3);
        assert_eq!(edit.tone.brightness, 0.0);
        assert!(edit.local.is_neutral());
        assert_eq!(edit.vignette, VignetteParams::default());
        assert_eq!(edit.lens, LensParams::default());
        assert_eq!(edit.hdr.scene_white_ev, -0.4);
    }

    #[test]
    fn lens_corrections_default_on_and_survive_a_round_trip() {
        // Like hot-pixel suppression, removing known optical falloff is part
        // of neutral decoding, so the default edit has it enabled.
        let d = EditState::default();
        assert!(d.lens.enable);
        assert_eq!(d.lens.vignetting, 100.0);
        assert!(d.lens.profile.is_none());
        assert!(d.lens.lens_model_override.is_none());
        assert!(!d.lens.defringe);
        assert_eq!(d.lens.defringe_radius_px, 2.0);
        assert_eq!(d.lens.defringe_threshold, 55.0);
        assert_eq!(d.lens.defringe_purple_hue, 300.0);
        assert_eq!(d.lens.defringe_green_hue, 120.0);

        let mut e = EditState::default();
        e.lens.enable = false;
        e.lens.vignetting = 60.0;
        e.lens.profile = Some("XCD 90mm".into());
        e.lens.lens_model_override = Some("XCD 90".into());
        e.lens.defringe = true;
        e.lens.defringe_purple = 44.0;
        e.lens.defringe_green = 21.0;
        let json = serde_json::to_string(&e).expect("serialize");
        assert_eq!(
            serde_json::from_str::<EditState>(&json).expect("deserialize"),
            e
        );
    }

    #[test]
    fn lens_vignetting_is_clamped() {
        let mut e = EditState::default();
        e.lens.vignetting = 999.0;
        assert_eq!(e.clone().clamped().lens.vignetting, 100.0);
        e.lens.vignetting = -5.0;
        assert_eq!(e.clamped().lens.vignetting, 0.0);
    }

    #[test]
    fn defringe_controls_are_clamped() {
        let mut edit = EditState::default();
        edit.lens.defringe_purple = 140.0;
        edit.lens.defringe_green = -3.0;
        edit.lens.defringe_radius_px = 20.0;
        edit.lens.defringe_threshold = -10.0;
        edit.lens.defringe_purple_hue = 999.0;
        edit.lens.defringe_purple_range = 0.0;
        edit.lens.defringe_green_hue = -2.0;
        edit.lens.defringe_green_range = 200.0;
        let lens = edit.clamped().lens;
        assert_eq!(lens.defringe_purple, 100.0);
        assert_eq!(lens.defringe_green, 0.0);
        assert_eq!(lens.defringe_radius_px, 20.0);
        assert_eq!(lens.defringe_threshold, 0.0);
        assert_eq!(lens.defringe_purple_hue, 360.0);
        assert_eq!(lens.defringe_purple_range, 2.0);
        assert_eq!(lens.defringe_green_hue, 0.0);
        assert_eq!(lens.defringe_green_range, 90.0);
    }

    #[test]
    fn clamp_rejects_out_of_range_sidecar_values() {
        let mut e = EditState::default();
        e.tone.exposure_ev = 99.0;
        e.wb.temp_kelvin = Some(100.0);
        e.geometry.rotate_deg = -400.0;
        e.tone.brightness = 999.0;
        e.local.dehaze = -999.0;
        e.vignette.feather = 999.0;
        e.hdr.scene_white_ev = 9.0;
        e.color.grading.shadows.hue_degrees = 999.0;
        e.color.grading.highlights.saturation = -20.0;
        e.color.grading.balance = 999.0;
        e.color.monochrome_mix[2] = -999.0;
        e.sharpen.sharpness = 999.0;
        let c = e.clamped();
        assert_eq!(c.sharpen.sharpness, 100.0);
        assert_eq!(c.tone.exposure_ev, 5.0);
        assert_eq!(c.wb.temp_kelvin, Some(2_000.0));
        assert_eq!(c.geometry.rotate_deg, -45.0);
        assert_eq!(c.tone.brightness, 100.0);
        assert_eq!(c.local.dehaze, -100.0);
        assert_eq!(c.vignette.feather, 100.0);
        assert_eq!(c.hdr.scene_white_ev, 2.0);
        assert_eq!(c.color.grading.shadows.hue_degrees, 360.0);
        assert_eq!(c.color.grading.highlights.saturation, 0.0);
        assert_eq!(c.color.grading.balance, 100.0);
        assert_eq!(c.color.monochrome_mix[2], -100.0);
    }

    #[test]
    fn sharpness_zero_is_the_neutral_default() {
        assert_eq!(SharpenParams::from_sharpness(0.0), SharpenParams::default());
        let mut sharpen = SharpenParams::from_sharpness(60.0);
        sharpen.apply_sharpness(0.0);
        assert_eq!(sharpen, SharpenParams::default());
    }

    #[test]
    fn sharpness_recipe_stays_in_range_and_is_monotonic() {
        let mut previous = SharpenParams::from_sharpness(0.0);
        for step in 1..=100 {
            let current = SharpenParams::from_sharpness(step as f32);
            let clamped = EditState {
                sharpen: current,
                ..EditState::default()
            }
            .clamped();
            assert_eq!(clamped.sharpen, current, "step {step} left the valid range");
            assert!(
                current.amount > previous.amount,
                "amount must grow at {step}"
            );
            assert!(current.follows_sharpness());
            previous = current;
        }
        assert!(SharpenParams::from_sharpness(150.0).sharpness <= 100.0);
        assert!(SharpenParams::from_sharpness(-5.0).sharpness >= 0.0);
    }

    #[test]
    fn hand_tuned_sharpening_no_longer_follows_master() {
        let mut sharpen = SharpenParams::from_sharpness(40.0);
        assert!(sharpen.follows_sharpness());
        sharpen.radius_px += 0.5;
        assert!(!sharpen.follows_sharpness());
        sharpen.apply_sharpness(40.0);
        assert!(sharpen.follows_sharpness());
    }

    #[test]
    fn sharpness_is_optional_in_old_sidecars() {
        let json = serde_json::to_string(&EditState::default()).unwrap();
        let stripped = json
            .replace("\"sharpness\":0.0,", "")
            .replace(",\"sharpness\":0.0", "");
        assert_ne!(json, stripped, "the fixture must exercise a missing field");
        let edit: EditState = serde_json::from_str(&stripped).unwrap();
        assert_eq!(edit.sharpen.sharpness, 0.0);
    }

    #[test]
    fn color_range_mask_is_serialized_and_clamped() {
        let mut edit = EditState::default();
        edit.local_adjustments.push(LocalAdjustment {
            name: "Blue range".into(),
            enabled: true,
            mask: LocalMask::ColorRange {
                target_ab: [-2.0, 3.0],
                radius: 0.0,
                feather: 2.0,
            },
            tone: ToneParams::default(),
            color: ColorParams::default(),
            ..LocalAdjustment::default()
        });
        let clamped = edit.clamped();
        assert_eq!(
            clamped.local_adjustments[0].mask,
            LocalMask::ColorRange {
                target_ab: [-0.5, 0.5],
                radius: 0.001,
                feather: 0.5,
            }
        );
        let json = serde_json::to_string(&clamped).expect("serialize");
        assert_eq!(
            serde_json::from_str::<EditState>(&json).expect("deserialize"),
            clamped
        );
    }

    #[test]
    fn nr_seeding_scales_with_iso() {
        assert_eq!(NrParams::seeded_for_iso(None).luminance, 0.0);
        let base = NrParams::seeded_for_iso(Some(100));
        let high = NrParams::seeded_for_iso(Some(6400));
        assert!(high.chroma > base.chroma);
        assert!(high.luminance > base.luminance);
        // Hot-pixel default stays on, details at defaults.
        assert!(high.hot_pixels);
        assert_eq!(high.luminance_detail, NrParams::default().luminance_detail);
    }

    #[test]
    fn supersample_factors() {
        assert_eq!(ResolutionMode::Full.factor(), 1);
        assert_eq!(ResolutionMode::Super2x2.factor(), 2);
        assert_eq!(ResolutionMode::Super4x4.factor(), 4);
    }
}
