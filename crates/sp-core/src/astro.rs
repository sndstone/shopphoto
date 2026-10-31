//! Matched-exposure Bayer calibration, before demosaicing. Sensor subtraction
//! stays signed floating point. No dark scaling or guessed camera sequence.
use crate::{
    color::{mat3_apply, mat3_mul, XYZ_TO_REC2020},
    raw::{ActiveArea, CfaPattern, ImageMeta, Orientation, RawImage},
    source::RasterImage,
};
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Debug, Clone, PartialEq)]
struct Signature {
    make: String,
    model: String,
    iso: u32,
    width: u32,
    height: u32,
    area: ActiveArea,
    cfa: CfaPattern,
    range: [f32; 3],
}
impl Signature {
    fn new(raw: &RawImage, meta: &ImageMeta) -> Result<Self, String> {
        let area = raw.active_area;
        if area.width < 4
            || area.height < 4
            || area.x.checked_add(area.width).is_none_or(|v| v > raw.width)
            || area
                .y
                .checked_add(area.height)
                .is_none_or(|v| v > raw.height)
            || raw.cfa_data.len() != raw.width as usize * raw.height as usize
            || u64::from(area.width) * u64::from(area.height) > 150_000_000
        {
            return Err(
                "Astro calibration needs a valid Bayer active area of at most 150 MP".into(),
            );
        }
        let iso = meta
            .iso
            .filter(|v| *v > 0)
            .ok_or("Astro calibration requires capture ISO metadata")?;
        if meta.camera_model.is_empty() {
            return Err("Astro calibration requires the capture camera model".into());
        }
        let range = [0, 1, 2].map(|c| raw.white_levels[c] - raw.black_levels[c]);
        if range.iter().any(|v| !v.is_finite() || *v < 1.0)
            || raw.black_levels.iter().any(|v| !v.is_finite())
        {
            return Err("Invalid sensor black/white calibration".into());
        }
        Ok(Self {
            make: meta.camera_make.clone(),
            model: meta.camera_model.clone(),
            iso,
            width: raw.width,
            height: raw.height,
            area,
            cfa: raw.cfa,
            range,
        })
    }
}
fn exposure(meta: &ImageMeta) -> Result<f32, String> {
    meta.shutter_s
        .filter(|v| v.is_finite() && *v > 0.0)
        .ok_or_else(|| "Astro calibration requires capture exposure times".into())
}
fn same_time(a: f32, b: f32) -> bool {
    (a - b).abs() <= a.max(b) * 0.01
}
fn cancelled(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Relaxed) {
        Err("Astro calibration cancelled".into())
    } else {
        Ok(())
    }
}
fn plane(raw: &RawImage, cancel: &AtomicBool) -> Result<Vec<f32>, String> {
    let a = raw.active_area;
    let mut values = Vec::with_capacity(a.width as usize * a.height as usize);
    for y in a.y..a.y + a.height {
        cancelled(cancel)?;
        for x in a.x..a.x + a.width {
            // Decoder/pipeline use camera channels R,G,B; the fourth slot is
            // a second green coefficient for formats that supply it.
            let c = raw.cfa.color_at(x, y) as usize;
            values.push(
                (raw.cfa_data[(y * raw.width + x) as usize] as f32 - raw.black_levels[c])
                    / (raw.white_levels[c] - raw.black_levels[c]),
            );
        }
    }
    Ok(values)
}

/// Running arithmetic mean. Holds one sensor plane regardless of group size.
#[derive(Debug)]
pub struct Master {
    signature: Signature,
    exposure: f32,
    pub count: usize,
    values: Vec<f32>,
}
impl Master {
    pub fn from_raw(raw: &RawImage, meta: &ImageMeta, cancel: &AtomicBool) -> Result<Self, String> {
        Ok(Self {
            signature: Signature::new(raw, meta)?,
            exposure: exposure(meta)?,
            count: 1,
            values: plane(raw, cancel)?,
        })
    }
    pub fn add(
        &mut self,
        raw: &RawImage,
        meta: &ImageMeta,
        cancel: &AtomicBool,
    ) -> Result<(), String> {
        if self.signature != Signature::new(raw, meta)?
            || !same_time(self.exposure, exposure(meta)?)
        {
            return Err("Each calibration group must match camera, ISO, sensor geometry, range and exposure time".into());
        }
        let next = plane(raw, cancel)?;
        self.count += 1;
        for (mean, value) in self.values.iter_mut().zip(next) {
            *mean += (value - *mean) / self.count as f32;
        }
        Ok(())
    }
}

pub struct Calibration {
    signature: Signature,
    exposure: f32,
    subtraction: Option<Vec<f32>>,
    flat: Option<Vec<f32>>,
}
impl Calibration {
    pub fn new(
        raw: &RawImage,
        meta: &ImageMeta,
        bias: Option<Master>,
        dark: Option<Master>,
        flat: Option<Master>,
        flat_dark: Option<Master>,
    ) -> Result<Self, String> {
        let signature = Signature::new(raw, meta)?;
        let light_time = exposure(meta)?;
        for master in [&bias, &dark, &flat, &flat_dark].into_iter().flatten() {
            if master.signature != signature {
                return Err("Calibration frames must match the light camera, ISO, sensor geometry and raw range".into());
            }
        }
        if dark
            .as_ref()
            .is_some_and(|m| !same_time(m.exposure, light_time))
        {
            return Err(
                "Dark exposure must match the lights within 1%; dark scaling is unavailable".into(),
            );
        }
        if bias.as_ref().is_some_and(|m| m.exposure > 0.05) {
            return Err("Bias exposures must be 1/20 second or shorter".into());
        }
        if flat_dark.is_some() && flat.is_none() {
            return Err("Flat-dark frames need matching flat frames".into());
        }
        let flat = if let Some(mut flat) = flat {
            if flat.values.iter().any(|v| *v >= 0.98) {
                return Err(
                    "Flat calibration contains clipped photosites; use an unclipped flat set"
                        .into(),
                );
            }
            let correction = if let Some(ref fd) = flat_dark {
                if !same_time(fd.exposure, flat.exposure) {
                    return Err("Flat-dark exposure must match the flats within 1%".into());
                }
                &fd.values
            } else if let Some(ref bias) = bias {
                if flat.exposure > 1.0 {
                    return Err(
                        "Flats longer than one second require matched flat-dark frames".into(),
                    );
                }
                &bias.values
            } else {
                return Err("Flat calibration needs bias or matched flat-dark frames".into());
            };
            let mut sums = [0.0f64; 3];
            let mut counts = [0usize; 3];
            for (i, (v, b)) in flat.values.iter_mut().zip(correction).enumerate() {
                *v -= b;
                if !v.is_finite() || *v <= 1e-5 {
                    return Err("Corrected flat contains zero/negative sensor response; replace the flat set".into());
                }
                let x = i as u32 % signature.area.width + signature.area.x;
                let y = i as u32 / signature.area.width + signature.area.y;
                let c = signature.cfa.color_at(x, y) as usize;
                sums[c] += f64::from(*v);
                counts[c] += 1;
            }
            let means = [0, 1, 2].map(|c| (sums[c] / counts[c] as f64) as f32);
            for (i, v) in flat.values.iter_mut().enumerate() {
                let c = signature.cfa.color_at(
                    i as u32 % signature.area.width + signature.area.x,
                    i as u32 / signature.area.width + signature.area.y,
                ) as usize;
                *v /= means[c];
                if *v < 0.05 || *v > 20.0 {
                    return Err("Flat response exceeds the supported correction range".into());
                }
            }
            Some(flat.values)
        } else {
            None
        };
        // A matched dark already contains bias. Subtract exactly once.
        let subtraction = dark.or(bias).map(|master| master.values);
        Ok(Self {
            signature,
            exposure: light_time,
            subtraction,
            flat,
        })
    }
    pub fn apply(
        &self,
        raw: &RawImage,
        meta: &ImageMeta,
        wb: [f32; 4],
        cancel: &AtomicBool,
    ) -> Result<RasterImage, String> {
        if self.signature != Signature::new(raw, meta)?
            || !same_time(self.exposure, exposure(meta)?)
        {
            return Err(
                "Astro lights must match camera, ISO, sensor geometry, range and exposure time"
                    .into(),
            );
        }
        let mut values = plane(raw, cancel)?;
        for (i, v) in values.iter_mut().enumerate() {
            if let Some(subtraction) = &self.subtraction {
                *v -= subtraction[i];
            }
            if let Some(flat) = &self.flat {
                *v /= flat[i];
            }
        }
        let a = raw.active_area;
        let cfa = raw.cfa.shifted(a.x, a.y);
        let matrix = mat3_mul(
            &XYZ_TO_REC2020,
            &crate::profile::display_cam_to_xyz(&raw.cam_to_xyz, false),
        );
        let gains = [wb[0] / wb[1], 1.0, wb[2] / wb[1]];
        if gains.iter().any(|g| !g.is_finite() || *g <= 0.0) {
            return Err("Reference camera WB is invalid".into());
        }
        let (width, height) = match raw.orientation {
            Orientation::Rotate90 | Orientation::Rotate270 => (a.height, a.width),
            _ => (a.width, a.height),
        };
        let mut pixels = vec![[0.0, 0.0, 0.0, 1.0]; width as usize * height as usize];
        for y in 0..a.height {
            cancelled(cancel)?;
            for x in 0..a.width {
                let own = cfa.color_at(x, y) as usize;
                let mut rgb = [0.0; 3];
                let mut weights = [0.0; 3];
                for dy in -1i32..=1 {
                    for dx in -1i32..=1 {
                        let (sx, sy) = (x as i32 + dx, y as i32 + dy);
                        if sx < 0 || sy < 0 || sx >= a.width as i32 || sy >= a.height as i32 {
                            continue;
                        }
                        let c = cfa.color_at(sx as u32, sy as u32) as usize;
                        if c == own && (dx != 0 || dy != 0) {
                            continue;
                        }
                        let weight = if dx != 0 && dy != 0 { 0.5 } else { 1.0 };
                        rgb[c] += values[sy as usize * a.width as usize + sx as usize] * weight;
                        weights[c] += weight;
                    }
                }
                for c in 0..3 {
                    rgb[c] = rgb[c] / weights[c] * gains[c];
                }
                let rgb = mat3_apply(&matrix, rgb);
                let (ox, oy) = match raw.orientation {
                    Orientation::Normal => (x, y),
                    Orientation::Rotate90 => (a.height - 1 - y, x),
                    Orientation::Rotate180 => (a.width - 1 - x, a.height - 1 - y),
                    Orientation::Rotate270 => (y, a.width - 1 - x),
                };
                pixels[(oy * width + ox) as usize] = [rgb[0], rgb[1], rgb[2], 1.0];
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
}

#[cfg(test)]
mod tests {
    use super::*;
    fn raw(signal: impl Fn(u32, u32) -> f32) -> RawImage {
        RawImage {
            width: 32,
            height: 24,
            cfa: CfaPattern::Rggb,
            cfa_data: (0..24)
                .flat_map(|y| (0..32).map(move |x| (x, y)))
                .map(|(x, y)| {
                    (signal(x, y) * 60000.0 + 1000.0)
                        .round()
                        .clamp(0.0, 65535.0) as u16
                })
                .collect(),
            black_levels: [1000.0; 4],
            white_levels: [61000.0; 4],
            wb_coeffs: [1.0; 4],
            cam_to_xyz: crate::color::REC2020_TO_XYZ,
            active_area: ActiveArea {
                x: 0,
                y: 0,
                width: 32,
                height: 24,
            },
            orientation: Orientation::Normal,
        }
    }
    fn meta(time: f32) -> ImageMeta {
        ImageMeta {
            camera_make: "Test".into(),
            camera_model: "Bayer".into(),
            iso: Some(800),
            shutter_s: Some(time),
            ..Default::default()
        }
    }
    #[test]
    fn dark_contains_bias_once_flat_corrects_before_signed_demosaic() {
        let cancel = AtomicBool::new(false);
        let response = |x: u32, _y: u32| 0.7 + 0.6 * x as f32 / 31.0;
        let light = raw(|x, y| 0.2 * response(x, y) + 0.03);
        let bias = Master::from_raw(&raw(|_, _| 0.01), &meta(0.001), &cancel).unwrap();
        let dark = Master::from_raw(&raw(|_, _| 0.03), &meta(30.0), &cancel).unwrap();
        let flat = Master::from_raw(
            &raw(|x, y| 0.5 * response(x, y) + 0.01),
            &meta(0.1),
            &cancel,
        )
        .unwrap();
        let calibration = Calibration::new(
            &light,
            &meta(30.0),
            Some(bias),
            Some(dark),
            Some(flat),
            None,
        )
        .unwrap();
        let out = calibration
            .apply(&light, &meta(30.0), [1.0; 4], &cancel)
            .unwrap();
        // Per-color flat normalization keeps each channel's flat mean.
        for (i, p) in out.linear_rec2020.iter().enumerate() {
            if i % 32 > 1 && i % 32 < 30 {
                for value in &p[..3] {
                    assert!((*value - 0.2).abs() < 0.003, "{p:?}");
                }
            }
        }
        let below = raw(|_, _| 0.02);
        let negative = calibration
            .apply(&below, &meta(30.0), [1.0; 4], &cancel)
            .unwrap();
        assert!(negative
            .linear_rec2020
            .iter()
            .all(|p| p[..3].iter().all(|v| *v < 0.0)));
    }
    #[test]
    fn calibration_rejects_mismatch_clipped_flats_and_cancellation() {
        let cancel = AtomicBool::new(false);
        let light = raw(|_, _| 0.2);
        let dark = Master::from_raw(&raw(|_, _| 0.01), &meta(15.0), &cancel).unwrap();
        assert!(Calibration::new(&light, &meta(30.0), None, Some(dark), None, None).is_err());
        let calibration = Calibration::new(&light, &meta(30.0), None, None, None, None).unwrap();
        let mut wrong = meta(30.0);
        wrong.iso = Some(1600);
        assert!(calibration
            .apply(&light, &wrong, [1.0; 4], &cancel)
            .is_err());
        let bias = Master::from_raw(&raw(|_, _| 0.01), &meta(0.001), &cancel).unwrap();
        let flat = Master::from_raw(&raw(|_, _| 0.99), &meta(0.1), &cancel).unwrap();
        assert!(Calibration::new(&light, &meta(30.0), Some(bias), None, Some(flat), None).is_err());
        assert!(calibration
            .apply(&light, &meta(30.0), [1.0; 4], &AtomicBool::new(true))
            .is_err());
    }
    #[test]
    fn master_mean_is_bounded_and_orientation_keeps_upright_dimensions() {
        let cancel = AtomicBool::new(false);
        let mut light = raw(|_, _| 0.1);
        let mut master = Master::from_raw(&raw(|_, _| 0.01), &meta(0.001), &cancel).unwrap();
        master
            .add(&raw(|_, _| 0.03), &meta(0.001), &cancel)
            .unwrap();
        assert_eq!(master.count, 2);
        assert!((master.values[0] - 0.02).abs() < 0.00002);
        light.orientation = Orientation::Rotate90;
        let calibration =
            Calibration::new(&light, &meta(30.0), Some(master), None, None, None).unwrap();
        let image = calibration
            .apply(&light, &meta(30.0), [1.0; 4], &cancel)
            .unwrap();
        assert_eq!((image.width, image.height), (24, 32));
        assert!((image.linear_rec2020[0][0] - 0.08).abs() < 0.0001);
    }
}
