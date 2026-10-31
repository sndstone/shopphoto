//! Advisory comparison of source previews. These descriptors do not determine
//! artistic quality, identify people, or automatically rate/reject photographs.

use sp_core::SpError;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PreviewQuality {
    pub detail: f32,
    pub highlights_percent: f32,
    pub shadows_percent: f32,
    /// Optional explicitly requested face/eye preview assistance.
    pub faces: Option<FaceAnalysis>,
    hash: u64,
    histogram: [f32; 16],
}

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FaceAnalysis {
    pub faces: u32,
    pub eye_regions: u32,
    pub weakest_eye_detail: Option<f32>,
}
impl FaceAnalysis {
    pub fn valid(self) -> bool {
        self.faces <= 32
            && self.eye_regions <= self.faces * 2
            && self
                .weakest_eye_detail
                .is_none_or(|v| v.is_finite() && (0.0..=100.0).contains(&v))
            && (self.eye_regions == 0) == self.weakest_eye_detail.is_none()
    }
}

fn resize_luma(
    width: usize,
    height: usize,
    rgba: &[u8],
    out_width: usize,
    out_height: usize,
) -> Vec<f32> {
    (0..out_height)
        .flat_map(|y| {
            (0..out_width).map(move |x| {
                let x0 = x * width / out_width;
                let x1 = ((x + 1) * width / out_width).max(x0 + 1).min(width);
                let y0 = y * height / out_height;
                let y1 = ((y + 1) * height / out_height).max(y0 + 1).min(height);
                let mut sum = 0.0;
                let mut samples = 0;
                for sy in y0..y1 {
                    for sx in x0..x1 {
                        let index = (sy * width + sx) * 4;
                        sum += (0.2126 * f32::from(rgba[index])
                            + 0.7152 * f32::from(rgba[index + 1])
                            + 0.0722 * f32::from(rgba[index + 2]))
                            / 255.0;
                        samples += 1;
                    }
                }
                sum / samples.max(1) as f32
            })
        })
        .collect()
}

pub fn analyze(width: u32, height: u32, rgba: &[u8]) -> Result<PreviewQuality, SpError> {
    let (width, height) = (width as usize, height as usize);
    if width == 0
        || height == 0
        || width
            .checked_mul(height)
            .and_then(|value| value.checked_mul(4))
            != Some(rgba.len())
    {
        return Err(SpError::Catalog(
            "Invalid preview pixels for quality analysis".into(),
        ));
    }
    let side = 64;
    let luma = resize_luma(width, height, rgba, side, side);
    let mut energy = 0.0;
    let mut mean = 0.0;
    let mut squares = 0.0;
    let mut histogram = [0.0; 16];
    for &value in &luma {
        mean += value;
        squares += value * value;
        histogram[(value * 15.999) as usize] += 1.0 / luma.len() as f32;
    }
    mean /= luma.len() as f32;
    let variance = (squares / luma.len() as f32 - mean * mean).max(0.001);
    for y in 1..side - 1 {
        for x in 1..side - 1 {
            let i = y * side + x;
            let laplacian =
                4.0 * luma[i] - luma[i - 1] - luma[i + 1] - luma[i - side] - luma[i + side];
            energy += laplacian * laplacian;
        }
    }
    let detail = (energy / ((side - 2) * (side - 2)) as f32 / variance).ln_1p() * 35.0;
    let hash_pixels = resize_luma(width, height, rgba, 9, 8);
    let mut hash = 0u64;
    for y in 0..8 {
        for x in 0..8 {
            if hash_pixels[y * 9 + x] > hash_pixels[y * 9 + x + 1] {
                hash |= 1u64 << (y * 8 + x);
            }
        }
    }
    let pixels = (width * height) as f32;
    let highlights = rgba
        .chunks_exact(4)
        .filter(|pixel| pixel[..3].iter().all(|v| *v >= 253))
        .count();
    let shadows = rgba
        .chunks_exact(4)
        .filter(|pixel| pixel[..3].iter().all(|v| *v <= 2))
        .count();
    Ok(PreviewQuality {
        detail: detail.clamp(0.0, 100.0),
        highlights_percent: highlights as f32 / pixels * 100.0,
        shadows_percent: shadows as f32 / pixels * 100.0,
        hash,
        histogram,
        faces: None,
    })
}

impl PreviewQuality {
    /// Coarse similarity for culling suggestions, including luminance
    /// distribution so flat images do not all appear identical.
    pub fn similarity(self, other: Self) -> f32 {
        let structure = 1.0 - (self.hash ^ other.hash).count_ones() as f32 / 64.0;
        let distribution = 1.0
            - self
                .histogram
                .iter()
                .zip(other.histogram)
                .map(|(a, b)| (a - b).abs())
                .sum::<f32>()
                * 0.5;
        (structure * 0.7 + distribution * 0.3).clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preview_detail_prefers_edges_and_similarity_rejects_opposite_flat_exposure() {
        let mut sharp = vec![255; 64 * 64 * 4];
        let mut soft = sharp.clone();
        for y in 0..64 {
            for x in 0..64 {
                let value = if (x / 4 + y / 4) % 2 == 0 { 40 } else { 210 };
                for c in 0..3 {
                    sharp[(y * 64 + x) * 4 + c] = value;
                    soft[(y * 64 + x) * 4 + c] = 125;
                }
            }
        }
        let detailed = analyze(64, 64, &sharp).unwrap();
        let smooth = analyze(64, 64, &soft).unwrap();
        assert!(detailed.detail > smooth.detail + 20.0);
        assert_eq!(detailed.similarity(detailed), 1.0);
        let dark = analyze(64, 64, &vec![0; 64 * 64 * 4]).unwrap();
        let white = analyze(64, 64, &vec![255; 64 * 64 * 4]).unwrap();
        assert!(dark.similarity(white) < 0.8);
        assert_eq!(white.highlights_percent, 100.0);
        assert_eq!(dark.shadows_percent, 100.0);
        assert!(analyze(64, 64, &[0; 4]).is_err());
    }
    #[test]
    fn face_analysis_rejects_impossible_counts_and_nonfinite_detail() {
        assert!(FaceAnalysis {
            faces: 1,
            eye_regions: 2,
            weakest_eye_detail: Some(35.0)
        }
        .valid());
        for summary in [
            FaceAnalysis {
                faces: 33,
                eye_regions: 0,
                weakest_eye_detail: None,
            },
            FaceAnalysis {
                faces: 1,
                eye_regions: 3,
                weakest_eye_detail: Some(35.0),
            },
            FaceAnalysis {
                faces: 1,
                eye_regions: 1,
                weakest_eye_detail: Some(f32::NAN),
            },
            FaceAnalysis {
                faces: 0,
                eye_regions: 0,
                weakest_eye_detail: Some(0.0),
            },
        ] {
            assert!(!summary.valid());
        }
    }
}
