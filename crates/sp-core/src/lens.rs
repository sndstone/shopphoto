//! Lens correction profiles: per-photosite shading (vignetting) grids.
//!
//! A profile stores multiplicative gain on a coarse grid anchored to the
//! lens's optical centre in full-sensor coordinates. Correction is applied
//! **pre-demosaic**, in the mosaic domain, because shading is a property of
//! individual photosites: each CFA quad slot gets its own gain, so a colour
//! cast that varies across the frame is corrected as such rather than being
//! averaged into a single luminance falloff.
//!
//! Gains are indexed by *quad slot* — `(y & 1) * 2 + (x & 1)` relative to the
//! full-sensor origin — not by colour. That keeps a profile independent of
//! the CFA pattern: a flat-field capture measures the four slots directly and
//! the decoder's pattern never enters the calibration.
//!
//! Vignetting is aperture-dependent, so a profile carries one grid per
//! calibrated f-number and interpolates between the bracketing pair.
//!
//! Nothing here ships with data. Grids come from the user's own flat-field
//! captures via the calibration workflow; the built-in
//! [`crate::hasselblad`] tables supply only sensor geometry so a fresh
//! profile starts with the right optical centre and spacing.

use serde::{Deserialize, Serialize};

/// Number of gain values per grid cell: one per 2×2 CFA quad slot.
pub const QUAD_SLOTS: usize = 4;

/// Grid sample spacing used by new calibrations, in sensor pixels.
///
/// Lens shading is a very low-frequency field, so a 100-pixel grid retains
/// ample spatial resolution while keeping calibration files compact and
/// interpolation inexpensive. This is a ShopPhoto calibration choice; no
/// third-party correction data is embedded or required.
pub const DEFAULT_GRID_SPACING_PX: f32 = 100.0;

/// Points sampled along a line that is straight in the photographed scene.
/// Coordinates are normalized around the optical centre: the shorter image
/// half-axis is one unit. A calibration normally contains several points on
/// each edge of a checkerboard or architectural line and at least one line
/// near every side of the frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StraightLineSample {
    pub points: Vec<[f32; 2]>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DistortionFitError {
    #[error("distortion calibration needs at least two lines with three points each")]
    NotEnoughLines,
    #[error("distortion calibration points must be finite and inside the normalized frame")]
    InvalidPoint,
    #[error("distortion calibration lines are too short or degenerate")]
    DegenerateLine,
}

fn corrected_point(point: [f32; 2], coefficients: [f32; 3]) -> [f32; 2] {
    let radius2 = point[0] * point[0] + point[1] * point[1];
    let radius4 = radius2 * radius2;
    let scale = 1.0
        + coefficients[0] * radius2
        + coefficients[1] * radius4
        + coefficients[2] * radius4 * radius2;
    [point[0] * scale, point[1] * scale]
}

fn straightness_error(lines: &[StraightLineSample], coefficients: [f32; 3]) -> f64 {
    let mut error = 0.0f64;
    let mut count = 0usize;
    for line in lines {
        let corrected: Vec<_> = line
            .points
            .iter()
            .copied()
            .map(|point| corrected_point(point, coefficients))
            .collect();
        let center = corrected.iter().fold([0.0f64; 2], |mut sum, point| {
            sum[0] += f64::from(point[0]);
            sum[1] += f64::from(point[1]);
            sum
        });
        let center = [
            center[0] / corrected.len() as f64,
            center[1] / corrected.len() as f64,
        ];
        let (xx, xy, yy) = corrected.iter().fold((0.0, 0.0, 0.0), |sum, point| {
            let x = f64::from(point[0]) - center[0];
            let y = f64::from(point[1]) - center[1];
            (sum.0 + x * x, sum.1 + x * y, sum.2 + y * y)
        });
        let angle = 0.5 * (2.0 * xy).atan2(xx - yy);
        let normal = [-angle.sin(), angle.cos()];
        for point in corrected {
            let dx = f64::from(point[0]) - center[0];
            let dy = f64::from(point[1]) - center[1];
            let distance = dx * normal[0] + dy * normal[1];
            error += distance * distance;
            count += 1;
        }
    }
    let regularization = coefficients
        .into_iter()
        .enumerate()
        .map(|(index, value)| f64::from(value * value) * [1e-5, 2e-5, 4e-5][index])
        .sum::<f64>();
    error / count.max(1) as f64 + regularization
}

/// Fit clean-room Brown–Conrady radial coefficients from user-marked lines.
///
/// This is a plumb-line calibration: the optimizer searches for the radial
/// remap that minimizes the orthogonal distance of corrected samples from
/// each line's own best-fit axis. It uses no maker profile data and is fully
/// deterministic, which makes a saved `.splens` portable and reproducible.
pub fn fit_radial_distortion(lines: &[StraightLineSample]) -> Result<[f32; 3], DistortionFitError> {
    let usable: Vec<_> = lines
        .iter()
        .filter(|line| line.points.len() >= 3)
        .cloned()
        .collect();
    if usable.len() < 2 {
        return Err(DistortionFitError::NotEnoughLines);
    }
    for line in &usable {
        if line.points.iter().any(|point| {
            !point[0].is_finite()
                || !point[1].is_finite()
                || point[0].abs() > 2.0
                || point[1].abs() > 2.0
        }) {
            return Err(DistortionFitError::InvalidPoint);
        }
        let first = line.points[0];
        if line
            .points
            .iter()
            .all(|point| (point[0] - first[0]).hypot(point[1] - first[1]) < 1e-4)
        {
            return Err(DistortionFitError::DegenerateLine);
        }
    }

    let mut coefficients = [0.0f32; 3];
    let mut steps = [0.05f32, 0.02, 0.01];
    let mut best = straightness_error(&usable, coefficients);
    for _ in 0..90 {
        let mut improved = false;
        for axis in 0..3 {
            for direction in [-1.0f32, 1.0] {
                let mut candidate = coefficients;
                candidate[axis] = (candidate[axis] + steps[axis] * direction).clamp(-0.75, 0.75);
                let score = straightness_error(&usable, candidate);
                if score + 1e-14 < best {
                    coefficients = candidate;
                    best = score;
                    improved = true;
                }
            }
        }
        if !improved {
            for step in &mut steps {
                *step *= 0.55;
            }
        }
        if steps.iter().all(|step| *step < 2e-6) {
            break;
        }
    }
    Ok(coefficients)
}

/// One calibrated aperture: a full gain grid measured at `f_number`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShadingGrid {
    /// The f-number this grid was measured at.
    pub f_number: f32,
    /// `grid_w * grid_h` cells in row-major order, each holding four
    /// multiplicative gains indexed by quad slot. 1.0 = no correction.
    pub gains: Vec<[f32; QUAD_SLOTS]>,
}

/// A lens shading profile for one lens (and, for zooms, one focal length).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LensProfile {
    /// Display name, e.g. `"XCD 90mm f/3.2"`.
    pub name: String,
    /// Normalized match key derived from EXIF `LensModel` — see
    /// [`lens_key`].
    pub key: String,
    /// Focal length in mm for zoom lenses; `None` for primes, which match
    /// on `key` alone.
    #[serde(default)]
    pub focal_mm: Option<f32>,
    /// Grid dimensions in cells. Both must be ≥ 2.
    pub grid_w: u32,
    pub grid_h: u32,
    /// Optical centre in full-sensor pixel coordinates. Grid cell
    /// `((grid_w - 1) / 2, (grid_h - 1) / 2)` sits here.
    pub center_x: f32,
    pub center_y: f32,
    /// Grid sample spacing in sensor pixels.
    pub spacing_px: f32,
    /// Calibrated apertures, sorted ascending by `f_number`. At least one.
    pub apertures: Vec<ShadingGrid>,
    /// Clean-room Brown-Conrady radial distortion coefficients.
    #[serde(default)]
    pub distortion: [f32; 3],
    /// Red/blue radial scale deltas relative to green.
    #[serde(default)]
    pub lateral_ca: [f32; 2],
}

/// Normalize a lens name into a stable match key: lowercase, alphanumerics
/// only, with a leading `hasselblad` maker prefix dropped.
///
/// Hasselblad append the zoom position to the model itself — a shot on the
/// 35-100 at 60 mm records `"XCD 35-100E@60"` — so anything from `@`
/// onward is stripped first. Without that, every focal length would key as
/// a different lens and a zoom could never match one profile. The position
/// is recovered separately by [`zoom_position_mm`]; the shot's EXIF focal
/// length says the same thing.
///
/// `"Hasselblad XCD 90mm f/3.2"` and `"XCD90"` key to `"xcd90mmf32"` and
/// `"xcd90"` respectively — close but not equal, so matching is done by
/// [`LensProfile::matches`], which accepts either as a prefix of the other.
#[must_use]
pub fn lens_key(model: &str) -> String {
    let base = model.split('@').next().unwrap_or(model);
    let cleaned: String = base
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect();
    cleaned
        .strip_prefix("hasselblad")
        .map_or(cleaned.clone(), str::to_owned)
}

/// The zoom position Hasselblad encoded in a lens name, in mm.
///
/// `"XCD 35-100E@60"` → `Some(60.0)`; a prime or an unadorned name →
/// `None`.
#[must_use]
pub fn zoom_position_mm(model: &str) -> Option<f32> {
    let tail = model.split('@').nth(1)?;
    let digits: String = tail
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    digits.parse().ok().filter(|v: &f32| *v > 0.0)
}

impl LensProfile {
    /// Cells in one grid.
    #[must_use]
    pub fn cell_count(&self) -> usize {
        (self.grid_w as usize) * (self.grid_h as usize)
    }

    /// Whether every grid has the expected length and the profile is
    /// structurally usable.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.grid_w >= 2
            && self.grid_h >= 2
            && self.spacing_px > 0.0
            && !self.apertures.is_empty()
            && self.distortion.iter().all(|value| value.is_finite())
            && self.lateral_ca.iter().all(|value| value.is_finite())
            && self
                .apertures
                .iter()
                .all(|a| a.gains.len() == self.cell_count() && a.f_number > 0.0)
    }

    /// Whether this profile applies to a shot taken with `model` at
    /// `focal_mm`.
    ///
    /// Names arrive from EXIF in inconsistent forms (`"XCD 90"`, `"XCD90"`,
    /// `"Hasselblad XCD 3,2/90"`), so keys match when either is a prefix of
    /// the other. Zoom profiles additionally require the focal length to be
    /// within 5% of the calibrated one — taken from `focal_mm`, or from the
    /// `@` position in the name when EXIF carries no focal length.
    #[must_use]
    pub fn matches(&self, model: &str, focal_mm: Option<f32>) -> bool {
        let shot = lens_key(model);
        if shot.is_empty() || self.key.is_empty() {
            return false;
        }
        if !shot.starts_with(&self.key) && !self.key.starts_with(&shot) {
            return false;
        }
        match (self.focal_mm, focal_mm.or_else(|| zoom_position_mm(model))) {
            (Some(want), Some(got)) => (got - want).abs() <= want * 0.05,
            (Some(_), None) => false,
            (None, _) => true,
        }
    }

    /// Gains for `f_number`, interpolating between the bracketing calibrated
    /// apertures.
    ///
    /// Vignetting closes up roughly with the log of the aperture ratio, so
    /// the blend runs in log-f space rather than linear f-number. Outside
    /// the calibrated range the nearest grid is used unchanged — extrapolated
    /// shading is worse than slightly stale shading.
    #[must_use]
    pub fn gains_for_aperture(&self, f_number: Option<f32>) -> Vec<[f32; QUAD_SLOTS]> {
        debug_assert!(self.is_valid());
        let first = &self.apertures[0];
        let Some(f) = f_number.filter(|f| *f > 0.0) else {
            return first.gains.clone();
        };
        if self.apertures.len() == 1 || f <= first.f_number {
            return first.gains.clone();
        }
        let last = self.apertures.last().expect("non-empty");
        if f >= last.f_number {
            return last.gains.clone();
        }
        let upper = self
            .apertures
            .iter()
            .position(|a| a.f_number >= f)
            .unwrap_or(self.apertures.len() - 1)
            .max(1);
        let lo = &self.apertures[upper - 1];
        let hi = &self.apertures[upper];
        let span = (hi.f_number / lo.f_number).ln();
        let t = if span.abs() < 1e-6 {
            0.0
        } else {
            ((f / lo.f_number).ln() / span).clamp(0.0, 1.0)
        };
        lo.gains
            .iter()
            .zip(&hi.gains)
            .map(|(a, b)| {
                let mut out = [1.0; QUAD_SLOTS];
                for slot in 0..QUAD_SLOTS {
                    out[slot] = a[slot] + (b[slot] - a[slot]) * t;
                }
                out
            })
            .collect()
    }

    /// An identity profile of the given shape — every gain 1.0. The starting
    /// point for a calibration, and what the renderer falls back to.
    #[must_use]
    pub fn identity(name: impl Into<String>, key: impl Into<String>, geom: GridGeometry) -> Self {
        Self {
            name: name.into(),
            key: key.into(),
            focal_mm: None,
            grid_w: geom.grid_w,
            grid_h: geom.grid_h,
            center_x: geom.center_x,
            center_y: geom.center_y,
            spacing_px: geom.spacing_px,
            apertures: vec![ShadingGrid {
                f_number: 8.0,
                gains: vec![[1.0; QUAD_SLOTS]; (geom.grid_w * geom.grid_h) as usize],
            }],
            distortion: [0.0; 3],
            lateral_ca: [0.0; 2],
        }
    }
}

/// The grid layout a profile uses, derived from sensor geometry.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GridGeometry {
    pub grid_w: u32,
    pub grid_h: u32,
    pub center_x: f32,
    pub center_y: f32,
    pub spacing_px: f32,
}

impl GridGeometry {
    /// Cover a `width`×`height` sensor with samples every `spacing_px`,
    /// centred on the optical axis.
    ///
    /// The cell count is forced odd so a cell lands exactly on the optical
    /// centre, which is where gain is 1.0 by definition and where a
    /// calibration is most trustworthy.
    #[must_use]
    pub fn for_sensor(width: u32, height: u32, spacing_px: f32) -> Self {
        let cells = |extent: u32| {
            let n = ((extent as f32 / spacing_px).ceil() as u32).max(2);
            if n.is_multiple_of(2) {
                n + 1
            } else {
                n
            }
        };
        Self {
            grid_w: cells(width),
            grid_h: cells(height),
            center_x: width as f32 / 2.0,
            center_y: height as f32 / 2.0,
            spacing_px,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn distort(corrected: [f32; 2], coefficients: [f32; 3]) -> [f32; 2] {
        let target_radius = corrected[0].hypot(corrected[1]);
        if target_radius < 1e-8 {
            return corrected;
        }
        let mut radius = target_radius;
        for _ in 0..12 {
            let r2 = radius * radius;
            let r4 = r2 * r2;
            let scale =
                1.0 + coefficients[0] * r2 + coefficients[1] * r4 + coefficients[2] * r4 * r2;
            let derivative = 1.0
                + 3.0 * coefficients[0] * r2
                + 5.0 * coefficients[1] * r4
                + 7.0 * coefficients[2] * r4 * r2;
            radius -= (radius * scale - target_radius) / derivative;
        }
        let scale = radius / target_radius;
        [corrected[0] * scale, corrected[1] * scale]
    }

    fn grid(f_number: f32, value: f32) -> ShadingGrid {
        ShadingGrid {
            f_number,
            gains: vec![[value; QUAD_SLOTS]; 9],
        }
    }

    fn profile(apertures: Vec<ShadingGrid>) -> LensProfile {
        LensProfile {
            name: "XCD 90mm".into(),
            key: "xcd90".into(),
            focal_mm: None,
            grid_w: 3,
            grid_h: 3,
            center_x: 100.0,
            center_y: 100.0,
            spacing_px: 100.0,
            apertures,
            distortion: [0.0; 3],
            lateral_ca: [0.0; 2],
        }
    }

    #[test]
    fn plumb_line_fit_recovers_radial_distortion() {
        let expected = [0.12, -0.03, 0.01];
        let positions = [-0.9f32, -0.6, -0.3, 0.0, 0.3, 0.6, 0.9];
        let lines = vec![
            StraightLineSample {
                points: positions
                    .iter()
                    .map(|value| distort([-0.82, *value], expected))
                    .collect(),
            },
            StraightLineSample {
                points: positions
                    .iter()
                    .map(|value| distort([0.82, *value], expected))
                    .collect(),
            },
            StraightLineSample {
                points: positions
                    .iter()
                    .map(|value| distort([*value, -0.72], expected))
                    .collect(),
            },
            StraightLineSample {
                points: positions
                    .iter()
                    .map(|value| distort([*value, 0.72], expected))
                    .collect(),
            },
        ];
        let fitted = fit_radial_distortion(&lines).unwrap();
        assert!(
            straightness_error(&lines, fitted) < straightness_error(&lines, [0.0; 3]) * 0.002,
            "fitted {fitted:?}"
        );
        // Individual polynomial terms are correlated; what must be recovered
        // is the radial scale over the photographed field.
        for radius2 in [0.5f32, 1.0, 1.5] {
            let scale = |coefficients: [f32; 3]| {
                1.0 + coefficients[0] * radius2
                    + coefficients[1] * radius2.powi(2)
                    + coefficients[2] * radius2.powi(3)
            };
            assert!(
                (scale(fitted) - scale(expected)).abs() < 0.015,
                "fitted {fitted:?}"
            );
        }
    }

    #[test]
    fn plumb_line_fit_rejects_insufficient_or_invalid_samples() {
        assert_eq!(
            fit_radial_distortion(&[]),
            Err(DistortionFitError::NotEnoughLines)
        );
        let invalid = StraightLineSample {
            points: vec![[0.0, 0.0], [f32::NAN, 0.0], [1.0, 0.0]],
        };
        assert_eq!(
            fit_radial_distortion(&[invalid.clone(), invalid]),
            Err(DistortionFitError::InvalidPoint)
        );
    }

    #[test]
    fn key_strips_maker_and_punctuation() {
        assert_eq!(lens_key("Hasselblad XCD 90mm f/3.2"), "xcd90mmf32");
        assert_eq!(lens_key("XCD90"), "xcd90");
        assert_eq!(lens_key(""), "");
    }

    #[test]
    fn zoom_position_does_not_leak_into_the_key() {
        // Real strings from X2D II files: without stripping the `@` suffix,
        // every focal length of one zoom would key as a different lens.
        assert_eq!(lens_key("Hasselblad XCD 35-100E@60"), "xcd35100e");
        assert_eq!(lens_key("Hasselblad XCD 35-100E@42"), "xcd35100e");
        assert_eq!(lens_key("Hasselblad XCD 28P"), "xcd28p");
        assert_eq!(zoom_position_mm("Hasselblad XCD 35-100E@60"), Some(60.0));
        assert_eq!(zoom_position_mm("Hasselblad XCD 20-35E@24.5"), Some(24.5));
        assert_eq!(zoom_position_mm("Hasselblad XCD 28P"), None);
        assert_eq!(zoom_position_mm("XCD 35-100E@"), None);
    }

    #[test]
    fn a_zoom_profile_matches_the_position_in_the_name() {
        let geom = GridGeometry::for_sensor(400, 300, 100.0);
        let mut p = LensProfile::identity("XCD 35-100E @ 60", "xcd35100e", geom);
        p.focal_mm = Some(60.0);
        // EXIF focal length is the primary source...
        assert!(p.matches("Hasselblad XCD 35-100E@60", Some(60.0)));
        assert!(!p.matches("Hasselblad XCD 35-100E@42", Some(42.0)));
        // ...but the name's own position stands in when it is missing.
        assert!(p.matches("Hasselblad XCD 35-100E@60", None));
        assert!(!p.matches("Hasselblad XCD 35-100E@42", None));
    }

    #[test]
    fn matching_accepts_either_name_as_prefix() {
        let p = profile(vec![grid(3.2, 1.0)]);
        assert!(p.matches("XCD90", None));
        assert!(p.matches("Hasselblad XCD 90mm f/3.2", None));
        assert!(!p.matches("XCD 120mm", None));
        assert!(!p.matches("", None));
    }

    #[test]
    fn zoom_profiles_require_matching_focal_length() {
        let mut p = profile(vec![grid(4.0, 1.0)]);
        p.key = "xcd2035".into();
        p.focal_mm = Some(24.0);
        assert!(p.matches("XCD2035", Some(24.0)));
        assert!(p.matches("XCD2035", Some(24.5)), "within 5%");
        assert!(!p.matches("XCD2035", Some(35.0)));
        assert!(
            !p.matches("XCD2035", None),
            "a zoom profile cannot match without a focal length"
        );
    }

    #[test]
    fn single_aperture_profile_is_used_everywhere() {
        let p = profile(vec![grid(5.6, 1.4)]);
        for f in [None, Some(2.8), Some(5.6), Some(22.0)] {
            assert_eq!(p.gains_for_aperture(f)[0][0], 1.4);
        }
    }

    #[test]
    fn aperture_interpolation_is_log_spaced_and_clamped() {
        let p = profile(vec![grid(4.0, 2.0), grid(16.0, 1.0)]);
        // Below/above the calibrated range: nearest grid, never extrapolated.
        assert_eq!(p.gains_for_aperture(Some(2.0))[0][0], 2.0);
        assert_eq!(p.gains_for_aperture(Some(32.0))[0][0], 1.0);
        // f/8 is the log-midpoint of f/4..f/16, so exactly halfway.
        let mid = p.gains_for_aperture(Some(8.0))[0][0];
        assert!((mid - 1.5).abs() < 1e-5, "expected 1.5, got {mid}");
    }

    #[test]
    fn missing_aperture_falls_back_to_the_widest_grid() {
        let p = profile(vec![grid(4.0, 2.0), grid(16.0, 1.0)]);
        assert_eq!(p.gains_for_aperture(None)[0][0], 2.0);
    }

    #[test]
    fn identity_profile_is_valid_and_neutral() {
        let geom = GridGeometry::for_sensor(11656, 8742, DEFAULT_GRID_SPACING_PX);
        let p = LensProfile::identity("XCD 55V", "xcd55v", geom);
        assert!(p.is_valid());
        assert!(p.apertures[0].gains.iter().all(|c| c == &[1.0; QUAD_SLOTS]));
    }

    #[test]
    fn sensor_grid_has_an_odd_cell_count_centred_on_the_axis() {
        let geom = GridGeometry::for_sensor(11656, 8742, 100.0);
        assert_eq!(geom.grid_w % 2, 1);
        assert_eq!(geom.grid_h % 2, 1);
        assert!(geom.grid_w >= 117 && geom.grid_h >= 88);
        assert_eq!(geom.center_x, 5828.0);
    }

    #[test]
    fn invalid_profiles_are_rejected() {
        let mut p = profile(vec![grid(4.0, 1.0)]);
        assert!(p.is_valid());
        p.apertures[0].gains.pop();
        assert!(!p.is_valid(), "grid length must match the cell count");
        assert!(!profile(vec![]).is_valid(), "at least one aperture");
    }
}
