//! Deterministic photographic donor suggestions and review-only dust proposals.
use crate::{edit::SpotCorrection, source::RasterImage};
#[derive(Debug, Clone, PartialEq)]
pub struct DustCandidate {
    pub position: [f32; 2],
    pub radius: f32,
    pub confidence: f32,
}
fn rgb(image: &RasterImage, p: [f32; 2]) -> [f32; 3] {
    let x = (p[0] * image.width.saturating_sub(1) as f32)
        .clamp(0.0, image.width.saturating_sub(1) as f32);
    let y = (p[1] * image.height.saturating_sub(1) as f32)
        .clamp(0.0, image.height.saturating_sub(1) as f32);
    let (x0, y0) = (x as u32, y as u32);
    let (x1, y1) = (
        (x0 + 1).min(image.width - 1),
        (y0 + 1).min(image.height - 1),
    );
    let fetch = |x, y| image.linear_rec2020[(y * image.width + x) as usize];
    let (a, b, c, d) = (fetch(x0, y0), fetch(x1, y0), fetch(x0, y1), fetch(x1, y1));
    let fx = x - x0 as f32;
    let fy = y - y0 as f32;
    std::array::from_fn(|c0| {
        let top = a[c0] + (b[c0] - a[c0]) * fx;
        let bottom = c[c0] + (d[c0] - c[c0]) * fx;
        top + (bottom - top) * fy
    })
}
fn valid(image: &RasterImage) -> bool {
    image.width > 0
        && image.height > 0
        && image.linear_rec2020.len() == image.width as usize * image.height as usize
}
fn ring(image: &RasterImage, center: [f32; 2], radius: f32) -> [[f32; 3]; 16] {
    let short = image.width.min(image.height) as f32;
    std::array::from_fn(|i| {
        let angle = i as f32 * std::f32::consts::TAU / 16.0;
        rgb(
            image,
            [
                center[0] + angle.cos() * radius * short / image.width as f32,
                center[1] + angle.sin() * radius * short / image.height as f32,
            ],
        )
    })
}
/// Search a bounded grid using the correction's surrounding texture and color.
/// The center defect is excluded; brightness mismatch matters less for healing
/// than cloning. Returns a suggestion that remains editable by the photographer.
pub fn suggest_source(image: &RasterImage, spot: &SpotCorrection) -> Option<[f32; 2]> {
    if !valid(image) || spot.radius <= 0.0 || !spot.radius.is_finite() {
        return None;
    }
    let short = image.width.min(image.height) as f32;
    let radius = spot.radius * 1.35;
    let padding = [
        radius * short / image.width as f32,
        radius * short / image.height as f32,
    ];
    if padding.iter().any(|v| *v >= 0.49) {
        return None;
    }
    let target = ring(image, spot.target, radius);
    let mean = |ring: &[[f32; 3]; 16]| {
        std::array::from_fn::<_, 3, _>(|c| ring.iter().map(|v| v[c]).sum::<f32>() / 16.0)
    };
    let target_mean = mean(&target);
    let mut best = None;
    let mut best_score = f32::INFINITY;
    for y in 0..64 {
        for x in 0..64 {
            let point = [
                padding[0] + (x as f32 + 0.5) / 64.0 * (1.0 - padding[0] * 2.0),
                padding[1] + (y as f32 + 0.5) / 64.0 * (1.0 - padding[1] * 2.0),
            ];
            let offset = [point[0] - spot.target[0], point[1] - spot.target[1]];
            let distance =
                (offset[0] * image.width as f32).hypot(offset[1] * image.height as f32) / short;
            if distance < spot.radius * 2.8 {
                continue;
            }
            if spot.stroke.iter().any(|p| {
                p[0] + offset[0] < padding[0]
                    || p[0] + offset[0] > 1.0 - padding[0]
                    || p[1] + offset[1] < padding[1]
                    || p[1] + offset[1] > 1.0 - padding[1]
            }) {
                continue;
            }
            let candidate = ring(image, point, radius);
            let candidate_mean = mean(&candidate);
            let mut score = distance * 0.00001;
            for c in 0..3 {
                let bias = candidate_mean[c] - target_mean[c];
                score += bias * bias * if spot.clone { 1.0 } else { 0.2 };
                for i in 0..16 {
                    let residual =
                        (candidate[i][c] - candidate_mean[c]) - (target[i][c] - target_mean[c]);
                    score += residual * residual / 16.0;
                }
            }
            if score < best_score {
                best = Some(point);
                best_score = score;
            }
        }
    }
    best
}
/// Find compact dark defects on smooth surroundings. Proposals require review:
/// scene details and sensor dust cannot be distinguished reliably from one photo.
pub fn find_dust(image: &RasterImage, sensitivity: f32) -> Vec<DustCandidate> {
    if !valid(image) || image.width < 20 || image.height < 20 {
        return vec![];
    }
    let width = image.width as usize;
    let height = image.height as usize;
    let luma: Vec<f32> = image
        .linear_rec2020
        .iter()
        .map(|v| v[0] * 0.2627 + v[1] * 0.6780 + v[2] * 0.0593)
        .collect();
    let threshold = 0.16 - sensitivity.clamp(0.0, 1.0) * 0.13;
    let mut proposals = Vec::new();
    for y in 9..height - 9 {
        for x in 9..width - 9 {
            let value = luma[y * width + x];
            if !value.is_finite()
                || value > luma[y * width + x - 1]
                || value > luma[y * width + x + 1]
                || value > luma[(y - 1) * width + x]
                || value > luma[(y + 1) * width + x]
            {
                continue;
            }
            for radius in [2usize, 3, 5, 8] {
                let samples = [
                    luma[y * width + x - radius],
                    luma[y * width + x + radius],
                    luma[(y - radius) * width + x],
                    luma[(y + radius) * width + x],
                    luma[(y - radius) * width + x - radius],
                    luma[(y - radius) * width + x + radius],
                    luma[(y + radius) * width + x - radius],
                    luma[(y + radius) * width + x + radius],
                ];
                let mean = samples.iter().sum::<f32>() / 8.0;
                let contrast = (mean - value) / (mean + 0.02);
                if contrast < threshold {
                    continue;
                }
                let deviation =
                    (samples.iter().map(|v| (v - mean) * (v - mean)).sum::<f32>() / 8.0).sqrt()
                        / (mean + 0.02);
                if deviation > 0.1 || samples.iter().any(|v| *v < value + (mean - value) * 0.35) {
                    continue;
                }
                proposals.push(DustCandidate {
                    position: [
                        (x as f32 + 0.5) / width as f32,
                        (y as f32 + 0.5) / height as f32,
                    ],
                    radius: radius as f32 * 1.3 / image.width.min(image.height) as f32,
                    confidence: (contrast / (deviation + 0.05)).clamp(0.0, 1.0),
                });
                break;
            }
        }
    }
    proposals.sort_by(|a, b| b.confidence.total_cmp(&a.confidence));
    let mut result: Vec<DustCandidate> = Vec::new();
    for proposal in proposals {
        let overlaps = result.iter().any(|existing| {
            ((existing.position[0] - proposal.position[0]) * width as f32)
                .hypot((existing.position[1] - proposal.position[1]) * height as f32)
                < (existing.radius + proposal.radius) * image.width.min(image.height) as f32
        });
        if !overlaps {
            result.push(proposal);
            if result.len() == 100 {
                break;
            }
        }
    }
    result
}
#[cfg(test)]
mod tests {
    use super::*;
    fn image() -> RasterImage {
        RasterImage {
            width: 128,
            height: 96,
            linear_rec2020: (0..128 * 96)
                .map(|i| {
                    let x = (i % 128) as f32;
                    let y = (i / 128) as f32;
                    let background = 0.5 + x * 0.0002;
                    let dust = 0.16 * (-((x - 50.0).powi(2) + (y - 52.0).powi(2)) / 7.0).exp();
                    [background - dust; 4]
                })
                .collect(),
            bits_per_channel: 16,
            embedded_icc: None,
        }
    }
    #[test]
    fn dust_review_finds_a_compact_defect_and_rejects_straight_edges() {
        let mut image = image();
        for y in 0..96 {
            for x in 100..128 {
                image.linear_rec2020[y * 128 + x] = [0.1; 4];
            }
        }
        let candidates = find_dust(&image, 0.5);
        assert!(candidates
            .iter()
            .any(|c| (c.position[0] * 128.0 - 50.0).abs() < 3.0
                && (c.position[1] * 96.0 - 52.0).abs() < 3.0));
        assert!(!candidates
            .iter()
            .any(|c| (c.position[0] * 128.0 - 100.0).abs() < 3.0));
    }
    #[test]
    fn donor_search_excludes_defect_and_finds_similar_surroundings() {
        let image = image();
        let spot = SpotCorrection {
            target: [50.0 / 128.0, 52.0 / 96.0],
            radius: 0.04,
            ..Default::default()
        };
        let donor = suggest_source(&image, &spot).unwrap();
        assert!(
            ((donor[0] - spot.target[0]) * 128.0).hypot((donor[1] - spot.target[1]) * 96.0)
                > spot.radius * 96.0 * 2.8
        );
        assert!((rgb(&image, donor)[0] - 0.51).abs() < 0.02);
    }
}
