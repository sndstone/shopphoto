//! Analytic tone LUT (BUILD.md Phase 3 step 2).
//!
//! Exposure, contrast, highlights, shadows, whites, blacks and the user
//! tone curve compress into ONE 1-D LUT rebuilt on the CPU per parameter
//! change and applied per channel in `tone_color.wgsl`. Six sliders cost
//! one texture-free buffer lookup on the GPU.
//!
//! Domain: scene-linear input x is index-mapped through the shaper
//! `s(x) = x / (x + 1)` (bijective [0,∞) → [0,1)), so the LUT covers the
//! whole HDR range with resolution concentrated where tones live. The i-th
//! entry stores the OUTPUT linear value for input `x_i = t/(1-t)`,
//! `t = i / (N-1)` (capped below 1). The shader mirrors the shaper.
//!
//! Invariants (tested): neutral params ⇒ identity within float error;
//! output is monotonic (enforced) and non-negative.

use sp_core::color::{srgb_eotf, srgb_oetf, REC2020_LUMA};
use sp_core::edit::{CurveParams, ParametricCurve, ToneCurve, ToneParams};

/// Number of LUT entries; must match `tone_color.wgsl`.
pub const TONE_LUT_SIZE: usize = 1024;

/// Highest representable input (shaper cap). ~64 = +6 stops over white.
const MAX_INPUT: f32 = 64.0;

fn shaper(x: f32) -> f32 {
    x / (x + 1.0)
}

fn shaper_inv(t: f32) -> f32 {
    (t / (1.0 - t)).min(MAX_INPUT)
}

/// Smoothstep helper.
fn sstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Evaluate the user Catmull-Rom curve at encoded position `e` ∈ [0,1].
/// Points are sorted by x with endpoints present; identity when neutral.
fn eval_curve(curve: &ToneCurve, e: f32) -> f32 {
    let pts = &curve.points;
    if pts.len() < 2 {
        return e;
    }
    let e = e.clamp(0.0, 1.0);
    // Find segment.
    let mut seg = 0;
    while seg + 2 < pts.len() && pts[seg + 1].x <= e {
        seg += 1;
    }
    let p1 = pts[seg];
    let p2 = pts[seg + 1];
    if (p2.x - p1.x).abs() < 1e-6 {
        return p2.y;
    }
    // Neighbors for tangents (clamped at ends).
    let p0 = if seg == 0 { p1 } else { pts[seg - 1] };
    let p3 = if seg + 2 < pts.len() {
        pts[seg + 2]
    } else {
        p2
    };
    let t = (e - p1.x) / (p2.x - p1.x);

    // Catmull-Rom with non-uniform x handled via finite-difference tangents
    // (Fritsch-Carlson-ish scaling keeps overshoot mild; monotonicity is
    // enforced globally after LUT build anyway).
    let d1 = (p2.y - p0.y) / (p2.x - p0.x).max(1e-6) * (p2.x - p1.x);
    let d2 = (p3.y - p1.y) / (p3.x - p1.x).max(1e-6) * (p2.x - p1.x);
    let t2 = t * t;
    let t3 = t2 * t;
    let h00 = 2.0 * t3 - 3.0 * t2 + 1.0;
    let h10 = t3 - 2.0 * t2 + t;
    let h01 = -2.0 * t3 + 3.0 * t2;
    let h11 = t3 - t2;
    (h00 * p1.y + h10 * d1 + h01 * p2.y + h11 * d2).clamp(0.0, 1.0)
}

/// Apply the parametric tone controls to one linear value.
fn tone_map(p: &ToneParams, x: f32) -> f32 {
    const MID: f32 = 0.18;
    let mut y = x * 2f32.powf(p.exposure_ev);

    // Contrast: power curve around mid-gray in log space, ±100 → ×0.55..1.45.
    if p.contrast != 0.0 && y > 0.0 {
        let k = 1.0 + p.contrast / 100.0 * 0.45;
        y = MID * (y / MID).powf(k);
    }

    // Encoded-domain gamma pins black and SDR white. Preserve the linear
    // offset above 1.0 so changing brightness does not consume HDR headroom.
    if p.brightness != 0.0 {
        let base = y.min(1.0);
        let encoded = srgb_oetf(base);
        let gamma = 2f32.powf(-p.brightness / 100.0 * 0.75);
        y = srgb_eotf(encoded.powf(gamma)) + (y - base);
    }

    // Region masks computed on the encoded (perceptual) axis.
    let e = srgb_oetf(y.min(1.0));
    let shadow_mask = 1.0 - sstep(0.0, 0.55, e);
    let highlight_mask = sstep(0.45, 1.0, e);

    // Highlights/shadows: up to ±0.9 EV in their regions.
    if p.shadows != 0.0 {
        y *= 2f32.powf(p.shadows / 100.0 * 0.9 * shadow_mask);
    }
    if p.highlights != 0.0 {
        y *= 2f32.powf(p.highlights / 100.0 * 0.9 * highlight_mask);
    }

    // Whites: scale the white point (±25%). Blacks: lift/crush the floor.
    if p.whites != 0.0 {
        y *= 1.0 + p.whites / 100.0 * 0.25 * highlight_mask.max(0.3);
    }
    if p.blacks != 0.0 {
        let lift = p.blacks / 100.0 * 0.06;
        let e2 = srgb_oetf(y.min(1.0));
        let m = 1.0 - sstep(0.0, 0.35, e2);
        y = (srgb_eotf((e2 + lift * m).clamp(0.0, 1.0))).max(0.0) + (y - y.min(1.0));
    }

    y.max(0.0)
}

fn raised_cosine(e: f32, center: f32, half_width: f32) -> f32 {
    let distance = (e - center).abs();
    if distance >= half_width {
        0.0
    } else {
        0.5 + 0.5 * (std::f32::consts::PI * distance / half_width).cos()
    }
}

/// Apply the four parametric-curve regions in encoded space.
fn apply_parametric(parametric: &ParametricCurve, encoded: f32) -> f32 {
    let regions = [
        (0.85, parametric.highlights),
        (0.62, parametric.lights),
        (0.38, parametric.darks),
        (0.15, parametric.shadows),
    ];
    let delta = regions.into_iter().fold(0.0, |sum, (center, amount)| {
        sum + raised_cosine(encoded, center, 0.35) * amount / 100.0 * 0.12
    });
    (encoded + delta).clamp(0.0, 1.0)
}

/// Gentle default tone curve for the Tier-1 "ShopPhoto Standard" render:
/// a mild filmic lift so neutral files read as a finished photograph
/// instead of a flat linear ramp (cameras' embedded previews and every
/// commercial processor bake in a comparable base curve). Profiles with
/// their own base curve replace it entirely.
#[must_use]
pub fn standard_base_curve() -> ToneCurve {
    use sp_core::edit::CurvePoint;
    ToneCurve {
        points: vec![
            CurvePoint { x: 0.0, y: 0.0 },
            CurvePoint { x: 0.05, y: 0.06 },
            CurvePoint { x: 0.15, y: 0.21 },
            CurvePoint { x: 0.35, y: 0.46 },
            CurvePoint { x: 0.60, y: 0.72 },
            CurvePoint { x: 0.85, y: 0.91 },
            CurvePoint { x: 1.0, y: 1.0 },
        ],
    }
}

/// Public curve sampling for UI drawing (encoded domain, 0..1 both axes).
#[must_use]
pub fn sample_curve(curve: &ToneCurve, e: f32) -> f32 {
    eval_curve(curve, e)
}

/// Build the combined LUT: parametric controls, then the camera profile's
/// base curve (Phase 3B, DCP ProfileToneCurve), then the user curve — all
/// in the encoded domain.
#[must_use]
pub fn build_tone_lut(
    params: &ToneParams,
    curve: &CurveParams,
    base_curve: Option<&ToneCurve>,
) -> Vec<f32> {
    let n = TONE_LUT_SIZE;
    let mut lut = Vec::with_capacity(n);
    let curve_is_neutral = curve.luma.is_neutral();
    let base = base_curve.filter(|c| !c.is_neutral() && c.points.len() >= 2);
    for i in 0..n {
        let t = i as f32 / (n - 1) as f32 * shaper(MAX_INPUT);
        let x = shaper_inv(t);
        let mut y = tone_map(params, x);

        if y <= 1.0 {
            y = srgb_eotf(apply_parametric(&curve.parametric, srgb_oetf(y)));
        }

        if let Some(bc) = base {
            if y <= 1.0 {
                y = srgb_eotf(eval_curve(bc, srgb_oetf(y)));
            } else {
                y *= srgb_eotf(eval_curve(bc, 1.0)).max(1e-4);
            }
        }

        if !curve_is_neutral {
            if y <= 1.0 {
                let e = srgb_oetf(y);
                y = srgb_eotf(eval_curve(&curve.luma, e));
            } else {
                // Above SDR white: scale by the curve's white-point gain so
                // headroom follows the curve endpoint smoothly.
                let white_gain = srgb_eotf(eval_curve(&curve.luma, 1.0)).max(1e-4);
                y *= white_gain;
            }
        }
        lut.push(y);
    }
    // Enforce monotonicity (guards against extreme curve/slider combos).
    for i in 1..n {
        if lut[i] < lut[i - 1] {
            lut[i] = lut[i - 1];
        }
    }
    lut
}

/// Entries per encoded-domain channel curve; must match tone_color.wgsl.
pub const CHANNEL_LUT_SIZE: usize = 256;

/// Build R, G, and B encoded curve tables, packed consecutively.
#[must_use]
pub fn build_channel_lut(curves: &CurveParams) -> Vec<f32> {
    [&curves.red, &curves.green, &curves.blue]
        .into_iter()
        .flat_map(|curve| {
            (0..CHANNEL_LUT_SIZE)
                .map(move |index| eval_curve(curve, index as f32 / (CHANNEL_LUT_SIZE - 1) as f32))
        })
        .collect()
}

/// CPU mirror of the per-channel encoded curve stage.
#[must_use]
pub fn apply_channel_lut(lut: &[f32], rgb: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|channel| {
        let x = rgb[channel];
        let base = x.clamp(0.0, 1.0);
        let position = srgb_oetf(base) * (CHANNEL_LUT_SIZE - 1) as f32;
        let index = (position.floor() as usize).min(CHANNEL_LUT_SIZE - 2);
        let fraction = position - index as f32;
        let offset = channel * CHANNEL_LUT_SIZE;
        let encoded = lut[offset + index] * (1.0 - fraction) + lut[offset + index + 1] * fraction;
        srgb_eotf(encoded) + (x - base)
    })
}

/// CPU mirror of the output shader's radial vignette gain, including its
/// highlight-protection fade.
#[must_use]
pub fn vignette_gain(params: &sp_core::edit::VignetteParams, uv: [f32; 2], luminance: f32) -> f32 {
    if params.amount == 0.0 {
        return 1.0;
    }
    let x = (uv[0] - 0.5) * 2.0;
    let y = (uv[1] - 0.5) * 2.0;
    let circular = (x * x + y * y).sqrt() / std::f32::consts::SQRT_2;
    let rectangular = x.abs().max(y.abs());
    let radius = rectangular + (circular - rectangular) * params.roundness / 100.0;
    let midpoint = 0.15 + (0.8 - 0.15) * params.midpoint / 100.0;
    let edge = midpoint + ((1.0 - midpoint) * params.feather / 100.0).max(0.02);
    let t = ((radius - midpoint) / (edge - midpoint)).clamp(0.0, 1.0);
    let mask = t * t * (3.0 - 2.0 * t);
    let gain = 2f32.powf(params.amount / 100.0 * 1.5 * mask);
    let highlight_priority = 1.0 / (1.0 + (luminance - 1.0).max(0.0) * 2.0);
    1.0 + (gain - 1.0) * highlight_priority
}

/// CPU mirror of the local-contrast shader's Oklab-L adjustment before RGB
/// reconstruction. Used for edge/overshoot guards.
#[must_use]
pub fn local_contrast_l(
    params: &sp_core::edit::LocalContrastParams,
    center_l: f32,
    small_blur_l: f32,
    large_blur_l: f32,
    edge_strength: f32,
    encoded_luma: f32,
) -> f32 {
    let texture_mask = 1.0 - sstep(0.035, 0.12, edge_strength);
    let mut adjusted =
        center_l + (center_l - small_blur_l) * params.texture / 100.0 * 0.6 * texture_mask;
    let midtone = 1.0 - (2.0 * encoded_luma - 1.0).abs().powf(1.5);
    adjusted += (center_l - large_blur_l) * params.clarity / 100.0 * 0.75 * midtone;
    adjusted.clamp(0.0, 1.1f32.powf(1.0 / 3.0))
}

/// CPU-side LUT application (mirrors the shader; used by tests/reference).
#[must_use]
pub fn apply_tone_lut(lut: &[f32], x: f32) -> f32 {
    let n = lut.len();
    let t = (shaper(x.max(0.0)) / shaper(MAX_INPUT)).clamp(0.0, 1.0) * (n - 1) as f32;
    let i = (t as usize).min(n - 2);
    let frac = t - i as f32;
    lut[i] * (1.0 - frac) + lut[i + 1] * frac
}

/// CPU reference for the HDR shoulder duplicated in the output shaders.
/// Below the knee it is exactly linear; above it the curve has unit slope at
/// the join and approaches the requested headroom without reaching a hard
/// clip. A hyperbolic tangent keeps substantially more separation in real
/// highlights than the former `u / (1 + u)` shoulder (which mapped a value
/// at the selected ceiling to only half of the available highlight span).
/// `rolloff` is the UI's 0..100 softness value.
#[must_use]
pub fn hdr_shoulder_luminance(luminance: f32, headroom_stops: f32, rolloff: f32) -> f32 {
    let ceiling = 2f32.powf(headroom_stops.clamp(0.0, 4.0));
    if ceiling <= 1.001 {
        return luminance.clamp(0.0, 1.0);
    }
    let softness = (rolloff / 100.0).clamp(0.0, 1.0);
    // Reference white is an anchor, not part of the effect. Even the softest
    // setting starts its bend only slightly below it; Apple EDR likewise
    // reserves values above 1.0 for emissive/specular content.
    let knee = 1.0 + (0.82 - 1.0) * softness;
    if luminance <= knee {
        return luminance.max(0.0);
    }
    let span = (ceiling - knee).max(1e-4);
    // Leave a tiny amount of display headroom unused so even extreme input
    // cannot numerically become a flat patch of peak-white pixels.
    let curved_span = span * 0.999;
    let u = (luminance - knee).max(0.0) / curved_span;
    knee + curved_span * u.tanh()
}

/// CPU reference for the HDR-to-SDR luminance shoulder in the output
/// shaders. Input and output are scene-linear Rec.2020; the later gamut
/// transform and peak clamp are output-space operations.
#[must_use]
pub fn hdr_to_sdr_working_rgb(rgb: [f32; 3], rolloff: f32) -> [f32; 3] {
    let rgb = rgb.map(|channel| channel.max(0.0));
    let luminance = rgb
        .iter()
        .zip(REC2020_LUMA)
        .map(|(channel, weight)| channel * weight)
        .sum::<f32>();
    let softness = (rolloff / 100.0).clamp(0.0, 1.0);
    let knee = 0.90 + (0.72 - 0.90) * softness;
    if luminance <= knee {
        return rgb;
    }
    let span = (1.0 - knee).max(1e-4);
    let mapped = knee + span * ((luminance - knee) / span).tanh();
    let scale = mapped / luminance;
    rgb.map(|channel| channel * scale)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sp_core::edit::{CurvePoint, ToneCurve};

    #[test]
    fn neutral_lut_is_identity() {
        let lut = build_tone_lut(&ToneParams::default(), &CurveParams::default(), None);
        for x in [0.0, 0.01, 0.18, 0.5, 1.0, 4.0, 20.0] {
            let y = apply_tone_lut(&lut, x);
            let tol = (x * 0.02).max(0.002);
            assert!((y - x).abs() < tol, "identity broke at {x}: {y}");
        }
    }

    #[test]
    fn exposure_doubles() {
        let p = ToneParams {
            exposure_ev: 1.0,
            ..Default::default()
        };
        let lut = build_tone_lut(&p, &CurveParams::default(), None);
        let y = apply_tone_lut(&lut, 0.2);
        assert!((y - 0.4).abs() < 0.01, "got {y}");
    }

    #[test]
    fn contrast_steepens_around_mid() {
        let p = ToneParams {
            contrast: 60.0,
            ..Default::default()
        };
        let lut = build_tone_lut(&p, &CurveParams::default(), None);
        assert!(apply_tone_lut(&lut, 0.05) < 0.05);
        assert!(apply_tone_lut(&lut, 0.5) > 0.5);
        let mid = apply_tone_lut(&lut, 0.18);
        assert!((mid - 0.18).abs() < 0.01, "mid moved: {mid}");
    }

    #[test]
    fn shadows_lift_only_shadows() {
        let p = ToneParams {
            shadows: 80.0,
            ..Default::default()
        };
        let lut = build_tone_lut(&p, &CurveParams::default(), None);
        assert!(apply_tone_lut(&lut, 0.02) > 0.025);
        let hi = apply_tone_lut(&lut, 0.8);
        assert!((hi - 0.8).abs() < 0.02, "highlights moved: {hi}");
    }

    #[test]
    fn highlights_recover_only_highlights() {
        let p = ToneParams {
            highlights: -80.0,
            ..Default::default()
        };
        let lut = build_tone_lut(&p, &CurveParams::default(), None);
        assert!(apply_tone_lut(&lut, 0.8) < 0.75);
        let lo = apply_tone_lut(&lut, 0.05);
        assert!((lo - 0.05).abs() < 0.01, "shadows moved: {lo}");
    }

    #[test]
    fn curve_applies_in_encoded_domain() {
        // An S-curve point pulling the quarter-tone down.
        let curve = ToneCurve {
            points: vec![
                CurvePoint { x: 0.0, y: 0.0 },
                CurvePoint { x: 0.25, y: 0.15 },
                CurvePoint { x: 1.0, y: 1.0 },
            ],
        };
        let curves = CurveParams {
            luma: curve,
            ..Default::default()
        };
        let lut = build_tone_lut(&ToneParams::default(), &curves, None);
        // Encoded 0.25 ↔ linear ~0.0508; result must be darker.
        let y = apply_tone_lut(&lut, 0.0508);
        assert!(y < 0.04, "curve had no effect: {y}");
    }

    #[test]
    fn base_curve_applies_before_user_curve() {
        let base = ToneCurve {
            points: vec![
                CurvePoint { x: 0.0, y: 0.0 },
                CurvePoint { x: 0.5, y: 0.65 },
                CurvePoint { x: 1.0, y: 1.0 },
            ],
        };
        let lut = build_tone_lut(&ToneParams::default(), &CurveParams::default(), Some(&base));
        // Encoded 0.5 ↔ linear ~0.214: base curve lifts it.
        let y = apply_tone_lut(&lut, 0.214);
        assert!(y > 0.28, "base curve had no effect: {y}");
    }

    #[test]
    fn always_monotonic() {
        let p = ToneParams {
            contrast: 100.0,
            blacks: -100.0,
            whites: 100.0,
            shadows: 100.0,
            highlights: -100.0,
            ..Default::default()
        };
        let lut = build_tone_lut(&p, &CurveParams::default(), None);
        for w in lut.windows(2) {
            assert!(w[1] >= w[0]);
        }
    }

    #[test]
    fn brightness_pins_endpoints_and_lifts_midtones() {
        let params = ToneParams {
            brightness: 100.0,
            ..Default::default()
        };
        let lut = build_tone_lut(&params, &CurveParams::default(), None);
        let mid = apply_tone_lut(&lut, srgb_eotf(0.5));
        assert!(srgb_oetf(mid) > 0.62);
        assert!(apply_tone_lut(&lut, 0.0).abs() < 1e-3);
        assert!((apply_tone_lut(&lut, 1.0) - 1.0).abs() < 1e-3);
    }

    #[test]
    fn red_curve_only_changes_red() {
        let mut curves = CurveParams::default();
        curves.red.points.insert(1, CurvePoint { x: 0.5, y: 0.3 });
        let lut = build_channel_lut(&curves);
        let input = [srgb_eotf(0.5); 3];
        let output = apply_channel_lut(&lut, input);
        assert!(output[0] < input[0]);
        assert!((output[1] - input[1]).abs() < 1e-5);
        assert!((output[2] - input[2]).abs() < 1e-5);
    }

    #[test]
    fn parametric_extremes_remain_monotonic() {
        let curves = CurveParams {
            parametric: ParametricCurve {
                highlights: -100.0,
                lights: 100.0,
                darks: -100.0,
                shadows: 100.0,
            },
            ..Default::default()
        };
        let lut = build_tone_lut(&ToneParams::default(), &curves, None);
        assert!(lut.windows(2).all(|window| window[1] >= window[0]));
    }

    #[test]
    fn vignette_is_neutral_at_center_and_reaches_corner_gain() {
        let params = sp_core::edit::VignetteParams {
            amount: -100.0,
            ..Default::default()
        };
        assert!((vignette_gain(&params, [0.5, 0.5], 0.5) - 1.0).abs() < 1e-6);
        let dark_corner = vignette_gain(&params, [0.0, 0.0], 0.5);
        assert!((dark_corner - 2f32.powf(-1.5)).abs() < 1e-4);
        let bright_corner = vignette_gain(&params, [0.0, 0.0], 4.0);
        assert!(bright_corner > dark_corner && bright_corner < 1.0);
    }

    #[test]
    fn clarity_step_edge_has_no_endpoint_halo() {
        let params = sp_core::edit::LocalContrastParams {
            clarity: 50.0,
            ..Default::default()
        };
        let black = local_contrast_l(&params, 0.0, 0.0, 0.35, 1.0, 0.0);
        let white = local_contrast_l(&params, 1.0, 1.0, 0.65, 1.0, 1.0);
        assert!(black >= 0.0);
        assert!(white <= 1.05);
    }

    #[test]
    fn hdr_shoulder_keeps_reference_white_near_one() {
        let white = hdr_shoulder_luminance(1.0, 2.0, 50.0);
        assert!((0.97..=1.0).contains(&white), "white mapped to {white}");
        assert!(white < 1.1, "reference white was expanded into HDR");
    }

    #[test]
    fn hdr_shoulder_is_smooth_monotonic_and_bounded() {
        let knee = 0.91;
        let epsilon = 1e-3;
        let left = (hdr_shoulder_luminance(knee, 2.0, 50.0)
            - hdr_shoulder_luminance(knee - epsilon, 2.0, 50.0))
            / epsilon;
        let right = (hdr_shoulder_luminance(knee + epsilon, 2.0, 50.0)
            - hdr_shoulder_luminance(knee, 2.0, 50.0))
            / epsilon;
        assert!((left - right).abs() < 0.01, "join slopes: {left}, {right}");

        let samples: Vec<_> = (0..=1024)
            .map(|index| hdr_shoulder_luminance(index as f32 / 16.0, 2.0, 50.0))
            .collect();
        assert!(samples.windows(2).all(|pair| pair[1] >= pair[0]));
        assert!(samples.iter().all(|value| *value < 4.0));
    }

    #[test]
    fn hdr_shoulder_uses_the_selected_headroom_without_crushing_highlights() {
        // At a two-stop content limit, a scene value of 2.0 should still
        // look close to twice reference white. The previous rational curve
        // reduced it to about 1.55 and was visibly dull.
        let highlight = hdr_shoulder_luminance(2.0, 2.0, 50.0);
        assert!(
            (1.85..=2.0).contains(&highlight),
            "highlight mapped to {highlight}"
        );
        let at_limit = hdr_shoulder_luminance(4.0, 2.0, 50.0);
        assert!((3.1..3.5).contains(&at_limit), "limit mapped to {at_limit}");
    }

    #[test]
    fn hdr_shoulder_stays_well_behaved_at_every_ui_extreme() {
        for stops in [0.0, 0.1, 1.0, 2.0, 4.0] {
            let ceiling = 2f32.powf(stops);
            for rolloff in [0.0, 25.0, 50.0, 75.0, 100.0] {
                let samples: Vec<_> = (0..=2048)
                    .map(|index| hdr_shoulder_luminance(index as f32 / 32.0, stops, rolloff))
                    .collect();
                assert!(
                    samples.windows(2).all(|pair| pair[1] >= pair[0]),
                    "non-monotonic at {stops} stops / {rolloff} rolloff"
                );
                assert!(samples.iter().all(|value| value.is_finite()));
                assert!(
                    samples.iter().all(|value| *value <= ceiling + 1e-6),
                    "exceeded {ceiling} at {stops} stops / {rolloff} rolloff"
                );
                let white = hdr_shoulder_luminance(1.0, stops, rolloff);
                assert!(
                    (0.97..=1.001).contains(&white),
                    "reference white {white} at {stops} stops / {rolloff} rolloff"
                );
            }
        }
    }

    #[test]
    fn hdr_to_sdr_keeps_white_bright_and_preserves_rgb_ratios() {
        let white = hdr_to_sdr_working_rgb([1.0; 3], 50.0);
        assert!(
            (0.94..1.0).contains(&white[0]),
            "white mapped to {}",
            white[0]
        );

        let color = hdr_to_sdr_working_rgb([2.0, 1.0, 0.5], 50.0);
        assert!((color[0] / color[1] - 2.0).abs() < 1e-6);
        assert!((color[1] / color[2] - 2.0).abs() < 1e-6);
        assert!(color.iter().all(|value| value.is_finite() && *value >= 0.0));
    }

    #[test]
    fn hdr_to_sdr_leaves_deep_values_and_black_unchanged() {
        assert_eq!(hdr_to_sdr_working_rgb([0.0; 3], 50.0), [0.0; 3]);
        let dim = [0.1, 0.2, 0.3];
        assert_eq!(hdr_to_sdr_working_rgb(dim, 50.0), dim);
    }
}
