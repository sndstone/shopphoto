//! Expert four-phase integer Bayer reconstruction. Offsets are explicit input,
//! not inferred from camera names or an arbitrary burst's feature alignment.
use crate::{
    color::{mat3_apply, mat3_mul, XYZ_TO_REC2020},
    raw::{ImageMeta, Orientation, RawImage},
    source::RasterImage,
};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Descriptor {
    pub version: u32,
    /// For reference scene pixel (x,y), sample source sensor (x+dx,y+dy).
    /// Relative to each source's active-area origin, before EXIF orientation.
    pub offsets: [[i32; 2]; 4],
    /// Maximum normalized difference between redundant green measurements.
    pub green_tolerance: f32,
}
impl Default for Descriptor {
    fn default() -> Self {
        Self {
            version: 1,
            offsets: [[0, 0], [1, 0], [1, 1], [0, 1]],
            green_tolerance: 0.03,
        }
    }
}
#[derive(Debug)]
pub struct ResultImage {
    pub image: RasterImage,
    pub crop: [u32; 4],
    pub inconsistent_pixels: u64,
    pub green_rms: f32,
}
fn exposure(meta: &ImageMeta) -> Result<f32, String> {
    meta.shutter_s
        .filter(|v| v.is_finite() && *v > 0.0)
        .ok_or_else(|| "Pixel shift requires capture exposure metadata".into())
}
/// Reconstruct one real R, two G and one B measurement for every output pixel.
/// This four-shot mode improves color sampling, not spatial pixel dimensions.
pub fn reconstruct(
    frames: &[(&RawImage, &ImageMeta)],
    descriptor: &Descriptor,
    cancel: &AtomicBool,
) -> Result<ResultImage, String> {
    if frames.len() != 4
        || descriptor.version != 1
        || descriptor.offsets[0] != [0, 0]
        || !descriptor.green_tolerance.is_finite()
        || !(0.001..=0.1).contains(&descriptor.green_tolerance)
    {
        return Err("Pixel shift needs a version-1 four-shot descriptor anchored at offset [0,0] and green tolerance 0.001–0.1".into());
    }
    let mut phases = [false; 4];
    for [dx, dy] in descriptor.offsets {
        if !(-2..=2).contains(&dx) || !(-2..=2).contains(&dy) {
            return Err("Only integer sensor shifts from −2 to +2 photosites are supported".into());
        }
        let phase = (dy.rem_euclid(2) * 2 + dx.rem_euclid(2)) as usize;
        if phases[phase] {
            return Err("Offsets must sample all four Bayer phases exactly once".into());
        }
        phases[phase] = true;
    }
    let (reference, metadata) = frames[0];
    let area = reference.active_area;
    let time = exposure(metadata)?;
    if metadata.camera_model.is_empty() || metadata.iso.is_none_or(|v| v == 0) {
        return Err("Pixel shift requires camera and ISO metadata".into());
    }
    for (raw, meta) in frames {
        if raw.width != reference.width
            || raw.height != reference.height
            || raw.active_area != area
            || raw.cfa != reference.cfa
            || raw.orientation != reference.orientation
            || raw.cam_to_xyz != reference.cam_to_xyz
            || raw.black_levels != reference.black_levels
            || raw.white_levels != reference.white_levels
            || meta.camera_make != metadata.camera_make
            || meta.camera_model != metadata.camera_model
            || meta.iso != metadata.iso
            || (exposure(meta)? / time - 1.0).abs() > 0.01
            || meta.aperture_f != metadata.aperture_f
        {
            return Err("Pixel-shift sources must match camera, ISO, exposure, aperture, active geometry, CFA, orientation and sensor calibration".into());
        }
        if raw.cfa_data.len() != raw.width as usize * raw.height as usize
            || area.x.checked_add(area.width).is_none_or(|v| v > raw.width)
            || area
                .y
                .checked_add(area.height)
                .is_none_or(|v| v > raw.height)
            || area.width < 8
            || area.height < 8
            || u64::from(area.width) * u64::from(area.height) > 150_000_000
            || (0..3).any(|c| {
                !raw.black_levels[c].is_finite()
                    || !raw.white_levels[c].is_finite()
                    || raw.white_levels[c] - raw.black_levels[c] < 1.0
            })
        {
            return Err("Invalid Bayer sensor geometry or calibration for pixel shift".into());
        }
    }
    // The current decoder's RGBE contract and ordinary RAW pipeline use one
    // green calibration. A different explicit G2 calibration cannot be inferred
    // from a camera name or safely collapsed for this reconstruction.
    if reference.black_levels[3] != reference.black_levels[1]
        || reference.white_levels[3] != reference.white_levels[1]
    {
        return Err(
            "Independent G2 calibration is not supported by this four-shot decoder contract".into(),
        );
    }
    let left = descriptor
        .offsets
        .iter()
        .map(|p| -p[0])
        .max()
        .unwrap()
        .max(0) as u32;
    let top = descriptor
        .offsets
        .iter()
        .map(|p| -p[1])
        .max()
        .unwrap()
        .max(0) as u32;
    let right = area.width
        - descriptor
            .offsets
            .iter()
            .map(|p| p[0])
            .max()
            .unwrap()
            .max(0) as u32;
    let bottom = area.height
        - descriptor
            .offsets
            .iter()
            .map(|p| p[1])
            .max()
            .unwrap()
            .max(0) as u32;
    let (cw, ch) = (right - left, bottom - top);
    let (width, height) = match reference.orientation {
        Orientation::Rotate90 | Orientation::Rotate270 => (ch, cw),
        _ => (cw, ch),
    };
    let mut pixels = vec![[0.0, 0.0, 0.0, 1.0]; width as usize * height as usize];
    let matrix = mat3_mul(
        &XYZ_TO_REC2020,
        &crate::profile::display_cam_to_xyz(&reference.cam_to_xyz, false),
    );
    let wb = reference.wb_coeffs;
    let gains = [wb[0] / wb[1], 1.0, wb[2] / wb[1]];
    if gains.iter().any(|v| !v.is_finite() || *v <= 0.0) {
        return Err("Pixel-shift reference white balance is invalid".into());
    }
    let mut inconsistent = 0u64;
    let mut squared = 0.0f64;
    for y in top..bottom {
        if cancel.load(Ordering::Relaxed) {
            return Err("Pixel-shift reconstruction cancelled".into());
        }
        for x in left..right {
            let mut rgb = [0.0; 3];
            let mut counts = [0u32; 3];
            let mut greens = [0.0; 2];
            for ((raw, _), [dx, dy]) in frames.iter().zip(descriptor.offsets) {
                let sx = (area.x as i32 + x as i32 + dx) as u32;
                let sy = (area.y as i32 + y as i32 + dy) as u32;
                let c = raw.cfa.color_at(sx, sy) as usize;
                let value = (raw.cfa_data[(sy * raw.width + sx) as usize] as f32
                    - raw.black_levels[c])
                    / (raw.white_levels[c] - raw.black_levels[c]);
                if c == 1 {
                    greens[counts[c] as usize] = value;
                }
                rgb[c] += value;
                counts[c] += 1;
            }
            let difference = (greens[0] - greens[1]).abs();
            squared += f64::from(difference).powi(2);
            if difference > descriptor.green_tolerance {
                inconsistent += 1;
            }
            for c in 0..3 {
                rgb[c] = rgb[c] / counts[c] as f32 * gains[c];
            }
            let rgb = mat3_apply(&matrix, rgb);
            let (x, y) = (x - left, y - top);
            let (ox, oy) = match reference.orientation {
                Orientation::Normal => (x, y),
                Orientation::Rotate90 => (ch - 1 - y, x),
                Orientation::Rotate180 => (cw - 1 - x, ch - 1 - y),
                Orientation::Rotate270 => (y, cw - 1 - x),
            };
            pixels[(oy * width + ox) as usize] = [rgb[0], rgb[1], rgb[2], 1.0];
        }
    }
    let count = u64::from(width) * u64::from(height);
    if inconsistent * 100 > count {
        return Err(format!("Pixel-shift static-scene check failed: {:.2}% of duplicate green samples disagree. Verify offsets, motion, light and sensor calibration.",inconsistent as f64*100.0/count as f64));
    }
    Ok(ResultImage {
        image: RasterImage {
            width,
            height,
            linear_rec2020: pixels,
            bits_per_channel: 32,
            embedded_icc: None,
        },
        crop: [left, top, cw, ch],
        inconsistent_pixels: inconsistent,
        green_rms: (squared / count as f64).sqrt() as f32,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    fn scene(x: i32, y: i32) -> [f32; 3] {
        [
            0.2 + 0.06 * (x as f32 * 1.9).sin(),
            0.3 + 0.06 * (y as f32 * 1.3).cos(),
            0.5 + 0.05 * ((x + y) as f32 * 2.0).sin(),
        ]
    }
    fn frames() -> Vec<(RawImage, ImageMeta)> {
        Descriptor::default()
            .offsets
            .into_iter()
            .map(|[dx, dy]| {
                let cfa = crate::CfaPattern::Rggb;
                let raw = RawImage {
                    width: 32,
                    height: 24,
                    cfa,
                    cfa_data: (0..24)
                        .flat_map(|y| (0..32).map(move |x| (x, y)))
                        .map(|(x, y)| {
                            (scene(x as i32 - dx, y as i32 - dy)[cfa.color_at(x, y) as usize]
                                * 60000.0
                                + 1000.0)
                                .round() as u16
                        })
                        .collect(),
                    black_levels: [1000.0; 4],
                    white_levels: [61000.0; 4],
                    wb_coeffs: [1.0; 4],
                    cam_to_xyz: crate::color::REC2020_TO_XYZ,
                    active_area: crate::raw::ActiveArea {
                        x: 0,
                        y: 0,
                        width: 32,
                        height: 24,
                    },
                    orientation: Orientation::Normal,
                };
                (
                    raw,
                    ImageMeta {
                        camera_make: "Test".into(),
                        camera_model: "Expert Bayer".into(),
                        iso: Some(100),
                        shutter_s: Some(0.1),
                        aperture_f: Some(8.0),
                        ..Default::default()
                    },
                )
            })
            .collect()
    }
    #[test]
    fn four_phases_reconstruct_real_high_frequency_color_without_demosaic() {
        let frames = frames();
        let refs: Vec<_> = frames.iter().map(|(raw, meta)| (raw, meta)).collect();
        let result = reconstruct(&refs, &Descriptor::default(), &AtomicBool::new(false)).unwrap();
        assert_eq!((result.image.width, result.image.height), (31, 23));
        assert_eq!(result.inconsistent_pixels, 0);
        // Published rounded Rec.2020/D65 matrices and u16 quantization
        // account for at most 2e-4; phase errors are orders of magnitude larger.
        for y in 0..23 {
            for x in 0..31 {
                let expected = scene(x, y);
                let actual = result.image.linear_rec2020[(y * 31 + x) as usize];
                for c in 0..3 {
                    assert!(
                        (expected[c] - actual[c]).abs() < 0.0002,
                        "{expected:?} {actual:?}"
                    );
                }
            }
        }
    }
    #[test]
    fn rejects_duplicate_phases_metadata_mismatch_motion_and_cancel() {
        let mut frames = frames();
        let refs: Vec<_> = frames.iter().map(|(raw, meta)| (raw, meta)).collect();
        let mut descriptor = Descriptor::default();
        descriptor.offsets[1] = [0, 0];
        assert!(reconstruct(&refs, &descriptor, &AtomicBool::new(false)).is_err());
        assert!(reconstruct(&refs, &Descriptor::default(), &AtomicBool::new(true)).is_err());
        frames[1].1.iso = Some(200);
        let refs: Vec<_> = frames.iter().map(|(raw, meta)| (raw, meta)).collect();
        assert!(reconstruct(&refs, &Descriptor::default(), &AtomicBool::new(false)).is_err());
        frames[1].1.iso = Some(100);
        for v in &mut frames[1].0.cfa_data {
            *v += 6000;
        }
        let refs: Vec<_> = frames.iter().map(|(raw, meta)| (raw, meta)).collect();
        assert!(
            reconstruct(&refs, &Descriptor::default(), &AtomicBool::new(false))
                .unwrap_err()
                .contains("static-scene")
        );
    }
    #[test]
    fn unequal_rgb_levels_rotation_and_extreme_offsets_are_safe() {
        let mut frames = frames();
        for (raw, _) in &mut frames {
            let previous = raw.cfa_data.clone();
            raw.black_levels = [500.0, 1500.0, 3000.0, 1500.0];
            raw.white_levels = [62000.0, 58000.0, 64000.0, 58000.0];
            raw.orientation = Orientation::Rotate90;
            for y in 0..raw.height {
                for x in 0..raw.width {
                    let i = (y * raw.width + x) as usize;
                    let c = raw.cfa.color_at(x, y) as usize;
                    let v = (previous[i] as f32 - 1000.0) / 60000.0;
                    raw.cfa_data[i] = (raw.black_levels[c]
                        + v * (raw.white_levels[c] - raw.black_levels[c]))
                        .round() as u16;
                }
            }
        }
        let refs: Vec<_> = frames.iter().map(|(raw, meta)| (raw, meta)).collect();
        let result = reconstruct(&refs, &Descriptor::default(), &AtomicBool::new(false)).unwrap();
        assert_eq!((result.image.width, result.image.height), (23, 31));
        for y in 0..23 {
            for x in 0..31 {
                let expected = scene(x, y);
                let actual = result.image.linear_rec2020[(x * 23 + 22 - y) as usize];
                for c in 0..3 {
                    assert!((actual[c] - expected[c]).abs() < 0.0002);
                }
            }
        }
        let invalid = Descriptor {
            offsets: [[0, 0], [i32::MIN, 0], [1, 1], [0, 1]],
            ..Default::default()
        };
        assert!(reconstruct(&refs, &invalid, &AtomicBool::new(false)).is_err());
        for (raw, _) in &mut frames {
            raw.black_levels[3] += 2.0;
        }
        let refs: Vec<_> = frames.iter().map(|(raw, meta)| (raw, meta)).collect();
        assert!(
            reconstruct(&refs, &Descriptor::default(), &AtomicBool::new(false))
                .unwrap_err()
                .contains("G2")
        );
    }
}
