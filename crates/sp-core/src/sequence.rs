//! Exposure continuity for an overlapping sequence. White balance is resolved
//! from the reference and copied explicitly; exposure is measured from paired
//! scene samples after that common WB, not from a frame's overall scene key.
use crate::{
    color::{mat3_apply, mat3_mul, XYZ_TO_REC2020},
    edit::WhiteBalance,
    source::{ImageSource, RasterImage},
};
use std::sync::atomic::{AtomicBool, Ordering};

pub fn reference_white_balance(
    source: &ImageSource,
    wb: WhiteBalance,
) -> Result<WhiteBalance, String> {
    if wb.temp_kelvin.is_some() {
        return Ok(wb);
    }
    match source {
        ImageSource::Raster(_) => Ok(WhiteBalance {
            temp_kelvin: Some(5500.0),
            tint: wb.tint,
        }),
        ImageSource::Raw(raw) => {
            let g = raw.wb_coeffs[1].max(1e-6);
            let camera_rgb = [
                g / raw.wb_coeffs[0].max(1e-6),
                1.0,
                g / raw.wb_coeffs[2].max(1e-6),
            ];
            let (temp, tint) = crate::wb::temp_tint_for_neutral(&raw.cam_to_xyz, camera_rgb)
                .ok_or("Reference as-shot white balance could not be resolved")?;
            Ok(WhiteBalance {
                temp_kelvin: Some(temp),
                tint,
            })
        }
    }
}

/// Bounded linear thumbnail for robust paired measurements. RAW uses native
/// Bayer quads and capture calibration; raster images keep their ICC conversion.
pub fn thumbnail(
    source: &ImageSource,
    wb: WhiteBalance,
    exposure: f32,
    cancel: &AtomicBool,
) -> Result<RasterImage, String> {
    let valid_source = match source {
        ImageSource::Raster(image) => image.has_finite_opaque_pixels(),
        ImageSource::Raw(raw) => {
            raw.width > 0
                && raw.height > 0
                && raw.active_area.width > 0
                && raw.active_area.height > 0
                && (raw.width as usize).checked_mul(raw.height as usize) == Some(raw.cfa_data.len())
                && raw
                    .active_area
                    .x
                    .checked_add(raw.active_area.width)
                    .is_some_and(|end| end <= raw.width)
                && raw
                    .active_area
                    .y
                    .checked_add(raw.active_area.height)
                    .is_some_and(|end| end <= raw.height)
        }
    };
    if !valid_source {
        return Err(
            "Sequence thumbnails require complete source pixels and valid dimensions".into(),
        );
    }
    if !exposure.is_finite()
        || !wb.tint.is_finite()
        || wb
            .temp_kelvin
            .is_some_and(|temp| !temp.is_finite() || temp <= 0.0)
    {
        return Err("Sequence exposure and white balance must be finite and valid".into());
    }
    let (sw, sh) = source.dimensions();
    let step = sw.max(sh).div_ceil(512).max(1);
    let (width, height) = ((sw / step).max(1), (sh / step).max(1));
    let ev = 2.0f32.powf(exposure);
    if !ev.is_finite() {
        return Err("Sequence exposure exceeds the finite processing range".into());
    }
    let mut pixels = Vec::with_capacity(width as usize * height as usize);
    let raw_params = source.as_raw().map(|raw| {
        let gains = wb
            .temp_kelvin
            .and_then(|t| crate::wb::wb_gains_for_temp(&raw.cam_to_xyz, t, wb.tint))
            .unwrap_or_else(|| {
                let g = raw.wb_coeffs[1].max(1e-6);
                [raw.wb_coeffs[0] / g, 1.0, raw.wb_coeffs[2] / g]
            });
        let matrix = mat3_mul(
            &XYZ_TO_REC2020,
            &crate::profile::display_cam_to_xyz(&raw.cam_to_xyz, false),
        );
        (gains, matrix)
    });
    let raster_gains = RasterImage::white_balance_gains(wb.temp_kelvin, wb.tint);
    for y in 0..height {
        if cancel.load(Ordering::Relaxed) {
            return Err("Sequence matching cancelled".into());
        }
        for x in 0..width {
            let rgb = match source {
                ImageSource::Raster(image) => {
                    let p = image.linear_rec2020
                        [((y * step).min(sh - 1) * sw + (x * step).min(sw - 1)) as usize];
                    [
                        p[0] * raster_gains[0],
                        p[1] * raster_gains[1],
                        p[2] * raster_gains[2],
                    ]
                }
                ImageSource::Raw(raw) => {
                    let (gains, matrix) = raw_params.as_ref().unwrap();
                    let (sx, sy) = (
                        raw.active_area.x + (x * step).min(sw.saturating_sub(2)),
                        raw.active_area.y + (y * step).min(sh.saturating_sub(2)),
                    );
                    let mut sum = [0.0f32; 3];
                    let mut count = [0u32; 3];
                    for dy in 0..2 {
                        for dx in 0..2 {
                            let (px, py) =
                                ((sx + dx).min(raw.width - 1), (sy + dy).min(raw.height - 1));
                            let c = raw.cfa.color_at(px, py) as usize;
                            if c > 2 {
                                return Err("Sequence matching needs an RGB Bayer camera".into());
                            }
                            let value = (raw.cfa_data[(py * raw.width + px) as usize] as f32
                                - raw.black_levels[c])
                                / (raw.white_levels[c] - raw.black_levels[c]).max(1.0);
                            sum[c] += value;
                            count[c] += 1;
                        }
                    }
                    mat3_apply(
                        matrix,
                        [0, 1, 2].map(|c| sum[c] / count[c].max(1) as f32 * gains[c]),
                    )
                }
            };
            let pixel = [rgb[0] * ev, rgb[1] * ev, rgb[2] * ev, 1.0];
            if pixel.iter().any(|value| !value.is_finite()) {
                return Err("Sequence thumbnail produced nonfinite source values".into());
            }
            pixels.push(pixel);
        }
    }
    Ok(RasterImage {
        width,
        height,
        linear_rec2020: pixels,
        bits_per_channel: 32,
        embedded_icc: None,
    })
}

#[derive(Debug, Clone)]
pub struct ExposureMatch {
    pub delta_ev: f32,
    pub samples: usize,
    pub spread_ev: f32,
    pub correlation: f32,
}

pub fn match_exposure(
    reference: &RasterImage,
    source: &RasterImage,
    align: bool,
    cancel: &AtomicBool,
) -> Result<ExposureMatch, String> {
    if !reference.has_finite_opaque_pixels() || !source.has_finite_opaque_pixels() {
        return Err("Sequence frames require complete finite opaque source pixels".into());
    }
    if (reference.width, reference.height) != (source.width, source.height) {
        return Err("Sequence frames must have equal upright dimensions".into());
    }
    let registration = if align {
        crate::multiframe::register(reference, source, cancel)?
    } else {
        Default::default()
    };
    let luminance = |p: [f32; 4]| (p[0] * 0.2627 + p[1] * 0.6780 + p[2] * 0.0593).max(0.0);
    let mut ratios = Vec::new();
    for y in 2..reference.height.saturating_sub(2) {
        if cancel.load(Ordering::Relaxed) {
            return Err("Sequence matching cancelled".into());
        }
        for x in 2..reference.width.saturating_sub(2) {
            let (sx, sy) = registration.point(x as f32, y as f32, source.width, source.height);
            if sx < 0.0
                || sy < 0.0
                || sx > (source.width - 1) as f32
                || sy > (source.height - 1) as f32
            {
                continue;
            }
            let a = reference.linear_rec2020[(y * reference.width + x) as usize];
            let b = source.linear_rec2020
                [(sy.round() as u32 * source.width + sx.round() as u32) as usize];
            if a[..3]
                .iter()
                .chain(&b[..3])
                .any(|v| !v.is_finite() || *v < 0.005 || *v > 0.95)
            {
                continue;
            }
            let (la, lb) = (luminance(a), luminance(b));
            ratios.push((la / lb).log2());
        }
    }
    if ratios.len() < 64 {
        return Err("Too few unclipped overlapping samples to match exposure".into());
    }
    ratios.sort_by(f32::total_cmp);
    let delta_ev = ratios[ratios.len() / 2];
    let spread_ev = ratios[ratios.len() * 3 / 4] - ratios[ratios.len() / 4];
    if spread_ev > 0.5 {
        return Err(format!("Sequence content or light differs too much for a reliable global exposure match ({spread_ev:.2} EV spread)"));
    }
    Ok(ExposureMatch {
        delta_ev,
        samples: ratios.len(),
        spread_ev,
        correlation: registration.correlation,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn raster() -> RasterImage {
        RasterImage {
            width: 32,
            height: 32,
            linear_rec2020: vec![[0.2, 0.3, 0.4, 1.0]; 32 * 32],
            bits_per_channel: 16,
            embedded_icc: None,
        }
    }

    #[test]
    fn malformed_raster_sources_fail_before_thumbnail_or_exposure_sampling() {
        let valid = raster();
        let mut empty = valid.clone();
        empty.width = 0;
        empty.height = 0;
        empty.linear_rec2020.clear();
        let mut truncated = valid.clone();
        truncated.linear_rec2020.pop();
        let mut nonfinite = valid.clone();
        nonfinite.linear_rec2020[0][0] = f32::NAN;
        let mut infinite = valid.clone();
        infinite.linear_rec2020[0][1] = f32::INFINITY;
        let mut transparent = valid.clone();
        transparent.linear_rec2020[0][3] = 0.0;
        let cancel = AtomicBool::new(false);
        for invalid in [empty, truncated, nonfinite, infinite, transparent] {
            assert!(thumbnail(
                &ImageSource::Raster(Arc::new(invalid.clone())),
                WhiteBalance::default(),
                0.0,
                &cancel,
            )
            .is_err());
            for align in [false, true] {
                assert!(match_exposure(&valid, &invalid, align, &cancel).is_err());
                assert!(match_exposure(&invalid, &valid, align, &cancel).is_err());
            }
        }
        let unaligned = match_exposure(&valid, &valid, false, &cancel).unwrap();
        assert_eq!(unaligned.delta_ev, 0.0);
        assert_eq!(unaligned.samples, 28 * 28);
    }

    #[test]
    fn thumbnail_rejects_nonfinite_controls_and_overflowed_exposure() {
        let source = ImageSource::Raster(Arc::new(raster()));
        let cancel = AtomicBool::new(false);
        for exposure in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, f32::MAX] {
            assert!(thumbnail(&source, WhiteBalance::default(), exposure, &cancel).is_err());
        }
        for wb in [
            WhiteBalance {
                temp_kelvin: Some(f32::NAN),
                tint: 0.0,
            },
            WhiteBalance {
                temp_kelvin: Some(0.0),
                tint: 0.0,
            },
            WhiteBalance {
                temp_kelvin: None,
                tint: f32::INFINITY,
            },
        ] {
            assert!(thumbnail(&source, wb, 0.0, &cancel).is_err());
        }
        let mut image = raster();
        image.linear_rec2020[0][0] = f32::MAX;
        assert!(thumbnail(
            &ImageSource::Raster(Arc::new(image)),
            WhiteBalance::default(),
            1.0,
            &cancel,
        )
        .is_err());
    }

    #[test]
    fn malformed_raw_sources_fail_before_thumbnail_sampling() {
        let valid = crate::RawImage {
            cfa_data: vec![8192; 4],
            width: 2,
            height: 2,
            cfa: crate::CfaPattern::Rggb,
            black_levels: [0.0; 4],
            white_levels: [65535.0; 4],
            wb_coeffs: [1.0; 4],
            cam_to_xyz: crate::color::REC2020_TO_XYZ,
            active_area: crate::raw::ActiveArea {
                x: 0,
                y: 0,
                width: 2,
                height: 2,
            },
            orientation: crate::raw::Orientation::Normal,
        };
        let mut empty = valid.clone();
        empty.width = 0;
        empty.cfa_data.clear();
        let mut truncated = valid.clone();
        truncated.cfa_data.pop();
        let mut outside = valid.clone();
        outside.active_area.x = 1;
        let mut overflow = valid.clone();
        overflow.active_area.x = u32::MAX;
        let mut nonfinite = valid;
        nonfinite.cam_to_xyz[0][0] = f32::NAN;
        for invalid in [empty, truncated, outside, overflow, nonfinite] {
            assert!(thumbnail(
                &ImageSource::Raw(Arc::new(invalid)),
                WhiteBalance::default(),
                0.0,
                &AtomicBool::new(false),
            )
            .is_err());
        }
    }

    #[test]
    fn raw_as_shot_reference_resolves_without_assuming_raster_wb() {
        let matrix = crate::color::REC2020_TO_XYZ;
        let gains = crate::wb::wb_gains_for_temp(&matrix, 4200.0, 20.0).unwrap();
        let pattern = crate::CfaPattern::Rggb;
        let cfa_data = (0..32)
            .flat_map(|y| {
                (0..32).map(move |x| {
                    let channel = pattern.color_at(x, y) as usize;
                    (0.2 / gains[channel] * 65535.0).round() as u16
                })
            })
            .collect();
        let source = ImageSource::Raw(Arc::new(crate::RawImage {
            cfa_data,
            width: 32,
            height: 32,
            cfa: pattern,
            black_levels: [0.0; 4],
            white_levels: [65535.0; 4],
            wb_coeffs: [gains[0], 1.0, gains[2], 1.0],
            cam_to_xyz: matrix,
            active_area: crate::raw::ActiveArea {
                x: 0,
                y: 0,
                width: 32,
                height: 32,
            },
            orientation: crate::raw::Orientation::Normal,
        }));
        let wb = reference_white_balance(&source, WhiteBalance::default()).unwrap();
        assert!((wb.temp_kelvin.unwrap() - 4200.0).abs() < 5.0);
        assert!((wb.tint - 20.0).abs() < 0.1);
        let proxy = thumbnail(&source, wb, 0.0, &AtomicBool::new(false)).unwrap();
        for channel in &proxy.linear_rec2020[0][..3] {
            assert!((*channel - 0.2).abs() < 0.001);
        }
    }
    #[test]
    fn exposure_match_uses_paired_scene_values() {
        let pixels = (0..96)
            .flat_map(|y| {
                (0..128).map(move |x| {
                    let value = 0.3 + 0.1 * (x as f32 * 0.1).sin() + 0.05 * (y as f32 * 0.21).cos();
                    [value, value, value, 1.0]
                })
            })
            .collect();
        let reference = RasterImage {
            width: 128,
            height: 96,
            linear_rec2020: pixels,
            bits_per_channel: 32,
            embedded_icc: None,
        };
        let mut source = reference.clone();
        for p in &mut source.linear_rec2020 {
            for value in &mut p[..3] {
                *value *= 0.25;
            }
        }
        let result = match_exposure(&reference, &source, true, &AtomicBool::new(false)).unwrap();
        assert!((result.delta_ev - 2.0).abs() < 0.01);
        assert!(result.samples > 1000);
        assert!(result.spread_ev < 0.02);
    }
    #[test]
    fn raster_reference_wb_and_thumbnail_preserve_relative_controls() {
        let source = ImageSource::Raster(Arc::new(RasterImage {
            width: 64,
            height: 64,
            linear_rec2020: vec![[0.2, 0.3, 0.4, 1.0]; 4096],
            bits_per_channel: 16,
            embedded_icc: None,
        }));
        let wb = reference_white_balance(&source, WhiteBalance::default()).unwrap();
        assert_eq!(wb.temp_kelvin, Some(5500.0));
        let result = thumbnail(&source, wb, 1.0, &AtomicBool::new(false)).unwrap();
        assert_eq!(result.linear_rec2020[0], [0.4, 0.6, 0.8, 1.0]);
    }
}
