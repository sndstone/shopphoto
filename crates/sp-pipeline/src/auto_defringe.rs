//! Conservative automatic purple/green fringe recommendation.
//!
//! The GPU builds two score histograms from pixels that satisfy all of the
//! spatial tests used by the correction shader. This module decides whether
//! that evidence is widespread and strong enough to change the edit. It
//! deliberately prefers a no-op over damaging real subject color.

use sp_gpu::pipelines::HISTOGRAM_BINS;

const PLANE: usize = 256;
const PURPLE_OFFSET: usize = 4 * PLANE;
const GREEN_OFFSET: usize = 5 * PLANE;
const PURPLE_WIDTH_OFFSET: usize = 6 * PLANE;
const GREEN_WIDTH_OFFSET: usize = 7 * PLANE;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DefringeRecommendation {
    pub purple: f32,
    pub green: f32,
    pub radius_px: f32,
    pub threshold: f32,
    pub wide_bloom: bool,
}

#[derive(Debug, Clone, Copy)]
struct Evidence {
    amount: f32,
}

fn plane_evidence(bins: &[u32]) -> Option<Evidence> {
    let total: u64 = bins.iter().map(|&count| u64::from(count)).sum();
    if total < 256 {
        return None;
    }

    // Scores below 48/255 are intentionally ignored: weak chroma on a hard
    // edge is common in normal subjects and demosaic noise.
    let affected: u64 = bins[48..].iter().map(|&count| u64::from(count)).sum();
    let strong: u64 = bins[96..].iter().map(|&count| u64::from(count)).sum();
    let affected_fraction = affected as f64 / total as f64;
    let strong_fraction = strong as f64 / total as f64;

    // A 100 MP image can contain narrow but genuine fringes, so coverage may
    // be small. Requiring both minimum coverage and a high tail prevents a
    // handful of hot/demosaic pixels from enabling the tool.
    if affected_fraction < 0.000_08 || strong_fraction < 0.000_015 {
        return Some(Evidence { amount: 0.0 });
    }

    // Measure the upper tail among actual candidates, not among the many
    // zero-score pixels. Otherwise a narrow one-pixel fringe could disappear
    // below a whole-image percentile despite being spatially coherent.
    let target = ((affected as f64) * 0.95).ceil() as u64;
    let mut cumulative = 0_u64;
    let candidate_tail = bins[48..]
        .iter()
        .enumerate()
        .find_map(|(index, &count)| {
            cumulative += u64::from(count);
            (cumulative >= target).then_some((index + 48) as f32 / 255.0)
        })
        .unwrap_or(0.0);
    if candidate_tail < 0.25 {
        return Some(Evidence { amount: 0.0 });
    }

    let tail = ((candidate_tail - 0.20) / 0.65).clamp(0.0, 1.0);
    let coverage = ((affected_fraction as f32) / 0.003).sqrt().clamp(0.0, 1.0);
    let amount = (18.0 + 48.0 * tail + 14.0 * coverage).clamp(0.0, 78.0);
    Some(Evidence { amount })
}

fn radius_evidence(bins: &[u32]) -> Option<f32> {
    let candidates: u64 = bins[1..].iter().map(|&count| u64::from(count)).sum();
    if candidates == 0 {
        return None;
    }
    // Prefer the upper half of coherent detected widths so a broad halo is
    // not reduced to its narrowest edge pixels. The correction remains
    // bounded by hue and edge masks, so a modest overestimate is safer than
    // leaving the outside of a wide fringe untouched.
    let target = ((candidates as f64) * 0.70).ceil() as u64;
    let mut cumulative = 0_u64;
    bins[1..].iter().enumerate().find_map(|(index, &count)| {
        cumulative += u64::from(count);
        (cumulative >= target).then_some(((index + 1) as f32 / 255.0 * 256.0).clamp(1.0, 256.0))
    })
}

/// Convert the two opt-in fringe score planes into restrained manual
/// settings. Returns `None` when the histogram is absent or no reliable
/// fringe is detected; callers should preserve the user's current settings.
#[must_use]
pub fn recommend(histogram: &[u32]) -> Option<DefringeRecommendation> {
    if histogram.len() < HISTOGRAM_BINS {
        return None;
    }
    let purple = plane_evidence(&histogram[PURPLE_OFFSET..PURPLE_OFFSET + PLANE])?.amount;
    let green = plane_evidence(&histogram[GREEN_OFFSET..GREEN_OFFSET + PLANE])?.amount;
    if purple <= 0.0 && green <= 0.0 {
        return None;
    }
    let purple_radius = (purple > 0.0)
        .then(|| radius_evidence(&histogram[PURPLE_WIDTH_OFFSET..PURPLE_WIDTH_OFFSET + PLANE]))
        .flatten();
    let green_radius = (green > 0.0)
        .then(|| radius_evidence(&histogram[GREEN_WIDTH_OFFSET..GREEN_WIDTH_OFFSET + PLANE]))
        .flatten();
    let radius_px = purple_radius
        .into_iter()
        .chain(green_radius)
        .fold(2.0_f32, f32::max)
        .clamp(1.0, 256.0);
    Some(DefringeRecommendation {
        purple,
        green,
        radius_px,
        threshold: 55.0,
        wide_bloom: radius_px > 8.0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn analysis_histogram(purple: &[(usize, u32)], green: &[(usize, u32)]) -> Vec<u32> {
        let mut bins = vec![0_u32; HISTOGRAM_BINS];
        for plane in [PURPLE_OFFSET, GREEN_OFFSET] {
            bins[plane] = 1_000_000;
        }
        for &(index, count) in purple {
            bins[PURPLE_OFFSET] -= count;
            bins[PURPLE_OFFSET + index] += count;
        }
        for &(index, count) in green {
            bins[GREEN_OFFSET] -= count;
            bins[GREEN_OFFSET + index] += count;
        }
        bins
    }

    #[test]
    fn empty_or_weak_evidence_is_a_no_op() {
        assert!(recommend(&vec![0; HISTOGRAM_BINS]).is_none());
        let sparse = analysis_histogram(&[(180, 10)], &[]);
        assert!(recommend(&sparse).is_none());
        let low_confidence = analysis_histogram(&[(55, 2_000)], &[]);
        assert!(recommend(&low_confidence).is_none());
    }

    #[test]
    fn strong_purple_and_green_evidence_are_independent() {
        let purple = recommend(&analysis_histogram(&[(190, 900)], &[])).expect("purple fringe");
        assert!(purple.purple > 30.0);
        assert_eq!(purple.green, 0.0);

        let green = recommend(&analysis_histogram(&[], &[(170, 700)])).expect("green fringe");
        assert_eq!(green.purple, 0.0);
        assert!(green.green > 30.0);
    }

    #[test]
    fn recommendations_remain_below_destructive_full_strength() {
        let recommendation = recommend(&analysis_histogram(&[(255, 20_000)], &[(255, 20_000)]))
            .expect("strong fringe");
        assert!(recommendation.purple <= 78.0);
        assert!(recommendation.green <= 78.0);
        assert_eq!(recommendation.radius_px, 2.0);
        assert_eq!(recommendation.threshold, 55.0);
        assert!(!recommendation.wide_bloom);
    }

    #[test]
    fn width_planes_enable_wide_bloom_without_unbounded_radius() {
        let mut histogram = analysis_histogram(&[(220, 5_000)], &[]);
        histogram[PURPLE_WIDTH_OFFSET + 64] = 5_000; // approximately 64 px
        let recommendation = recommend(&histogram).expect("wide purple fringe");
        assert!((recommendation.radius_px - 64.25).abs() < 1.1);
        assert!(recommendation.wide_bloom);
    }
}
