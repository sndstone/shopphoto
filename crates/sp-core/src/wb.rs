//! White-balance color science: correlated color temperature → camera-space
//! gains (Phase 3).
//!
//! Pipeline use: when `EditState.wb.temp_kelvin` is `Some`, the renderer
//! derives WB gains from the requested illuminant instead of the as-shot
//! coefficients. The illuminant's chromaticity comes from the Kim et al.
//! cubic-spline approximation of the Planckian locus; tint shifts the `y`
//! chromaticity perpendicular-ish to the locus (+tint renders magenta by
//! assuming a greener illuminant, i.e. higher y),
//! an approximation of Duv that is smooth and monotonic — good enough for a
//! creative control, not a colorimeter.

use crate::color::{mat3_apply, mat3_inverse, Mat3};

/// Planckian locus xy chromaticity for a CCT in kelvin (Kim et al. cubic
/// approximation, valid 1667–25000 K; input is clamped).
#[must_use]
pub fn cct_to_xy(temp_kelvin: f32) -> (f32, f32) {
    let t = f64::from(temp_kelvin.clamp(1667.0, 25000.0));
    let t2 = t * t;
    let t3 = t2 * t;
    let x = if t <= 4000.0 {
        -0.266_123_9e9 / t3 - 0.234_358_9e6 / t2 + 0.877_695_6e3 / t + 0.179_910
    } else {
        -3.025_846_9e9 / t3 + 2.107_037_9e6 / t2 + 0.222_634_7e3 / t + 0.240_390
    };
    let x2 = x * x;
    let x3 = x2 * x;
    let y = if t <= 2222.0 {
        -1.106_381_4 * x3 - 1.348_110_2 * x2 + 2.185_558_32 * x - 0.202_196_83
    } else if t <= 4000.0 {
        -0.954_947_6 * x3 - 1.374_185_93 * x2 + 2.091_370_15 * x - 0.167_488_67
    } else {
        3.081_758_0 * x3 - 5.873_386_70 * x2 + 3.751_129_97 * x - 0.370_014_83
    };
    (x as f32, y as f32)
}

/// Approximate CCT for an xy chromaticity (McCamy 1992 cubic). Good to a
/// few tens of kelvin near the locus — used to pick the dual-illuminant
/// interpolation weight for as-shot white balance, not for display.
#[must_use]
pub fn cct_from_xy(x: f32, y: f32) -> f32 {
    let mut denom = f64::from(0.1858 - y);
    if denom.abs() < 1e-6 {
        denom = -1e-6; // typical photographic whites sit above the line
    }
    let n = f64::from(x - 0.3320) / denom;
    let cct = 449.0 * n.powi(3) + 3525.0 * n * n + 6823.3 * n + 5520.33;
    (cct as f32).clamp(1667.0, 25000.0)
}

/// Estimate the as-shot illuminant CCT from camera WB coefficients: the
/// illuminant white in camera space is `1 / gains`, mapped to XYZ via the
/// forward matrix.
#[must_use]
pub fn cct_from_wb_coeffs(cam_to_xyz: &Mat3, wb_coeffs: &[f32; 4]) -> f32 {
    let inv = |v: f32| if v > 1e-6 { 1.0 / v } else { 1.0 };
    let white_cam = [inv(wb_coeffs[0]), inv(wb_coeffs[1]), inv(wb_coeffs[2])];
    let xyz = mat3_apply(cam_to_xyz, white_cam);
    let sum = (xyz[0] + xyz[1] + xyz[2]).max(1e-6);
    cct_from_xy(xyz[0] / sum, xyz[1] / sum)
}

/// Camera-space WB gains for an explicit (temp, tint) pair, normalized so
/// the green gain is 1.0. `cam_to_xyz` is the camera's forward matrix.
/// Returns `None` if the camera matrix is singular (corrupt metadata).
#[must_use]
pub fn wb_gains_for_temp(cam_to_xyz: &Mat3, temp_kelvin: f32, tint: f32) -> Option<[f32; 3]> {
    let (x, mut y) = cct_to_xy(temp_kelvin);
    // Tint (Lightroom convention): positive = render MORE magenta, i.e.
    // assume a GREENER illuminant (higher y). ±150 UI units ≈ ±0.045 in y.
    y += tint * 3.0e-4;
    y = y.max(1e-3);

    // xy → XYZ with Y = 1.
    let white_xyz = [x / y, 1.0, (1.0 - x - y) / y];

    // Illuminant white in camera space; gains neutralize it.
    let xyz_to_cam = mat3_inverse(cam_to_xyz)?;
    let white_cam = mat3_apply(&xyz_to_cam, white_xyz);
    if white_cam.iter().any(|v| *v <= 1e-6) {
        return None;
    }
    Some([
        white_cam[1] / white_cam[0],
        1.0,
        white_cam[1] / white_cam[2],
    ])
}

/// The (temp, tint) pair that renders `camera_rgb` neutral: the white-balance
/// eyedropper. `camera_rgb` is a black-subtracted, un-white-balanced camera
/// sample of something that should be grey. Searches the same temperature
/// (2 000–50 000 K, log spaced) and tint (±150) ranges the sliders expose,
/// coarse to fine, minimising the log-ratio error between the gains that
/// pair produces and the gains that would neutralise the sample.
#[must_use]
pub fn temp_tint_for_neutral(cam_to_xyz: &Mat3, camera_rgb: [f32; 3]) -> Option<(f32, f32)> {
    if camera_rgb.iter().any(|v| !v.is_finite() || *v <= 1e-5) {
        return None;
    }
    let target = [
        (camera_rgb[1] / camera_rgb[0]).ln(),
        (camera_rgb[1] / camera_rgb[2]).ln(),
    ];
    let error = |temp: f32, tint: f32| -> f32 {
        wb_gains_for_temp(cam_to_xyz, temp, tint).map_or(f32::INFINITY, |gains| {
            (gains[0].ln() - target[0]).powi(2) + (gains[2].ln() - target[1]).powi(2)
        })
    };
    let (log_lo, log_hi) = (2_000f32.ln(), 50_000f32.ln());
    let (mut best_log_t, mut best_tint) = (5_500f32.ln(), 0.0f32);
    let mut best = error(5_500.0, 0.0);
    let (mut span_t, mut span_tint) = (log_hi - log_lo, 300.0f32);
    for _ in 0..6 {
        let (center_t, center_tint) = (best_log_t, best_tint);
        for i in 0..=16 {
            let log_t = (center_t + span_t * (i as f32 / 16.0 - 0.5)).clamp(log_lo, log_hi);
            for j in 0..=16 {
                let tint = (center_tint + span_tint * (j as f32 / 16.0 - 0.5)).clamp(-150.0, 150.0);
                let e = error(log_t.exp(), tint);
                if e < best {
                    best = e;
                    best_log_t = log_t;
                    best_tint = tint;
                }
            }
        }
        span_t *= 0.25;
        span_tint *= 0.25;
    }
    best.is_finite().then(|| (best_log_t.exp(), best_tint))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::REC2020_TO_XYZ;

    #[test]
    fn eyedropper_recovers_the_illuminant_it_was_lit_by() {
        // A grey patch lit by (4200 K, +20 tint) has camera values equal to
        // that illuminant's white; solving for neutral must return it.
        let (x, mut y) = cct_to_xy(4_200.0);
        y += 20.0 * 3.0e-4;
        let white_xyz = [x / y, 1.0, (1.0 - x - y) / y];
        let xyz_to_cam = mat3_inverse(&REC2020_TO_XYZ).unwrap();
        let camera = mat3_apply(&xyz_to_cam, white_xyz);
        let (temp, tint) = temp_tint_for_neutral(&REC2020_TO_XYZ, camera).unwrap();
        assert!((temp - 4_200.0).abs() < 40.0, "temp = {temp}");
        assert!((tint - 20.0).abs() < 2.0, "tint = {tint}");
        assert!(temp_tint_for_neutral(&REC2020_TO_XYZ, [0.0, 1.0, 1.0]).is_none());
    }

    #[test]
    fn cct_hits_known_chromaticities() {
        // 6504 K blackbody: x ≈ 0.3135, y ≈ 0.3237 (slightly below D65's
        // daylight-locus y = 0.329).
        let (x, y) = cct_to_xy(6504.0);
        assert!((x - 0.3135).abs() < 0.003, "x = {x}");
        assert!((y - 0.3237).abs() < 0.004, "y = {y}");
        // 2856 K (illuminant A): x ≈ 0.4476, y ≈ 0.4074.
        let (x, y) = cct_to_xy(2856.0);
        assert!((x - 0.4476).abs() < 0.005, "x = {x}");
        assert!((y - 0.4074).abs() < 0.006, "y = {y}");
    }

    #[test]
    fn mccamy_recovers_planckian_cct() {
        for t in [2856.0_f32, 4000.0, 5500.0, 6504.0, 9000.0] {
            let (x, y) = cct_to_xy(t);
            let got = cct_from_xy(x, y);
            assert!((got - t).abs() / t < 0.03, "cct {t}: recovered {got}");
        }
    }

    #[test]
    fn as_shot_cct_round_trip() {
        // Gains generated for 5000 K must estimate back near 5000 K.
        let gains = wb_gains_for_temp(&REC2020_TO_XYZ, 5000.0, 0.0).expect("gains");
        let coeffs = [gains[0], 1.0, gains[2], 1.0];
        let est = cct_from_wb_coeffs(&REC2020_TO_XYZ, &coeffs);
        assert!((est - 5000.0).abs() < 400.0, "estimated {est}");
    }

    #[test]
    fn neutral_camera_daylight_gains_are_near_one() {
        // Camera space = Rec.2020: at ~D65 the gains should be close to 1.
        let g = wb_gains_for_temp(&REC2020_TO_XYZ, 6504.0, 0.0).expect("gains");
        for v in g {
            assert!((v - 1.0).abs() < 0.1, "gains {g:?}");
        }
    }

    #[test]
    fn warm_light_boosts_blue_gain() {
        // Tungsten light is orange → neutralizing it needs more blue gain
        // and less red gain than daylight.
        let day = wb_gains_for_temp(&REC2020_TO_XYZ, 6504.0, 0.0).expect("day");
        let tung = wb_gains_for_temp(&REC2020_TO_XYZ, 3000.0, 0.0).expect("tungsten");
        assert!(tung[2] > day[2] * 1.5, "blue: {} vs {}", tung[2], day[2]);
        assert!(tung[0] < day[0], "red: {} vs {}", tung[0], day[0]);
    }

    #[test]
    fn tint_moves_green_magenta() {
        // Positive tint (magenta) must raise the green-neutralizing ratio
        // versus negative tint (green) monotonically.
        let magenta = wb_gains_for_temp(&REC2020_TO_XYZ, 5500.0, 100.0).expect("m");
        let green = wb_gains_for_temp(&REC2020_TO_XYZ, 5500.0, -100.0).expect("g");
        // With G pinned to 1, magenta shift raises R and B gains relative
        // to green shift.
        assert!(magenta[0] > green[0]);
        assert!(magenta[2] > green[2]);
    }
}
