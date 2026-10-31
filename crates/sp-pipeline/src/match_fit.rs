//! Reference Match fitting core (BUILD.md Phase 3B step 5, Tier 3).
//!
//! The offline fitter consumes paired scene-linear Rec.2020 samples. It
//! reserves a deterministic 20% holdout, fits a 3×3 least-squares matrix,
//! a monotonic encoded-domain tone curve, a luma-invariant chroma-plane
//! warp, then a smooth 33³ residual RGB LUT that mops up what the warp
//! cannot express. Quality is reported as real CIEDE2000 on both fit and
//! holdout sets.

use crate::tone::sample_curve;
use sp_core::color::{
    mat3_apply, mat3_inverse, mat3_mul, srgb_eotf, srgb_oetf, Mat3, REC2020_TO_XYZ, XYZ_TO_REC2020,
};
use sp_core::edit::{CurvePoint, ToneCurve};
use sp_core::profile::{
    CameraProfile, ChromaWarp, Lut3d, CHROMA_NEUTRAL, CHROMA_WARP_GRAY, CHROMA_WARP_SIZE,
};

/// Shared with the chroma-warp shader. The fit and the GPU must agree on
/// luma exactly, or the fitted warp lands on the wrong grid nodes.
use sp_core::color::REC2020_LUMA;

const TONE_POINTS: usize = 17;
const MATCH_LUT_SIZE: u32 = 33;

/// One neutral-ShopPhoto / target-reference scene-linear color pair.
pub type ColorPair = ([f32; 3], [f32; 3]);

/// Residual-error report in CIEDE2000 units.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct FitReport {
    pub mean_delta_e_00: f32,
    pub p95_delta_e_00: f32,
    pub samples: usize,
}

/// Complete result of the matrix → chroma warp → tone → 3-D LUT fit.
#[derive(Debug, Clone)]
pub struct MatchFit {
    pub matrix: Mat3,
    /// Luma-invariant chroma rendering, applied with the matrix.
    pub chroma_warp: ChromaWarp,
    pub tone_curve: ToneCurve,
    pub rgb_lut: Lut3d,
    pub training: FitReport,
    pub holdout: FitReport,
}

fn rec2020_luma(rgb: [f32; 3]) -> f32 {
    rgb[0] * REC2020_LUMA[0] + rgb[1] * REC2020_LUMA[1] + rgb[2] * REC2020_LUMA[2]
}

/// Fit M minimizing Σ‖M·ours − target‖². Returns `None` with fewer than 9
/// well-conditioned samples.
#[must_use]
pub fn fit_matrix(pairs: &[ColorPair]) -> Option<Mat3> {
    if pairs.len() < 9 {
        return None;
    }
    let mut aat = [[0.0f64; 3]; 3];
    let mut bat = [[0.0f64; 3]; 3];
    for (a, b) in pairs {
        for r in 0..3 {
            for c in 0..3 {
                aat[r][c] += f64::from(a[r]) * f64::from(a[c]);
                bat[r][c] += f64::from(b[r]) * f64::from(a[c]);
            }
        }
    }
    let aat: Mat3 = std::array::from_fn(|r| std::array::from_fn(|c| aat[r][c] as f32));
    let bat: Mat3 = std::array::from_fn(|r| std::array::from_fn(|c| bat[r][c] as f32));
    Some(mat3_mul(&bat, &mat3_inverse(&aat)?))
}

fn fit_tone_curve(matrix: &Mat3, pairs: &[ColorPair]) -> ToneCurve {
    let mut sum = [0.0f64; TONE_POINTS];
    let mut weight = [0.0f64; TONE_POINTS];
    for &(ours, target) in pairs {
        let x = srgb_oetf(rec2020_luma(mat3_apply(matrix, ours)).clamp(0.0, 1.0));
        let y = srgb_oetf(rec2020_luma(target).clamp(0.0, 1.0));
        let f = x * (TONE_POINTS - 1) as f32;
        let i0 = (f.floor() as usize).min(TONE_POINTS - 1);
        let i1 = (i0 + 1).min(TONE_POINTS - 1);
        let t = f - i0 as f32;
        sum[i0] += f64::from(y * (1.0 - t));
        weight[i0] += f64::from(1.0 - t);
        sum[i1] += f64::from(y * t);
        weight[i1] += f64::from(t);
    }
    let mut y = [0.0f32; TONE_POINTS];
    for i in 0..TONE_POINTS {
        y[i] = if weight[i] > 0.0 {
            (sum[i] / weight[i]) as f32
        } else {
            i as f32 / (TONE_POINTS - 1) as f32
        };
    }
    // Fill unsupported bins from their nearest supported neighbors.
    for i in 0..TONE_POINTS {
        if weight[i] == 0.0 {
            let left = (0..i).rev().find(|&j| weight[j] > 0.0);
            let right = (i + 1..TONE_POINTS).find(|&j| weight[j] > 0.0);
            y[i] = match (left, right) {
                (Some(a), Some(b)) => {
                    let t = (i - a) as f32 / (b - a) as f32;
                    y[a] * (1.0 - t) + y[b] * t
                }
                (Some(a), None) => y[a],
                (None, Some(b)) => y[b],
                (None, None) => i as f32 / (TONE_POINTS - 1) as f32,
            };
        }
    }
    y[0] = 0.0;
    for i in 1..TONE_POINTS {
        y[i] = y[i].max(y[i - 1]).clamp(0.0, 1.0);
    }
    ToneCurve {
        points: y
            .into_iter()
            .enumerate()
            .map(|(i, y)| CurvePoint {
                x: i as f32 / (TONE_POINTS - 1) as f32,
                y,
            })
            .collect(),
    }
}

fn apply_curve(curve: &ToneCurve, rgb: [f32; 3]) -> [f32; 3] {
    rgb.map(|v| {
        if v <= 1.0 {
            srgb_eotf(sample_curve(curve, srgb_oetf(v.max(0.0))))
        } else {
            v * srgb_eotf(sample_curve(curve, 1.0)).max(1e-4)
        }
    })
}

/// Split linear Rec.2020 RGB into luma and its normalized-RGB chromaticity
/// `(n_r, n_b)`. `None` for pixels too dark to have a meaningful
/// chromaticity.
fn to_chroma(rgb: [f32; 3]) -> Option<(f32, [f32; 2])> {
    let y = rec2020_luma(rgb);
    let sum = rgb[0] + rgb[1] + rgb[2];
    if y <= 1e-5 || sum <= 1e-5 {
        return None;
    }
    Some((y, [rgb[0] / sum, rgb[2] / sum]))
}

/// Rebuild RGB from a luma and a chromaticity, rescaling the simplex point
/// to hit `y` exactly. Mirrors `wb_color.wgsl`.
fn from_chroma(y: f32, c: [f32; 2]) -> Option<[f32; 3]> {
    let n = [c[0], 1.0 - c[0] - c[1], c[1]];
    let n_luma = rec2020_luma(n);
    if n_luma <= 1e-4 {
        return None;
    }
    let scale = y / n_luma;
    Some(n.map(|v| v * scale))
}

fn smoothstep(edge: f32, x: f32) -> f32 {
    let t = (x / edge.max(1e-5)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Distance of a chromaticity from the neutral point.
fn chroma_saturation(c: [f32; 2]) -> f32 {
    let dr = c[0] - CHROMA_NEUTRAL;
    let db = c[1] - CHROMA_NEUTRAL;
    (dr * dr + db * db).sqrt()
}

/// CPU mirror of the chroma-warp stage, so the fit optimizes what the GPU
/// will actually run. `tests/lens_and_color.rs` cross-checks the two on
/// grid conventions, and `sp-gpu`'s `chroma_warp_luma_matches_sp_core`
/// pins the shared luma coefficients.
#[must_use]
pub fn apply_chroma_warp(warp: &ChromaWarp, rgb: [f32; 3]) -> [f32; 3] {
    if !warp.is_valid() {
        return rgb;
    }
    let Some((y, c)) = to_chroma(rgb) else {
        return rgb;
    };
    let looked_up = warp.sample(c[0], c[1]);
    let fade = smoothstep(warp.gray_threshold, chroma_saturation(c));
    let warped = [
        c[0] + (looked_up[0] - c[0]) * fade,
        c[1] + (looked_up[1] - c[1]) * fade,
    ];
    from_chroma(y, warped).unwrap_or(rgb)
}

/// Fit the chroma-plane warp from matrix-corrected samples.
///
/// Structurally the same estimator as [`fit_residual_lut`] — bilinear splat
/// of measured residuals, then Laplacian diffusion with measured nodes
/// anchored — but in two dimensions over chromaticity instead of three over
/// RGB. That is the whole point: because the coordinates are luma-normalized,
/// every node is spent on hue and saturation, and a correction learned from
/// a midtone patch applies unchanged in shadow and highlight.
///
/// Near-neutral samples are skipped. Their chroma is mostly sensor noise,
/// the shader fades correction out there anyway, and including them would
/// drag the whole grid toward whatever the noise averaged to.
fn fit_chroma_warp(matrix: &Mat3, pairs: &[ColorPair]) -> ChromaWarp {
    let size = CHROMA_WARP_SIZE;
    let len = (size * size) as usize;
    let idx = |r: u32, b: u32| (b * size + r) as usize;
    let node = |i: u32| i as f32 / (size - 1) as f32;

    let mut sums = vec![[0.0f32; 2]; len];
    let mut weights = vec![0.0f32; len];
    for &(ours, target) in pairs {
        let (Some((_, input)), Some((_, want))) =
            (to_chroma(mat3_apply(matrix, ours)), to_chroma(target))
        else {
            continue;
        };
        if chroma_saturation(input) < CHROMA_WARP_GRAY {
            continue;
        }
        let f = input.map(|v| (v * (size - 1) as f32).clamp(0.0, (size - 1) as f32));
        let lo = [f[0].floor() as u32, f[1].floor() as u32];
        let hi = [(lo[0] + 1).min(size - 1), (lo[1] + 1).min(size - 1)];
        let t = [f[0] - lo[0] as f32, f[1] - lo[1] as f32];
        let residual = [want[0] - input[0], want[1] - input[1]];
        for (r, wr) in [(lo[0], 1.0 - t[0]), (hi[0], t[0])] {
            for (b, wb) in [(lo[1], 1.0 - t[1]), (hi[1], t[1])] {
                let w = wr * wb;
                let i = idx(r, b);
                sums[i][0] += residual[0] * w;
                sums[i][1] += residual[1] * w;
                weights[i] += w;
            }
        }
    }

    let mut residual = vec![[0.0f32; 2]; len];
    for i in 0..len {
        if weights[i] > 1e-6 {
            residual[i] = sums[i].map(|v| v / weights[i]);
        }
    }
    for _ in 0..24 {
        let old = residual.clone();
        for b in 0..size {
            for r in 0..size {
                let i = idx(r, b);
                let mut avg = [0.0f32; 2];
                let mut n = 0.0f32;
                for (nr, nb) in [
                    (r.saturating_sub(1), b),
                    ((r + 1).min(size - 1), b),
                    (r, b.saturating_sub(1)),
                    (r, (b + 1).min(size - 1)),
                ] {
                    if (nr, nb) != (r, b) {
                        let v = old[idx(nr, nb)];
                        avg[0] += v[0];
                        avg[1] += v[1];
                        n += 1.0;
                    }
                }
                for value in &mut avg {
                    *value /= n.max(1.0);
                }
                let anchor = if weights[i] > 1e-6 { 0.85 } else { 0.0 };
                residual[i] = std::array::from_fn(|c| {
                    let smooth = old[i][c] * 0.55 + avg[c] * 0.45;
                    smooth * (1.0 - anchor) + (sums[i][c] / weights[i].max(1e-6)) * anchor
                });
            }
        }
    }

    let mut entries = Vec::with_capacity(len);
    for b in 0..size {
        for r in 0..size {
            let d = residual[idx(r, b)];
            // Keep nodes on the simplex: r + g + b = 1 with all parts
            // non-negative, so reconstruction always yields a real colour.
            let nr = (node(r) + d[0]).clamp(0.0, 1.0);
            let nb = (node(b) + d[1]).clamp(0.0, 1.0 - nr);
            entries.push([nr, nb]);
        }
    }
    ChromaWarp {
        size_r: size,
        size_b: size,
        r_min: 0.0,
        r_max: 1.0,
        b_min: 0.0,
        b_max: 1.0,
        entries,
        gray_threshold: CHROMA_WARP_GRAY,
    }
}

fn fit_residual_lut(
    matrix: &Mat3,
    warp: &ChromaWarp,
    curve: &ToneCurve,
    pairs: &[ColorPair],
) -> Lut3d {
    let size = MATCH_LUT_SIZE;
    let len = size.pow(3) as usize;
    let mut sums = vec![[0.0f32; 3]; len];
    let mut weights = vec![0.0f32; len];
    let idx = |r: u32, g: u32, b: u32| ((r * size + g) * size + b) as usize;
    for &(ours, target) in pairs {
        let input = apply_curve(curve, apply_chroma_warp(warp, mat3_apply(matrix, ours)))
            .map(|v| v.clamp(0.0, 1.0));
        let f = input.map(|v| v * (size - 1) as f32);
        let lo = f.map(|v| v.floor() as u32);
        let hi = lo.map(|v| (v + 1).min(size - 1));
        let t = [
            f[0] - lo[0] as f32,
            f[1] - lo[1] as f32,
            f[2] - lo[2] as f32,
        ];
        let residual = std::array::from_fn::<_, 3, _>(|c| target[c] - input[c]);
        for (r, wr) in [(lo[0], 1.0 - t[0]), (hi[0], t[0])] {
            for (g, wg) in [(lo[1], 1.0 - t[1]), (hi[1], t[1])] {
                for (b, wb) in [(lo[2], 1.0 - t[2]), (hi[2], t[2])] {
                    let w = wr * wg * wb;
                    let i = idx(r, g, b);
                    for c in 0..3 {
                        sums[i][c] += residual[c] * w;
                    }
                    weights[i] += w;
                }
            }
        }
    }
    let mut residual = vec![[0.0f32; 3]; len];
    for i in 0..len {
        if weights[i] > 1e-6 {
            residual[i] = sums[i].map(|v| v / weights[i]);
        }
    }
    // Laplacian regularization. Measured nodes remain anchored; empty
    // nodes diffuse nearby corrections rather than producing hard cells.
    for _ in 0..24 {
        let old = residual.clone();
        for r in 0..size {
            for g in 0..size {
                for b in 0..size {
                    let i = idx(r, g, b);
                    let mut avg = [0.0f32; 3];
                    let mut n = 0.0f32;
                    for (rr, gg, bb) in [
                        (r.saturating_sub(1), g, b),
                        ((r + 1).min(size - 1), g, b),
                        (r, g.saturating_sub(1), b),
                        (r, (g + 1).min(size - 1), b),
                        (r, g, b.saturating_sub(1)),
                        (r, g, (b + 1).min(size - 1)),
                    ] {
                        if (rr, gg, bb) != (r, g, b) {
                            let v = old[idx(rr, gg, bb)];
                            for c in 0..3 {
                                avg[c] += v[c];
                            }
                            n += 1.0;
                        }
                    }
                    for value in &mut avg {
                        *value /= n.max(1.0);
                    }
                    let anchor = if weights[i] > 1e-6 { 0.85 } else { 0.0 };
                    residual[i] = std::array::from_fn(|c| {
                        let smooth = old[i][c] * 0.55 + avg[c] * 0.45;
                        smooth * (1.0 - anchor) + (sums[i][c] / weights[i].max(1e-6)) * anchor
                    });
                }
            }
        }
    }
    let denom = (size - 1) as f32;
    let mut entries = Vec::with_capacity(len);
    for r in 0..size {
        for g in 0..size {
            for b in 0..size {
                let base = [r as f32 / denom, g as f32 / denom, b as f32 / denom];
                let d = residual[idx(r, g, b)];
                entries.push(std::array::from_fn(|c| (base[c] + d[c]).clamp(0.0, 1.5)));
            }
        }
    }
    Lut3d { size, entries }
}

/// Apply a fitted correction in pipeline order: matrix and chroma warp
/// (both in `wb_color`), then the tone curve, then the residual RGB LUT.
#[must_use]
pub fn apply_fit(fit: &MatchFit, rgb: [f32; 3]) -> [f32; 3] {
    let rendered = apply_chroma_warp(&fit.chroma_warp, mat3_apply(&fit.matrix, rgb));
    fit.rgb_lut.sample(apply_curve(&fit.tone_curve, rendered))
}

fn xyz_to_lab(rgb: [f32; 3]) -> [f32; 3] {
    let xyz = mat3_apply(&REC2020_TO_XYZ, rgb.map(|v| v.max(0.0)));
    let white = [0.95047, 1.0, 1.08883];
    let f = |v: f32| {
        let v = v.max(0.0);
        if v > 216.0 / 24389.0 {
            v.cbrt()
        } else {
            (24389.0 / 27.0 * v + 16.0) / 116.0
        }
    };
    let fx = f(xyz[0] / white[0]);
    let fy = f(xyz[1] / white[1]);
    let fz = f(xyz[2] / white[2]);
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

// Sharma et al. 2005 reference equation.
fn delta_e_00(a: [f32; 3], b: [f32; 3]) -> f32 {
    let (l1, a1, b1) = (a[0] as f64, a[1] as f64, a[2] as f64);
    let (l2, a2, b2) = (b[0] as f64, b[1] as f64, b[2] as f64);
    let c1 = a1.hypot(b1);
    let c2 = a2.hypot(b2);
    let cbar = (c1 + c2) * 0.5;
    let g = 0.5 * (1.0 - (cbar.powi(7) / (cbar.powi(7) + 25f64.powi(7))).sqrt());
    let ap1 = (1.0 + g) * a1;
    let ap2 = (1.0 + g) * a2;
    let cp1 = ap1.hypot(b1);
    let cp2 = ap2.hypot(b2);
    let hp = |bb: f64, aa: f64| {
        let h = bb.atan2(aa).to_degrees();
        if h < 0.0 {
            h + 360.0
        } else {
            h
        }
    };
    let hp1 = hp(b1, ap1);
    let hp2 = hp(b2, ap2);
    let dl = l2 - l1;
    let dc = cp2 - cp1;
    let dh_deg = if cp1 * cp2 == 0.0 {
        0.0
    } else if (hp2 - hp1).abs() <= 180.0 {
        hp2 - hp1
    } else if hp2 <= hp1 {
        hp2 - hp1 + 360.0
    } else {
        hp2 - hp1 - 360.0
    };
    let dh = 2.0 * (cp1 * cp2).sqrt() * (dh_deg.to_radians() * 0.5).sin();
    let lbar = (l1 + l2) * 0.5;
    let cpbar = (cp1 + cp2) * 0.5;
    let hpbar = if cp1 * cp2 == 0.0 {
        hp1 + hp2
    } else if (hp1 - hp2).abs() <= 180.0 {
        (hp1 + hp2) * 0.5
    } else if hp1 + hp2 < 360.0 {
        (hp1 + hp2 + 360.0) * 0.5
    } else {
        (hp1 + hp2 - 360.0) * 0.5
    };
    let t = 1.0 - 0.17 * (hpbar - 30.0).to_radians().cos()
        + 0.24 * (2.0 * hpbar).to_radians().cos()
        + 0.32 * (3.0 * hpbar + 6.0).to_radians().cos()
        - 0.20 * (4.0 * hpbar - 63.0).to_radians().cos();
    let sl = 1.0 + 0.015 * (lbar - 50.0).powi(2) / (20.0 + (lbar - 50.0).powi(2)).sqrt();
    let sc = 1.0 + 0.045 * cpbar;
    let sh = 1.0 + 0.015 * cpbar * t;
    let rt = -2.0
        * (cpbar.powi(7) / (cpbar.powi(7) + 25f64.powi(7))).sqrt()
        * (60.0 * (-((hpbar - 275.0) / 25.0).powi(2)).exp())
            .to_radians()
            .sin();
    let (dl, dc, dh) = (dl / sl, dc / sc, dh / sh);
    (dl * dl + dc * dc + dh * dh + rt * dc * dh).sqrt() as f32
}

fn report_with(pairs: &[ColorPair], apply: impl Fn([f32; 3]) -> [f32; 3]) -> FitReport {
    let mut errors: Vec<f32> = pairs
        .iter()
        .map(|&(ours, target)| delta_e_00(xyz_to_lab(apply(ours)), xyz_to_lab(target)))
        .collect();
    errors.sort_by(f32::total_cmp);
    FitReport {
        mean_delta_e_00: errors.iter().sum::<f32>() / errors.len().max(1) as f32,
        p95_delta_e_00: errors
            .get(((errors.len() as f32 * 0.95) as usize).min(errors.len().saturating_sub(1)))
            .copied()
            .unwrap_or(0.0),
        samples: errors.len(),
    }
}

/// Fit all Tier-3 components. Every fifth sample is reserved for holdout.
#[must_use]
pub fn fit_match(pairs: &[ColorPair]) -> Option<MatchFit> {
    if pairs.len() < 50 {
        return None;
    }
    let is_holdout = |i: usize| {
        let mut x = i as u32 + 0x9e37_79b9;
        x ^= x >> 16;
        x = x.wrapping_mul(0x7feb_352d);
        x ^= x >> 15;
        x = x.wrapping_mul(0x846c_a68b);
        (x ^ (x >> 16)).is_multiple_of(5)
    };
    let training: Vec<_> = pairs
        .iter()
        .enumerate()
        .filter(|(i, _)| !is_holdout(*i))
        .map(|(_, &p)| p)
        .collect();
    let holdout: Vec<_> = pairs
        .iter()
        .enumerate()
        .filter(|(i, _)| is_holdout(*i))
        .map(|(_, &p)| p)
        .collect();
    let matrix = fit_matrix(&training)?;
    // The warp preserves luma by construction, so it cannot move what the
    // tone curve should be — the curve is fitted first and stays valid.
    let tone_curve = fit_tone_curve(&matrix, &training);
    let chroma_warp = fit_chroma_warp(&matrix, &training);
    // The RGB LUT mops up whatever the warp could not express. It is fitted
    // on the warp's output, so the two compose rather than compete.
    let rgb_lut = fit_residual_lut(&matrix, &chroma_warp, &tone_curve, &training);
    let mut fit = MatchFit {
        matrix,
        chroma_warp,
        tone_curve,
        rgb_lut,
        training: FitReport::default(),
        holdout: FitReport::default(),
    };
    fit.training = report_with(&training, |rgb| apply_fit(&fit, rgb));
    fit.holdout = report_with(&holdout, |rgb| apply_fit(&fit, rgb));
    Some(fit)
}

/// Build a selectable Tier-3 profile from a completed fit.
#[must_use]
pub fn make_match_profile(
    name: impl Into<String>,
    camera_model: impl Into<String>,
    embedded_cam_to_xyz: &Mat3,
    fit: &MatchFit,
) -> CameraProfile {
    let to_xyz = mat3_mul(
        &REC2020_TO_XYZ,
        &mat3_mul(&fit.matrix, &mat3_mul(&XYZ_TO_REC2020, embedded_cam_to_xyz)),
    );
    let mut profile = CameraProfile::standard(camera_model, to_xyz);
    profile.name = name.into();
    profile.base_curve = Some(fit.tone_curve.clone());
    profile.rgb_lut = Some(fit.rgb_lut.clone());
    profile.chroma_warp1 = Some(fit.chroma_warp.clone());
    profile
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_colors(n: usize) -> Vec<[f32; 3]> {
        let mut out = Vec::with_capacity(n * n * n);
        for r in 0..n {
            for g in 0..n {
                for b in 0..n {
                    out.push([
                        0.02 + r as f32 / (n - 1) as f32 * 0.9,
                        0.02 + g as f32 / (n - 1) as f32 * 0.9,
                        0.02 + b as f32 / (n - 1) as f32 * 0.9,
                    ]);
                }
            }
        }
        out
    }

    #[test]
    fn ciede2000_reference_pair() {
        let got = delta_e_00([50.0, 2.6772, -79.7751], [50.0, 0.0, -82.7485]);
        assert!((got - 2.0425).abs() < 0.0002, "{got}");
    }

    #[test]
    fn full_fit_recovers_synthetic_matrix_and_smooth_look() {
        let truth: Mat3 = [
            [1.035, 0.018, -0.012],
            [-0.015, 0.985, 0.020],
            [0.008, -0.025, 0.955],
        ];
        let pairs: Vec<_> = sample_colors(13)
            .into_iter()
            .map(|c| {
                let mut target = mat3_apply(&truth, c);
                let l = rec2020_luma(target).clamp(0.0, 1.0);
                target = target.map(|v| (v.max(0.0).powf(0.97) + 0.012 * l * (1.0 - l)).min(1.0));
                (c, target)
            })
            .collect();
        let fit = fit_match(&pairs).expect("fit");
        assert!(fit.holdout.mean_delta_e_00 < 0.5, "{:?}", fit.holdout);
        assert_eq!(fit.rgb_lut.size, 33);
    }

    #[test]
    fn match_profile_composes_fit_matrix() {
        let pairs: Vec<_> = sample_colors(5).into_iter().map(|c| (c, c)).collect();
        let fit = fit_match(&pairs).expect("fit");
        let profile = make_match_profile("Reference Match", "cam", &REC2020_TO_XYZ, &fit);
        assert_eq!(profile.name, "Reference Match");
        assert_eq!(profile.rgb_lut.as_ref().map(|l| l.size), Some(33));
    }

    #[test]
    fn too_few_samples_rejected() {
        assert!(fit_match(&vec![([0.5; 3], [0.5; 3]); 49]).is_none());
        assert!(fit_matrix(&[([0.5; 3], [0.5; 3]); 8]).is_none());
    }
}
