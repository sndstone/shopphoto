//! Deterministic histogram-driven automatic tone starting point.

use sp_core::color::{srgb_eotf, srgb_oetf};
use sp_core::edit::ToneParams;

#[cfg(test)]
use crate::tone::{apply_tone_lut, build_tone_lut};
#[cfg(test)]
use sp_core::edit::CurveParams;

/// Luminance distribution on the encoded axis used by the live histogram.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistogramStats {
    pub bins: [u64; 256],
}

impl HistogramStats {
    /// Read the renderer's true per-pixel luminance plane. Older 3-plane
    /// histograms remain accepted so side tests and callers can migrate
    /// without a flag day.
    #[must_use]
    pub fn from_rgb_bins(rgb: &[u32]) -> Option<Self> {
        if rgb.len() < 768 {
            return None;
        }
        let mut bins = [0_u64; 256];
        if rgb.len() >= 1024 {
            for (bin, &count) in bins.iter_mut().zip(&rgb[768..1024]) {
                *bin = u64::from(count);
            }
        } else {
            // Legacy approximation. Kept only for compatibility: channel
            // histograms no longer retain the correlation needed to derive
            // a colorimetric luminance histogram.
            for (index, bin) in bins.iter_mut().enumerate() {
                *bin = (u64::from(rgb[index])
                    + u64::from(rgb[256 + index])
                    + u64::from(rgb[512 + index]))
                    / 3;
            }
        }
        (bins.iter().any(|&count| count != 0)).then_some(Self { bins })
    }

    #[must_use]
    pub fn percentile_encoded(&self, percentile: f32) -> f32 {
        let total: u64 = self.bins.iter().sum();
        if total == 0 {
            return 0.0;
        }
        let target = (percentile.clamp(0.0, 1.0) * total.saturating_sub(1) as f32).round() as u64;
        let mut cumulative = 0_u64;
        for (index, count) in self.bins.iter().enumerate() {
            cumulative += count;
            if cumulative > target {
                return if index <= 223 {
                    index as f32 / 223.0
                } else {
                    // HDR bins cover 0..4 stops above reference white.
                    2f32.powf((index - 224) as f32 * 4.0 / 31.0)
                };
            }
        }
        1.0
    }
}

/// Find a conservative, usable starting tone from image statistics.
#[must_use]
pub fn auto_tone(histogram: &HistogramStats) -> ToneParams {
    let black_e = histogram.percentile_encoded(0.005).clamp(0.0, 1.0);
    let shadow_e = histogram.percentile_encoded(0.10).clamp(0.0, 1.0);
    let median_e = histogram.percentile_encoded(0.50).clamp(1e-4, 1.0);
    let highlight_e = histogram.percentile_encoded(0.90);
    let white_e = histogram.percentile_encoded(0.995);

    // Preserve some of the photographed scene's key instead of forcing
    // every image to the same median. The ±1.75 EV guardrail is what keeps
    // this a restrained starting point instead of an exaggerated phone-HDR
    // effect on intentionally dark or bright photographs.
    let target_median_e = (0.40 + (median_e - 0.40) * 0.20).clamp(0.34, 0.48);
    let target_median = srgb_eotf(target_median_e);
    let median_linear = srgb_eotf(median_e);
    let mut tone = ToneParams {
        exposure_ev: (target_median / median_linear.max(1e-6))
            .log2()
            .clamp(-1.75, 1.75),
        ..ToneParams::default()
    };

    let gain = 2f32.powf(tone.exposure_ev);
    // When exposure has reached its conservative guardrail, a strongly
    // backlit frame can still leave the subject compressed into the bottom
    // few histogram bins. A modest encoded-domain gamma lift opens those
    // mid-shadows while pinning black and reference white. This is the
    // restrained, single-frame analogue of the local lift users expect from
    // phone Auto HDR; it only engages for genuinely deep rendered medians.
    let exposed_median_e = srgb_oetf((median_linear * gain).min(1.0));
    if exposed_median_e < 0.24 && white_e > 0.80 {
        tone.brightness = ((0.24 - exposed_median_e) / 0.24 * 22.0).clamp(0.0, 18.0);
    }

    let exposed_black = srgb_oetf((srgb_eotf(black_e) * gain).min(1.0));
    tone.blacks = ((0.015 - exposed_black) / 0.08 * 100.0).clamp(-24.0, 24.0);

    let exposed_shadow = srgb_oetf((srgb_eotf(shadow_e) * gain).min(1.0));
    tone.shadows = if exposed_shadow < 0.16 {
        ((0.16 - exposed_shadow) / 0.16 * 26.0).clamp(0.0, 26.0)
    } else if exposed_shadow > 0.38 {
        (-((exposed_shadow - 0.38) / 0.30) * 12.0).clamp(-12.0, 0.0)
    } else {
        0.0
    };

    let white_linear = if white_e > 1.0 {
        white_e
    } else {
        srgb_eotf(white_e)
    };
    let exposed_white = white_linear * gain;
    let exposed_white_e = srgb_oetf(exposed_white.min(1.0));
    tone.whites = if exposed_white_e < 0.84 && white_e <= 1.0 {
        ((0.84 - exposed_white_e) / 0.30 * 14.0).clamp(0.0, 14.0)
    } else if exposed_white_e > 0.985 || exposed_white > 1.0 {
        (-((exposed_white.max(1.0).log2() + (exposed_white_e - 0.985) * 3.0) * 10.0))
            .clamp(-24.0, 0.0)
    } else {
        0.0
    };

    let exposed_highlight = if highlight_e > 1.0 {
        highlight_e * gain
    } else {
        srgb_eotf(highlight_e) * gain
    };
    // Blend the broad p90 shoulder with p99.5. Looking only at p90 misses
    // small windows, lamps and speculars in backlit scenes; looking only at
    // p99.5 lets a single hot pixel dominate. Their midpoint protects the
    // visible highlight island without flattening the rest of the frame.
    let highlight_probe = exposed_highlight.max((exposed_highlight + exposed_white) * 0.5);
    if highlight_probe > srgb_eotf(0.88) {
        let pressure = (highlight_probe / srgb_eotf(0.88)).max(1.0).log2();
        tone.highlights = (-pressure * 13.0).clamp(-36.0, 0.0);
    }

    let central_span = highlight_e.min(1.0) - shadow_e;
    tone.contrast = if central_span < 0.38 {
        ((0.38 - central_span) / 0.38 * 8.0).clamp(0.0, 8.0)
    } else if central_span > 0.78 {
        (-((central_span - 0.78) / 0.22) * 5.0).clamp(-5.0, 0.0)
    } else {
        0.0
    };

    // Do not run a synthetic LUT refinement here. The live histogram already
    // includes the active camera/look base curve, and reapplying a generic
    // curve-free model would systematically over-correct that rendered
    // signal. The next real render is the only authoritative refinement.
    tone
}

/// EV correction required to bring the current rendered median to the
/// conservative 42% encoded target. Because the live histogram includes the
/// current edit, callers add this correction to the existing exposure.
#[must_use]
pub fn auto_exposure(histogram: &HistogramStats) -> f32 {
    let median_encoded = histogram.percentile_encoded(0.5).clamp(1e-4, 1.0);
    let median_linear = srgb_eotf(median_encoded);
    (srgb_eotf(0.42) / median_linear.max(1e-6))
        .log2()
        .clamp(-5.0, 5.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tone::standard_base_curve;

    fn synthetic(low: usize, median: usize, high: usize) -> HistogramStats {
        let mut bins = [0_u64; 256];
        bins[low] = 2;
        bins[median] = 996;
        bins[high] = 2;
        HistogramStats { bins }
    }

    fn assert_conservative_balance(stats: &HistogramStats) {
        let tone = auto_tone(stats);
        let input = srgb_eotf(stats.percentile_encoded(0.5).min(1.0));
        let lut = build_tone_lut(&tone, &CurveParams::default(), None);
        let encoded = srgb_oetf(apply_tone_lut(&lut, input));
        assert!(
            (0.25..=0.52).contains(&encoded),
            "median landed at {encoded}"
        );
        assert!((-1.75..=1.75).contains(&tone.exposure_ev));
        assert!((-36.0..=0.0).contains(&tone.highlights));
        assert!((0.0..=18.0).contains(&tone.brightness));
        assert!((-24.0..=24.0).contains(&tone.blacks));
        assert!((-12.0..=26.0).contains(&tone.shadows));
    }

    #[test]
    fn low_key_high_key_and_flat_land_at_target() {
        assert_conservative_balance(&synthetic(1, 35, 180));
        assert_conservative_balance(&synthetic(20, 175, 223));
        assert_conservative_balance(&synthetic(100, 112, 125));
    }

    #[test]
    fn auto_balance_preserves_low_and_high_key_intent() {
        let low = synthetic(1, 35, 180);
        let high = synthetic(20, 175, 223);
        let low_tone = auto_tone(&low);
        let high_tone = auto_tone(&high);
        assert_eq!(low_tone.exposure_ev, 1.75, "low key should hit guardrail");
        assert!(high_tone.exposure_ev < 0.0);

        let render_median = |stats: &HistogramStats, tone: &ToneParams| {
            let input = srgb_eotf(stats.percentile_encoded(0.5));
            srgb_oetf(apply_tone_lut(
                &build_tone_lut(tone, &CurveParams::default(), None),
                input,
            ))
        };
        assert!(render_median(&low, &low_tone) < render_median(&high, &high_tone));
    }

    #[test]
    fn production_standard_curve_keeps_auto_medians_usable() {
        let base = standard_base_curve();
        for stats in [
            synthetic(1, 35, 180),
            synthetic(20, 96, 210),
            synthetic(20, 175, 223),
            synthetic(100, 112, 125),
        ] {
            let tone = auto_tone(&stats);
            let input = srgb_eotf(stats.percentile_encoded(0.5).min(1.0));
            let lut = build_tone_lut(&tone, &CurveParams::default(), Some(&base));
            let encoded = srgb_oetf(apply_tone_lut(&lut, input));
            assert!(
                (0.28..=0.68).contains(&encoded),
                "production median landed at {encoded} for {stats:?}"
            );
        }
    }

    #[test]
    fn backlit_scene_gets_a_bounded_mid_shadow_lift() {
        let mut bins = [0_u64; 256];
        bins[0] = 2;
        bins[6] = 988;
        bins[210] = 10;
        let stats = HistogramStats { bins };
        let tone = auto_tone(&stats);
        assert_eq!(tone.exposure_ev, 1.75);
        assert!(tone.brightness > 0.0);
        assert!(tone.brightness <= 18.0);
        assert!(tone.highlights < 0.0);
    }

    #[test]
    fn renderer_luminance_plane_wins_over_rgb_approximation() {
        let mut bins = vec![0_u32; 1024];
        bins[10] = 100;
        bins[256 + 20] = 100;
        bins[512 + 30] = 100;
        bins[768 + 140] = 100;
        let stats = HistogramStats::from_rgb_bins(&bins).expect("histogram");
        assert_eq!(stats.percentile_encoded(0.5), 140.0 / 223.0);
    }

    #[test]
    fn empty_histograms_are_not_actionable() {
        assert!(HistogramStats::from_rgb_bins(&vec![0_u32; 1024]).is_none());
        assert!(HistogramStats::from_rgb_bins(&vec![0_u32; 768]).is_none());
    }

    #[test]
    fn adversarial_distributions_remain_finite_and_bounded() {
        for bins in [
            {
                let mut bins = [0_u64; 256];
                bins[0] = 1_000_000;
                bins
            },
            {
                let mut bins = [0_u64; 256];
                bins[255] = 1_000_000;
                bins
            },
            [1_u64; 256],
        ] {
            let tone = auto_tone(&HistogramStats { bins });
            for value in [
                tone.exposure_ev,
                tone.brightness,
                tone.contrast,
                tone.highlights,
                tone.shadows,
                tone.whites,
                tone.blacks,
            ] {
                assert!(value.is_finite());
            }
            assert!((-1.75..=1.75).contains(&tone.exposure_ev));
            assert!((-36.0..=0.0).contains(&tone.highlights));
            assert!((-24.0..=14.0).contains(&tone.whites));
            assert!((-24.0..=24.0).contains(&tone.blacks));
        }
    }

    #[test]
    fn small_hdr_tail_triggers_recovery_without_moving_the_scene_key() {
        let mut base_bins = [0_u64; 256];
        base_bins[100] = 1_000;
        let base = auto_tone(&HistogramStats { bins: base_bins });

        let mut hdr_bins = [0_u64; 256];
        hdr_bins[100] = 990;
        hdr_bins[250] = 10; // about 10× reference white in the HDR tail
        let hdr = auto_tone(&HistogramStats { bins: hdr_bins });

        assert_eq!(hdr.exposure_ev, base.exposure_ev);
        assert!(hdr.highlights < base.highlights);
        assert!(hdr.whites < base.whites);
        assert!((-36.0..=0.0).contains(&hdr.highlights));
        assert!((-24.0..=0.0).contains(&hdr.whites));
    }

    #[test]
    fn auto_exposure_is_a_correction_that_converges() {
        let dark = synthetic(2, 70, 180);
        let correction = auto_exposure(&dark);
        assert!(correction > 0.0);

        let median_linear = srgb_eotf(dark.percentile_encoded(0.5));
        let corrected = srgb_oetf(median_linear * 2f32.powf(correction));
        assert!((corrected - 0.42).abs() < 0.001);
    }
}
