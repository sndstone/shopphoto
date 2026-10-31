//! Public Hasselblad hardware facts and ShopPhoto processing defaults.
//!
//! Body names and sensor dimensions are public product facts, cross-checked
//! against metadata in RAW files decoded by ShopPhoto. Processing parameters
//! below are ShopPhoto-authored heuristics; no vendor tables, lens maps or
//! factory calibration payloads are included.
//!
//! Deliberately **no calibration payloads**: no colour tables, no shading
//! grids. Those are Hasselblad's measured data. What lives here is the
//! scaffolding that makes our own calibration land in the right place — grid
//! anchors for [`crate::lens`], and target illuminants for profile fitting.
//!
//! ## Optical centre
//!
//! Shading correction is anchored at the geometric centre of the full sensor
//! readout. [`crate::lens::GridGeometry::for_sensor`] derives that centre from
//! dimensions supplied by the decoded RAW, so no camera-maker map is needed.

use crate::edit::CameraDenoiseMode;
use crate::lens::{GridGeometry, DEFAULT_GRID_SPACING_PX};

/// One sensor generation: the physical readout behind a family of bodies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SensorGeneration {
    /// Short label used in the UI and in generated profile names.
    pub label: &'static str,
    /// Active-area readout in photosites.
    pub width_px: u32,
    pub height_px: u32,
    /// Photosite pitch in micrometres ×1000 (i.e. nanometres), kept integral
    /// so the table stays `Eq`. 3760 nm = 3.76 µm.
    pub pitch_nm: u32,
    /// Bodies carrying this sensor.
    pub bodies: &'static [&'static str],
}

impl SensorGeneration {
    /// Photosite pitch in millimetres.
    #[must_use]
    pub fn pitch_mm(&self) -> f32 {
        self.pitch_nm as f32 / 1.0e6
    }

    /// Active area in millimetres.
    #[must_use]
    pub fn active_size_mm(&self) -> (f32, f32) {
        (
            self.width_px as f32 * self.pitch_mm(),
            self.height_px as f32 * self.pitch_mm(),
        )
    }
}

/// The medium-format sensor generations ShopPhoto cares about.
///
/// Dimensions and pitch are public product/RAW-metadata facts. The two 100 MP
/// body generations remain distinct so ShopPhoto can tune its own processing
/// independently while sharing geometry.
pub const SENSORS: &[SensorGeneration] = &[
    SensorGeneration {
        label: "50MP",
        width_px: 8272,
        height_px: 6200,
        pitch_nm: 5300,
        bodies: &["X1D II 50C", "CFV II 50C", "907X 50C"],
    },
    SensorGeneration {
        label: "100MP",
        width_px: 11656,
        height_px: 8742,
        pitch_nm: 3760,
        bodies: &["X2D 100C", "CFV 100C"],
    },
    SensorGeneration {
        label: "100MP-II",
        width_px: 11656,
        height_px: 8742,
        pitch_nm: 3760,
        bodies: &["X2D II 100C"],
    },
];

/// Sensor families with ShopPhoto-tuned GPU noise-reduction fallbacks.
/// Keeping this separate from sensor geometry makes support an allow-list:
/// adding a generic RAW decoder never accidentally exposes the control.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CameraDenoiseFamily {
    FiftyMp,
    HundredMp,
    HundredMpIi,
}

impl CameraDenoiseFamily {
    #[must_use]
    pub fn display_name(self) -> &'static str {
        match self {
            Self::FiftyMp => "50 MP",
            Self::HundredMp => "100 MP",
            Self::HundredMpIi => "100 MP II",
        }
    }

    fn noise_scale(self) -> f32 {
        match self {
            // The older 50 MP generation benefits from a little more read-
            // noise suppression; gen II needs the least intervention.
            Self::FiftyMp => 1.08,
            Self::HundredMp => 1.0,
            Self::HundredMpIi => 0.92,
        }
    }
}

/// Effective settings fed to ShopPhoto's mosaic- and RGB-domain denoisers.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CameraDenoiseTuning {
    pub luminance: f32,
    pub luminance_detail: f32,
    pub chroma: f32,
    pub chroma_detail: f32,
}

/// Return the allow-listed camera family for an EXIF model string.
#[must_use]
pub fn camera_denoise_family_for_model(model: &str) -> Option<CameraDenoiseFamily> {
    match sensor_for_model(model)?.label {
        "50MP" => Some(CameraDenoiseFamily::FiftyMp),
        "100MP" => Some(CameraDenoiseFamily::HundredMp),
        "100MP-II" => Some(CameraDenoiseFamily::HundredMpIi),
        _ => None,
    }
}

/// Camera- and ISO-conditioned parameters for ShopPhoto's edge-aware GPU
/// noise-reduction passes when the neural model is unavailable.
#[must_use]
pub fn camera_denoise_tuning(
    model: &str,
    iso: Option<u32>,
    mode: CameraDenoiseMode,
) -> Option<CameraDenoiseTuning> {
    if mode == CameraDenoiseMode::Off {
        return None;
    }
    let family = camera_denoise_family_for_model(model)?;
    // Work in stops so values between standard ISO markings interpolate
    // naturally. ISO 100..25600 maps to 0..1; lower/higher values clamp.
    let iso = iso.unwrap_or(100).max(50) as f32;
    let noise = ((iso / 100.0).log2() / 8.0).clamp(0.0, 1.0);
    let scale = family.noise_scale();
    let tuning = match mode {
        CameraDenoiseMode::Off => unreachable!(),
        CameraDenoiseMode::Cleanest => CameraDenoiseTuning {
            luminance: ((28.0 + 62.0 * noise) * scale).clamp(0.0, 100.0),
            luminance_detail: 34.0 - 14.0 * noise,
            chroma: ((34.0 + 58.0 * noise) * scale).clamp(0.0, 100.0),
            chroma_detail: 38.0 - 16.0 * noise,
        },
        CameraDenoiseMode::PreserveDetail => CameraDenoiseTuning {
            luminance: ((18.0 + 44.0 * noise) * scale).clamp(0.0, 100.0),
            luminance_detail: 82.0 - 10.0 * noise,
            chroma: ((27.0 + 48.0 * noise) * scale).clamp(0.0, 100.0),
            chroma_detail: 72.0 - 12.0 * noise,
        },
    };
    Some(tuning)
}

/// Find the sensor generation for a body model, ignoring case, spacing and
/// a leading maker name.
///
/// The 100 MP generations share geometry and are distinguished only by body,
/// so this matches on the model string rather than on dimensions.
#[must_use]
pub fn sensor_for_model(model: &str) -> Option<&'static SensorGeneration> {
    let key = normalize(model);
    if key.is_empty() {
        return None;
    }
    SENSORS
        .iter()
        .find(|s| s.bodies.iter().any(|b| normalize(b) == key))
}

fn normalize(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect();
    cleaned
        .strip_prefix("hasselblad")
        .map_or(cleaned.clone(), str::to_owned)
}

/// Grid layout for a fresh lens calibration on a full sensor readout of
/// `width`×`height`, at the default sample spacing.
#[must_use]
pub fn default_grid(width: u32, height: u32) -> GridGeometry {
    GridGeometry::for_sensor(width, height, DEFAULT_GRID_SPACING_PX)
}

/// Suggested clean-room capture temperatures for a multi-illuminant profile,
/// in Kelvin.
///
/// These are ordinary photographic/reference targets: CIE illuminant A,
/// warm-neutral LED, daylight/flash, and D65. They are not camera-maker
/// calibration nodes. A user should record the measured CCT of each actual
/// light and the fitter should interpolate in reciprocal temperature.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CalibrationIlluminants {
    pub low_tungsten_k: u32,
    pub tungsten_k: u32,
    pub flash_k: u32,
    pub high_k: u32,
}

impl CalibrationIlluminants {
    /// The four nodes, ascending.
    #[must_use]
    pub fn all(&self) -> [u32; 4] {
        [
            self.low_tungsten_k,
            self.tungsten_k,
            self.flash_k,
            self.high_k,
        ]
    }

    /// The node nearest `kelvin` — which calibration bucket a shot belongs
    /// to when sorting samples for a multi-illuminant fit.
    #[must_use]
    pub fn nearest(&self, kelvin: f32) -> u32 {
        self.all()
            .into_iter()
            .min_by(|a, b| {
                let da = (*a as f32 - kelvin).abs();
                let db = (*b as f32 - kelvin).abs();
                da.total_cmp(&db)
            })
            .expect("four nodes")
    }
}

/// Camera-independent reference targets for user-authored profiles.
pub const REFERENCE_ILLUMINANTS: CalibrationIlluminants = CalibrationIlluminants {
    low_tungsten_k: 2856,
    tungsten_k: 4000,
    flash_k: 5500,
    high_k: 6504,
};

/// Reference targets are intentionally independent of camera model.
#[must_use]
pub const fn illuminants_for_model(_model: &str) -> CalibrationIlluminants {
    REFERENCE_ILLUMINANTS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sensor_lookup_ignores_maker_and_spacing() {
        let s = sensor_for_model("Hasselblad X2D II 100C").expect("known body");
        assert_eq!(s.label, "100MP-II");
        assert_eq!(sensor_for_model("X2D II 100C"), Some(s));
        assert_eq!(
            sensor_for_model("X1D II 50C").map(|s| s.label),
            Some("50MP")
        );
        assert!(sensor_for_model("Canon EOS R5").is_none());
        assert!(sensor_for_model("").is_none());
    }

    #[test]
    fn sensor_active_size_matches_the_44x33_format() {
        let (w, h) = sensor_for_model("X2D 100C")
            .expect("known body")
            .active_size_mm();
        assert!((w - 43.83).abs() < 0.1, "width {w}");
        assert!((h - 32.87).abs() < 0.1, "height {h}");
    }

    #[test]
    fn the_two_100mp_generations_are_distinct_profiles_of_one_geometry() {
        let gen2 = sensor_for_model("X2D 100C").expect("x2d");
        let gen3 = sensor_for_model("X2D II 100C").expect("x2d ii");
        assert_ne!(gen2.label, gen3.label);
        assert_eq!(
            (gen2.width_px, gen2.height_px, gen2.pitch_nm),
            (gen3.width_px, gen3.height_px, gen3.pitch_nm)
        );
    }

    #[test]
    fn camera_denoise_support_is_an_explicit_allow_list() {
        assert_eq!(
            camera_denoise_family_for_model("Hasselblad X1D II 50C"),
            Some(CameraDenoiseFamily::FiftyMp)
        );
        assert_eq!(
            camera_denoise_family_for_model("CFV 100C"),
            Some(CameraDenoiseFamily::HundredMp)
        );
        assert_eq!(
            camera_denoise_family_for_model("X2D II 100C"),
            Some(CameraDenoiseFamily::HundredMpIi)
        );
        assert!(camera_denoise_family_for_model("Hasselblad H6D-100c").is_none());
        assert!(camera_denoise_family_for_model("Sony ILCE-1").is_none());
    }

    #[test]
    fn camera_denoise_tuning_tracks_iso_and_mode() {
        let low = camera_denoise_tuning("X2D 100C", Some(100), CameraDenoiseMode::Cleanest)
            .expect("supported");
        let high = camera_denoise_tuning("X2D 100C", Some(6400), CameraDenoiseMode::Cleanest)
            .expect("supported");
        let detail =
            camera_denoise_tuning("X2D 100C", Some(6400), CameraDenoiseMode::PreserveDetail)
                .expect("supported");
        assert!(high.luminance > low.luminance);
        assert!(high.chroma > low.chroma);
        assert!(high.luminance > detail.luminance);
        assert!(detail.luminance_detail > high.luminance_detail);
        assert!(
            camera_denoise_tuning("Canon EOS R5", Some(6400), CameraDenoiseMode::Cleanest)
                .is_none()
        );
        assert!(camera_denoise_tuning("X2D 100C", Some(6400), CameraDenoiseMode::Off).is_none());
    }

    #[test]
    fn default_grid_anchors_on_the_readout_centre() {
        let g = default_grid(11912, 8934);
        assert_eq!((g.center_x, g.center_y), (5956.0, 4467.0));
    }

    #[test]
    fn illuminant_targets_are_clean_room_and_camera_independent() {
        assert_eq!(illuminants_for_model("X2D II 100C"), REFERENCE_ILLUMINANTS);
        assert_eq!(illuminants_for_model("X1D II 50C"), REFERENCE_ILLUMINANTS);
        assert_eq!(illuminants_for_model("unknown"), REFERENCE_ILLUMINANTS);
    }

    #[test]
    fn nearest_illuminant_picks_the_calibration_bucket() {
        let i = REFERENCE_ILLUMINANTS;
        assert_eq!(i.nearest(2800.0), 2856);
        assert_eq!(i.nearest(5600.0), 5500);
        assert_eq!(i.nearest(20000.0), 6504);
        assert_eq!(i.nearest(0.0), 2856);
        assert_eq!(i.all(), [2856, 4000, 5500, 6504]);
    }
}
