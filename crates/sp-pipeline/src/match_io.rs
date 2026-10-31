//! ICC-aware reference-TIFF ingestion and spatial RAW/TIFF sampling.

use crate::match_fit::ColorPair;
use image::{DynamicImage, ImageDecoder};
use moxcms::{ColorProfile, Layout, ToneReprCurve, TransformOptions};
use sp_core::color::{mat3_apply, mat3_mul, XYZ_TO_REC2020};
use sp_core::raw::{Orientation, RawImage};
use sp_core::SpError;
use std::io::BufReader;
use std::path::Path;

/// Sampling diagnostics shown by the Reference Match wizard.
#[derive(Debug, Clone, Copy)]
pub struct SampleSummary {
    pub candidates: usize,
    pub accepted: usize,
    pub embedded_icc: bool,
}

fn decode_error(path: &Path, reason: impl Into<String>) -> SpError {
    SpError::decode(path.display().to_string(), reason)
}

fn sensor_uv(orientation: Orientation, u: f32, v: f32) -> (f32, f32) {
    match orientation {
        Orientation::Normal => (u, v),
        Orientation::Rotate90 => (v, 1.0 - u),
        Orientation::Rotate180 => (1.0 - u, 1.0 - v),
        Orientation::Rotate270 => (1.0 - v, u),
    }
}

/// Black-subtracted camera RGB around a normalized, orientation-corrected
/// active-area position, before white balance. Used by the white-balance
/// eyedropper.
#[must_use]
pub fn camera_rgb_sample(raw: &RawImage, u: f32, v: f32) -> Option<[f32; 3]> {
    let (u, v) = sensor_uv(raw.orientation, u, v);
    let cx = raw.active_area.x as f32 + u * raw.active_area.width.saturating_sub(1) as f32;
    let cy = raw.active_area.y as f32 + v * raw.active_area.height.saturating_sub(1) as f32;
    let (ix, iy) = (cx.round() as i32, cy.round() as i32);
    let mut sums = [0.0f32; 3];
    let mut counts = [0.0f32; 3];
    for dy in -4..=4 {
        for dx in -4..=4 {
            let (x, y) = (ix + dx, iy + dy);
            if x < 0 || y < 0 || x >= raw.width as i32 || y >= raw.height as i32 {
                continue;
            }
            let channel = raw.cfa.color_at(x as u32, y as u32) as usize;
            let count = raw.cfa_data[y as usize * raw.width as usize + x as usize] as f32;
            let black = raw.black_levels[channel];
            let white = raw.white_levels[channel];
            sums[channel] += ((count - black) / (white - black).max(1.0)).clamp(0.0, 1.0);
            counts[channel] += 1.0;
        }
    }
    (counts.iter().all(|&n| n > 0.0)).then(|| std::array::from_fn(|c| sums[c] / counts[c]))
}

fn neutral_raw_sample(raw: &RawImage, u: f32, v: f32) -> Option<[f32; 3]> {
    let (u, v) = sensor_uv(raw.orientation, u, v);
    let cx = raw.active_area.x as f32 + u * raw.active_area.width.saturating_sub(1) as f32;
    let cy = raw.active_area.y as f32 + v * raw.active_area.height.saturating_sub(1) as f32;
    let ix = cx.round() as i32;
    let iy = cy.round() as i32;
    let mut sums = [0.0f32; 3];
    let mut weights = [0.0f32; 3];
    for dy in -3..=3 {
        for dx in -3..=3 {
            let x = ix + dx;
            let y = iy + dy;
            if x < 0 || y < 0 || x >= raw.width as i32 || y >= raw.height as i32 {
                continue;
            }
            let channel = raw.cfa.color_at(x as u32, y as u32) as usize;
            let count = raw.cfa_data[y as usize * raw.width as usize + x as usize] as f32;
            let black = raw.black_levels[channel];
            let white = raw.white_levels[channel];
            let value = ((count - black) / (white - black).max(1.0)).clamp(0.0, 2.0);
            let dist2 = (cx - x as f32).powi(2) + (cy - y as f32).powi(2);
            let weight = 1.0 / (0.5 + dist2);
            sums[channel] += value * weight;
            weights[channel] += weight;
        }
    }
    if weights.iter().any(|&w| w <= 0.0) {
        return None;
    }
    let mut cam = std::array::from_fn(|c| sums[c] / weights[c]);
    let green = raw.wb_coeffs[1].max(1e-6);
    cam[0] *= raw.wb_coeffs[0] / green;
    cam[2] *= raw.wb_coeffs[2] / green;
    Some(mat3_apply(&mat3_mul(&XYZ_TO_REC2020, &raw.cam_to_xyz), cam))
}

fn pixel_luma(image: &image::ImageBuffer<image::Rgb<u16>, Vec<u16>>, x: u32, y: u32) -> f32 {
    let p = image.get_pixel(x, y).0;
    (0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32) / 65535.0
}

/// Decode one reference TIFF, honor its embedded ICC profile, and collect
/// low-gradient, non-clipped spatial pairs against a neutral RAW render.
///
/// # Errors
/// Returns `SpError::Decode` for malformed/non-RGB TIFFs or unusable ICC
/// profiles. Missing ICC falls back to sRGB and is reported in the summary.
pub fn sample_pair(
    raw: &RawImage,
    tiff_path: &Path,
    max_samples: usize,
) -> Result<(Vec<ColorPair>, SampleSummary), SpError> {
    let file = std::fs::File::open(tiff_path)
        .map_err(|e| SpError::io(tiff_path.display().to_string(), e))?;
    let mut decoder = image::codecs::tiff::TiffDecoder::new(BufReader::new(file))
        .map_err(|e| decode_error(tiff_path, format!("TIFF header: {e}")))?;
    let icc = decoder
        .icc_profile()
        .map_err(|e| decode_error(tiff_path, format!("ICC metadata: {e}")))?;
    let orientation = decoder
        .orientation()
        .map_err(|e| decode_error(tiff_path, format!("orientation: {e}")))?;
    let mut dynamic = DynamicImage::from_decoder(decoder)
        .map_err(|e| decode_error(tiff_path, format!("TIFF pixels: {e}")))?;
    dynamic.apply_orientation(orientation);
    let image = dynamic.into_rgb16();
    let (width, height) = image.dimensions();
    if width < 8 || height < 8 {
        return Err(decode_error(tiff_path, "TIFF is too small for calibration"));
    }

    let target = max_samples.max(50) as f64;
    let step = (((u64::from(width) * u64::from(height)) as f64 / target).sqrt() as u32).max(1);
    let mut encoded = Vec::<u16>::with_capacity(max_samples * 3);
    let mut positions = Vec::<(f32, f32)>::with_capacity(max_samples);
    let mut candidates = 0usize;
    let half = step / 2;
    let mut y = half.max(2);
    while y + 2 < height && positions.len() < max_samples {
        let mut x = half.max(2);
        while x + 2 < width && positions.len() < max_samples {
            candidates += 1;
            let p = image.get_pixel(x, y).0;
            let lo = *p.iter().min().unwrap_or(&0);
            let hi = *p.iter().max().unwrap_or(&u16::MAX);
            let center = pixel_luma(&image, x, y);
            let gradient = [
                pixel_luma(&image, x - 1, y),
                pixel_luma(&image, x + 1, y),
                pixel_luma(&image, x, y - 1),
                pixel_luma(&image, x, y + 1),
            ]
            .into_iter()
            .map(|v| (v - center).abs())
            .fold(0.0f32, f32::max);
            if lo > 384 && hi < 65_000 && gradient < 0.025 {
                let u = x as f32 / width.saturating_sub(1) as f32;
                let v = y as f32 / height.saturating_sub(1) as f32;
                if let Some(ours) = neutral_raw_sample(raw, u, v) {
                    if ours
                        .iter()
                        .all(|value| value.is_finite() && *value > 0.002 && *value < 1.2)
                    {
                        encoded.extend_from_slice(&p);
                        positions.push((u, v));
                    }
                }
            }
            x = x.saturating_add(step);
        }
        y = y.saturating_add(step);
    }
    if positions.len() < 50 {
        return Err(decode_error(
            tiff_path,
            format!("only {} usable low-gradient samples", positions.len()),
        ));
    }

    let source = match &icc {
        Some(bytes) => ColorProfile::new_from_slice(bytes)
            .map_err(|e| decode_error(tiff_path, format!("invalid embedded ICC: {e}")))?,
        None => ColorProfile::new_srgb(),
    };
    let mut destination = ColorProfile::new_bt2020();
    let linear = ToneReprCurve::Lut(Vec::new());
    destination.red_trc = Some(linear.clone());
    destination.green_trc = Some(linear.clone());
    destination.blue_trc = Some(linear);
    let transform = source
        .create_transform_16bit(
            Layout::Rgb,
            &destination,
            Layout::Rgb,
            TransformOptions::default(),
        )
        .map_err(|e| decode_error(tiff_path, format!("ICC transform: {e}")))?;
    let mut linear_2020 = vec![0u16; encoded.len()];
    transform
        .transform(&encoded, &mut linear_2020)
        .map_err(|e| decode_error(tiff_path, format!("ICC conversion: {e}")))?;

    let pairs = positions
        .into_iter()
        .zip(linear_2020.chunks_exact(3))
        .filter_map(|((u, v), target)| {
            let ours = neutral_raw_sample(raw, u, v)?;
            let target = [
                target[0] as f32 / 65535.0,
                target[1] as f32 / 65535.0,
                target[2] as f32 / 65535.0,
            ];
            Some((ours, target))
        })
        .collect::<Vec<_>>();
    Ok((
        pairs,
        SampleSummary {
            candidates,
            accepted: linear_2020.len() / 3,
            embedded_icc: icc.is_some(),
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sp_core::color::REC2020_TO_XYZ;
    use sp_core::raw::{ActiveArea, CfaPattern};

    #[test]
    fn orientation_inverse_mapping() {
        assert_eq!(sensor_uv(Orientation::Normal, 0.2, 0.3), (0.2, 0.3));
        assert_eq!(sensor_uv(Orientation::Rotate90, 0.2, 0.3), (0.3, 0.8));
        assert_eq!(sensor_uv(Orientation::Rotate180, 0.2, 0.3), (0.8, 0.7));
        assert_eq!(sensor_uv(Orientation::Rotate270, 0.2, 0.3), (0.7, 0.2));
    }

    #[test]
    fn samples_a_16_bit_tiff_without_icc() {
        let width = 64;
        let height = 64;
        let mut cfa_data = vec![0u16; width * height];
        for y in 0..height {
            for x in 0..width {
                let value = match CfaPattern::Rggb.color_at(x as u32, y as u32) {
                    0 => 30_000,
                    1 => 24_000,
                    _ => 18_000,
                };
                cfa_data[y * width + x] = value;
            }
        }
        let raw = RawImage {
            cfa_data,
            width: width as u32,
            height: height as u32,
            cfa: CfaPattern::Rggb,
            black_levels: [0.0; 4],
            white_levels: [65_535.0; 4],
            wb_coeffs: [1.0; 4],
            cam_to_xyz: REC2020_TO_XYZ,
            active_area: ActiveArea {
                x: 0,
                y: 0,
                width: width as u32,
                height: height as u32,
            },
            orientation: Orientation::Normal,
        };
        let image = image::ImageBuffer::from_pixel(
            width as u32,
            height as u32,
            image::Rgb([40_000u16, 32_000, 24_000]),
        );
        let path = std::env::temp_dir().join(format!("sp-match-{}.tiff", std::process::id()));
        image
            .save_with_format(&path, image::ImageFormat::Tiff)
            .expect("write TIFF");
        let (pairs, summary) = sample_pair(&raw, &path, 500).expect("sample");
        assert!(pairs.len() >= 50);
        assert_eq!(pairs.len(), summary.accepted);
        assert!(!summary.embedded_icc);
        let _ = std::fs::remove_file(path);
    }
}
