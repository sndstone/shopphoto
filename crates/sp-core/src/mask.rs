//! Original-resolution, continuous alpha masks. Pixel storage is shared by
//! undo snapshots; sidecars losslessly compress delta-coded 16-bit pixels.
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AlphaMask {
    pub width: u32,
    pub height: u32,
    #[serde(with = "runs")]
    pub alpha: Arc<[u16]>,
    pub source: String,
    #[serde(default)]
    pub inverted: bool,
    #[serde(default)]
    pub repairs: Vec<MaskStroke>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MaskStroke {
    pub points: Vec<[f32; 2]>,
    pub radius: f32,
    pub feather: f32,
    pub subtract: bool,
}

/// Reusable sampler that skips repair strokes outside their area of influence.
/// Construct once per render region, so bounds are not recomputed per pixel.
pub struct AlphaMaskSampler<'a> {
    mask: &'a AlphaMask,
    bounds: Vec<[f32; 4]>,
}

impl AlphaMaskSampler<'_> {
    pub fn sample(&self, uv: [f32; 2]) -> f32 {
        self.mask.sample_with_bounds(uv, Some(&self.bounds))
    }
}

impl AlphaMask {
    pub fn valid(&self) -> bool {
        self.width > 0
            && self.height > 0
            && u64::from(self.width) * u64::from(self.height) == self.alpha.len() as u64
            && self.alpha.len() <= 150_000_000
    }

    pub fn sampler(&self) -> AlphaMaskSampler<'_> {
        let short = self.width.min(self.height) as f32;
        let bounds = self
            .repairs
            .iter()
            .map(|stroke| {
                let radius = stroke.radius.clamp(0.0001, 1.0);
                // Match the minimum smoothing width used by sample(), including
                // the tiny transition just outside a nominally hard brush.
                let inner = radius * (1.0 - stroke.feather.clamp(0.0, 1.0));
                let radius = radius.max(inner + 1e-6) + f32::EPSILON;
                let padding = [
                    radius * short / self.width.max(1) as f32,
                    radius * short / self.height.max(1) as f32,
                ];
                let mut bounds = [
                    f32::INFINITY,
                    f32::INFINITY,
                    f32::NEG_INFINITY,
                    f32::NEG_INFINITY,
                ];
                for point in &stroke.points {
                    for axis in 0..2 {
                        bounds[axis] = bounds[axis].min(point[axis] - padding[axis]);
                        bounds[axis + 2] = bounds[axis + 2].max(point[axis] + padding[axis]);
                    }
                }
                bounds
            })
            .collect();
        AlphaMaskSampler { mask: self, bounds }
    }

    /// Bilinear sampling in the uncropped, developed-image coordinate frame.
    pub fn sample(&self, uv: [f32; 2]) -> f32 {
        self.sample_with_bounds(uv, None)
    }

    fn sample_with_bounds(&self, uv: [f32; 2], bounds: Option<&[[f32; 4]]>) -> f32 {
        if !self.valid() || !uv.iter().all(|v| v.is_finite()) {
            return 0.0;
        }
        let x = (uv[0] * self.width as f32 - 0.5).clamp(0.0, (self.width - 1) as f32);
        let y = (uv[1] * self.height as f32 - 0.5).clamp(0.0, (self.height - 1) as f32);
        let (ix, iy) = (x as u32, y as u32);
        let get = |x: u32, y: u32| f32::from(self.alpha[(y * self.width + x) as usize]) / 65535.0;
        let (nx, ny) = ((ix + 1).min(self.width - 1), (iy + 1).min(self.height - 1));
        let a = get(ix, iy) * (1.0 - x.fract()) + get(nx, iy) * x.fract();
        let b = get(ix, ny) * (1.0 - x.fract()) + get(nx, ny) * x.fract();
        let mut alpha = a * (1.0 - y.fract()) + b * y.fract();
        if self.inverted {
            alpha = 1.0 - alpha;
        }
        let short_edge = self.width.min(self.height) as f32;
        let metric = [
            self.width as f32 / short_edge,
            self.height as f32 / short_edge,
        ];
        for (index, stroke) in self.repairs.iter().enumerate() {
            if bounds.is_some_and(|bounds| {
                let b = bounds[index];
                uv[0] < b[0] || uv[1] < b[1] || uv[0] > b[2] || uv[1] > b[3]
            }) {
                continue;
            }
            let mut distance = f32::INFINITY;
            for (i, p) in stroke.points.iter().enumerate() {
                let previous = stroke.points[i.saturating_sub(1)];
                let axis = [
                    (p[0] - previous[0]) * metric[0],
                    (p[1] - previous[1]) * metric[1],
                ];
                let delta = [
                    (uv[0] - previous[0]) * metric[0],
                    (uv[1] - previous[1]) * metric[1],
                ];
                let t = ((delta[0] * axis[0] + delta[1] * axis[1])
                    / (axis[0] * axis[0] + axis[1] * axis[1]).max(1e-12))
                .clamp(0.0, 1.0);
                distance = distance.min((delta[0] - t * axis[0]).hypot(delta[1] - t * axis[1]));
            }
            let radius = stroke.radius.clamp(0.0001, 1.0);
            let inner = radius * (1.0 - stroke.feather.clamp(0.0, 1.0));
            let t = ((distance - inner) / (radius - inner).max(1e-6)).clamp(0.0, 1.0);
            let coverage = 1.0 - t * t * (3.0 - 2.0 * t);
            alpha = if stroke.subtract {
                alpha * (1.0 - coverage)
            } else {
                alpha + (1.0 - alpha) * coverage
            };
        }
        alpha
    }
}

mod runs {
    use super::*;
    use base64::Engine;
    use serde::de::Error;
    use std::io::{Read, Write};
    pub fn serialize<S: serde::Serializer>(
        pixels: &Arc<[u16]>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
        let mut previous = 0u16;
        for chunk in pixels.chunks(8192) {
            let mut bytes = Vec::with_capacity(chunk.len() * 2);
            for &value in chunk {
                bytes.extend_from_slice(&value.wrapping_sub(previous).to_le_bytes());
                previous = value;
            }
            encoder
                .write_all(&bytes)
                .map_err(serde::ser::Error::custom)?;
        }
        let bytes = encoder.finish().map_err(serde::ser::Error::custom)?;
        serializer.serialize_str(&format!(
            "d16z1:{}",
            base64::engine::general_purpose::STANDARD.encode(bytes)
        ))
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Arc<[u16]>, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Storage {
            Compressed(String),
            Legacy(Vec<[u32; 2]>),
        }
        let runs = match Storage::deserialize(deserializer)? {
            Storage::Compressed(value) => {
                let data = value
                    .strip_prefix("d16z1:")
                    .ok_or_else(|| D::Error::custom("unknown alpha encoding"))?;
                if data.len() > 410_000_000 {
                    return Err(D::Error::custom("oversized alpha encoding"));
                }
                let compressed = base64::engine::general_purpose::STANDARD
                    .decode(data)
                    .map_err(D::Error::custom)?;
                let mut decoded = Vec::new();
                flate2::read::ZlibDecoder::new(compressed.as_slice())
                    .take(300_000_001)
                    .read_to_end(&mut decoded)
                    .map_err(D::Error::custom)?;
                if decoded.len() > 300_000_000 || decoded.len() % 2 != 0 {
                    return Err(D::Error::custom("invalid or oversized alpha pixels"));
                }
                let mut previous = 0u16;
                let pixels: Vec<_> = decoded
                    .chunks_exact(2)
                    .map(|v| {
                        previous = previous.wrapping_add(u16::from_le_bytes([v[0], v[1]]));
                        previous
                    })
                    .collect();
                return Ok(pixels.into());
            }
            Storage::Legacy(runs) => runs,
        };
        let mut pixels = Vec::new();
        for [value, count] in runs {
            if value > 65535
                || count == 0
                || pixels.len().saturating_add(count as usize) > 150_000_000
            {
                return Err(D::Error::custom("invalid or oversized alpha mask"));
            }
            pixels
                .try_reserve(count as usize)
                .map_err(D::Error::custom)?;
            pixels.resize(pixels.len() + count as usize, value as u16);
        }
        Ok(pixels.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "manual mask sampling benchmark"]
    fn repaired_mask_sampling_benchmark() {
        let mask = AlphaMask {
            width: 256,
            height: 256,
            alpha: vec![32768; 256 * 256].into(),
            source: "benchmark".into(),
            inverted: false,
            repairs: (0..24)
                .map(|i| MaskStroke {
                    points: (0..48)
                        .map(|j| {
                            [
                                0.05 + (i % 6) as f32 * 0.15 + j as f32 * 0.001,
                                0.05 + (i / 6) as f32 * 0.2,
                            ]
                        })
                        .collect(),
                    radius: 0.02,
                    feather: 0.5,
                    subtract: i % 2 == 0,
                })
                .collect(),
        };
        let measure = |sample: &dyn Fn([f32; 2]) -> f32| {
            let start = std::time::Instant::now();
            let values: Vec<_> = (0..256 * 256)
                .map(|i| {
                    sample(std::hint::black_box([
                        (i % 256) as f32 / 256.0,
                        (i / 256) as f32 / 256.0,
                    ]))
                })
                .collect();
            (start.elapsed(), values)
        };
        let (baseline, expected) = measure(&|uv| mask.sample(uv));
        let sampler = mask.sampler();
        let (bounded, actual) = measure(&|uv| sampler.sample(uv));
        assert_eq!(actual, expected);
        eprintln!("repair sampling: baseline {baseline:?}, bounded {bounded:?}");
    }

    #[test]
    fn bounded_sampler_preserves_hard_brush_transition() {
        let mask = AlphaMask {
            width: 1,
            height: 1,
            alpha: vec![0].into(),
            source: "test".into(),
            inverted: false,
            repairs: vec![MaskStroke {
                points: vec![[0.5, 0.5]],
                radius: 0.1,
                feather: 0.0,
                subtract: false,
            }],
        };
        let uv = [0.6000005, 0.5];
        assert!(mask.sample(uv) > 0.0);
        assert_eq!(mask.sampler().sample(uv), mask.sample(uv));
    }

    #[test]
    fn bounded_sampler_matches_repairs_at_every_pixel() {
        let mask = AlphaMask {
            width: 100,
            height: 40,
            alpha: vec![12345; 4000].into(),
            source: "test".into(),
            inverted: true,
            repairs: vec![
                MaskStroke {
                    points: vec![[0.1, 0.2], [0.7, 0.8]],
                    radius: 0.13,
                    feather: 0.6,
                    subtract: true,
                },
                MaskStroke {
                    points: vec![[0.3, 0.5]],
                    radius: 0.2,
                    feather: 0.0,
                    subtract: false,
                },
                MaskStroke {
                    points: vec![],
                    radius: 0.1,
                    feather: 1.0,
                    subtract: false,
                },
            ],
        };
        let sampler = mask.sampler();
        for y in 0..40 {
            for x in 0..100 {
                let uv = [(x as f32 + 0.5) / 100.0, (y as f32 + 0.5) / 40.0];
                assert_eq!(sampler.sample(uv), mask.sample(uv));
            }
        }
        assert_eq!(sampler.sample([f32::NAN, 0.5]), 0.0);
    }

    #[test]
    fn continuous_masks_compress_and_repairs_are_circular_in_pixels() {
        let mut mask = AlphaMask {
            width: 1024,
            height: 512,
            alpha: (0..1024 * 512)
                .map(|i| (i % 65536) as u16)
                .collect::<Vec<_>>()
                .into(),
            source: "gradient".into(),
            inverted: false,
            repairs: vec![],
        };
        let json = serde_json::to_string(&mask).unwrap();
        assert!(
            json.len() < 20000,
            "smooth alpha should compress well: {} bytes",
            json.len()
        );
        assert_eq!(serde_json::from_str::<AlphaMask>(&json).unwrap(), mask);
        mask.width = 4;
        mask.height = 1;
        mask.alpha = vec![0; 4].into();
        mask.repairs.push(MaskStroke {
            points: vec![[0.5, 0.5]],
            radius: 0.1,
            feather: 0.0,
            subtract: false,
        });
        assert_eq!(mask.sample([0.54, 0.5]), 0.0);
        assert_eq!(mask.sample([0.5, 0.54]), 1.0);
    }
    #[test]
    fn soft_alpha_roundtrip_and_pixel_centers() {
        let mask = AlphaMask {
            width: 2,
            height: 2,
            alpha: vec![0, 32768, 65535, 65535].into(),
            source: "test".into(),
            inverted: false,
            repairs: vec![],
        };
        let json = serde_json::to_string(&mask).unwrap();
        assert!(json.contains("d16z1:"));
        assert_eq!(serde_json::from_str::<AlphaMask>(&json).unwrap(), mask);
        let mut legacy = serde_json::to_value(&mask).unwrap();
        legacy["alpha"] = serde_json::json!([[0, 1], [32768, 1], [65535, 2]]);
        assert_eq!(
            serde_json::from_value::<AlphaMask>(legacy.clone()).unwrap(),
            mask
        );
        legacy["alpha"] = serde_json::json!("d16z1:invalid!");
        assert!(serde_json::from_value::<AlphaMask>(legacy).is_err());
        assert_eq!(mask.sample([0.25, 0.25]), 0.0);
        assert!((mask.sample([0.75, 0.25]) - 0.5).abs() < 0.001);
        assert_eq!(mask.sample([0.25, 0.75]), 1.0);
    }
    #[test]
    fn invalid_dimensions_are_empty_and_subtract_is_reversible() {
        let mut mask = AlphaMask {
            width: 1,
            height: 1,
            alpha: vec![65535].into(),
            source: "test".into(),
            inverted: false,
            repairs: vec![],
        };
        mask.repairs.push(MaskStroke {
            points: vec![[0.5, 0.5]],
            radius: 0.1,
            feather: 0.5,
            subtract: true,
        });
        assert_eq!(mask.sample([0.5, 0.5]), 0.0);
        mask.repairs.clear();
        assert_eq!(mask.sample([0.5, 0.5]), 1.0);
        mask.width = 2;
        assert_eq!(mask.sample([0.5, 0.5]), 0.0);
    }
}

/// Reproducible brush dab deposition. Spacing is a fraction of brush diameter;
/// pressure is retained per dab so resizing a stroke preserves its dynamics.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BrushSettings {
    pub size: f32,
    pub feather: f32,
    pub flow: f32,
    pub spacing: f32,
    pub smoothing: f32,
    pub pressure: bool,
}

impl Default for BrushSettings {
    fn default() -> Self {
        Self {
            size: 0.03,
            feather: 0.5,
            flow: 1.0,
            spacing: 0.15,
            smoothing: 0.15,
            pressure: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BrushDab {
    pub point: [f32; 2],
    pub pressure: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PaintStroke {
    pub dabs: Vec<BrushDab>,
    pub subtract: bool,
}

impl PaintStroke {
    pub fn new(point: [f32; 2], pressure: f32, subtract: bool) -> Self {
        Self {
            dabs: vec![BrushDab {
                point,
                pressure: pressure.clamp(0.0, 1.0),
            }],
            subtract,
        }
    }

    /// Insert evenly spaced dabs rather than depending on pointer event rate.
    /// A short release segment can be included with `finish`.
    pub fn extend(
        &mut self,
        point: [f32; 2],
        pressure: f32,
        settings: BrushSettings,
        image_size: [u32; 2],
        finish: bool,
    ) {
        let Some(last) = self.dabs.last().copied() else {
            return;
        };
        if !point.iter().all(|v| v.is_finite()) {
            return;
        }
        let short = image_size[0].min(image_size[1]).max(1) as f32;
        let metric = [image_size[0] as f32 / short, image_size[1] as f32 / short];
        let dx = point[0] - last.point[0];
        let dy = point[1] - last.point[1];
        let distance = (dx * metric[0]).hypot(dy * metric[1]);
        let pressure = pressure.clamp(0.0, 1.0);
        let pressure_size = if settings.pressure {
            last.pressure.min(pressure).max(0.05)
        } else {
            1.0
        };
        let step = (settings.size * 2.0 * settings.spacing * pressure_size).max(0.00001);
        let count = (distance / step).floor() as usize;
        // Bound one event's work as well as malformed imported brush data.
        for i in 1..=count.min(16384) {
            let t = i as f32 * step / distance;
            self.dabs.push(BrushDab {
                point: [last.point[0] + dx * t, last.point[1] + dy * t],
                pressure: last.pressure + (pressure - last.pressure) * t,
            });
        }
        if finish && distance > 1e-7 && self.dabs.last().is_some_and(|dab| dab.point != point) {
            self.dabs.push(BrushDab { point, pressure });
        }
    }
}

/// Portable local-adjustment recipe. Applying a recipe preserves the current
/// selection instead of carrying an unrelated subject's pixels to a new photo.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LocalPreset {
    pub version: u32,
    pub name: String,
    pub adjustment: crate::edit::LocalAdjustment,
}

impl LocalPreset {
    pub fn from_adjustment(
        name: impl Into<String>,
        adjustment: &crate::edit::LocalAdjustment,
    ) -> Self {
        let mut adjustment = adjustment.clone();
        adjustment.mask = crate::edit::LocalAdjustment::default().mask;
        adjustment.mask_inverted = false;
        Self {
            version: 1,
            name: name.into(),
            adjustment,
        }
    }

    pub fn apply(&self, target: &mut crate::edit::LocalAdjustment) -> bool {
        if self.version != 1 {
            return false;
        }
        let name = target.name.clone();
        let mask = target.mask.clone();
        let mask_inverted = target.mask_inverted;
        let enabled = target.enabled;
        let mut edit = crate::EditState::default();
        edit.local_adjustments.push(self.adjustment.clone());
        *target = edit.clamped().local_adjustments.remove(0);
        target.name = name;
        target.mask = mask;
        target.mask_inverted = mask_inverted;
        target.enabled = enabled;
        true
    }
}

#[cfg(test)]
mod brush_tests {
    use super::*;
    #[test]
    fn paint_sidecar_retains_per_dab_pressure_and_erase() {
        let mut edit = crate::EditState::default();
        edit.local_adjustments.push(crate::edit::LocalAdjustment {
            mask: crate::edit::LocalMask::Paint {
                strokes: vec![
                    PaintStroke::new([0.2, 0.3], 0.4, false),
                    PaintStroke::new([0.7, 0.6], 0.9, true),
                ],
                settings: BrushSettings {
                    flow: 0.25,
                    spacing: 0.3,
                    smoothing: 0.7,
                    ..Default::default()
                },
            },
            ..Default::default()
        });
        let json = serde_json::to_string(&edit).unwrap();
        let loaded: crate::EditState = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.clamped(), edit);
    }

    #[test]
    fn brush_spacing_is_in_pixel_space_and_independent_of_event_count() {
        let settings = BrushSettings {
            size: 0.05,
            spacing: 0.5,
            pressure: false,
            ..Default::default()
        };
        let mut sparse = PaintStroke::new([0.1, 0.5], 1.0, false);
        sparse.extend([0.9, 0.5], 1.0, settings, [200, 100], true);
        let mut dense = PaintStroke::new([0.1, 0.5], 1.0, false);
        for i in 1..=16 {
            dense.extend(
                [0.1 + i as f32 * 0.05, 0.5],
                1.0,
                settings,
                [200, 100],
                i == 16,
            );
        }
        assert!(sparse.dabs.len().abs_diff(dense.dabs.len()) <= 1);
        for pair in sparse.dabs.windows(2).take(sparse.dabs.len() - 2) {
            assert!(((pair[1].point[0] - pair[0].point[0]) * 2.0 - 0.05).abs() < 1e-5);
        }
    }
    #[test]
    fn local_preset_preserves_selection_identity_and_rejects_future_schema() {
        let source = crate::edit::LocalAdjustment {
            temperature: 30.0,
            local: crate::edit::LocalContrastParams {
                texture: 25.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let preset = LocalPreset::from_adjustment("Warm detail", &source);
        let mut target = crate::edit::LocalAdjustment {
            name: "Subject".into(),
            mask_inverted: true,
            mask: crate::edit::LocalMask::Radial {
                center: [0.3, 0.4],
                radius: [0.2, 0.1],
                feather: 0.3,
            },
            ..Default::default()
        };
        let mask = target.mask.clone();
        assert!(preset.apply(&mut target));
        assert_eq!(target.mask, mask);
        assert!(target.mask_inverted);
        assert_eq!(target.name, "Subject");
        assert_eq!(target.temperature, 30.0);
        let before = target.clone();
        let future = LocalPreset {
            version: 2,
            ..preset
        };
        assert!(!future.apply(&mut target));
        assert_eq!(target, before);
    }
}

/// Joint bilateral alpha refinement against real scene colors. The output is a
/// bounded preview matte (at most 1536 pixels on the long edge), preserving soft
/// coverage while reducing spill across visible color/luminance boundaries.
/// This is photographic edge guidance, not semantic segmentation or hair inference.
pub fn refine_guided(
    mask: &AlphaMask,
    guide: &crate::source::RasterImage,
    radius: f32,
    tolerance: f32,
) -> Result<AlphaMask, String> {
    if !mask.valid()
        || guide.width == 0
        || guide.height == 0
        || guide.linear_rec2020.len() != guide.width as usize * guide.height as usize
    {
        return Err("A valid mask and guidance image are required".into());
    }
    let scale = (1536.0 / mask.width.max(mask.height) as f32).min(1.0);
    let width = (mask.width as f32 * scale).round().max(1.0) as usize;
    let height = (mask.height as f32 * scale).round().max(1.0) as usize;
    let sampler = mask.sampler();
    let mut alpha = Vec::with_capacity(width * height);
    let mut colors = Vec::with_capacity(width * height);
    for y in 0..height {
        for x in 0..width {
            let uv = [
                (x as f32 + 0.5) / width as f32,
                (y as f32 + 0.5) / height as f32,
            ];
            alpha.push(sampler.sample(uv));
            let readout = crate::analysis::sample(guide, uv).ok_or("Invalid guidance sample")?;
            // Compressive positive RGB distances tolerate HDR scene values.
            colors.push(readout.rgb.0.map(|v| v.max(0.0).sqrt()));
        }
    }
    let radius = radius.clamp(1.0, 12.0);
    let sigma = tolerance.clamp(0.005, 0.5);
    let mut output = Vec::with_capacity(width * height);
    for y in 0..height {
        for x in 0..width {
            let center = colors[y * width + x];
            let mut sum = 0.0;
            let mut weight = 0.0;
            for dy in -2i32..=2 {
                for dx in -2i32..=2 {
                    let nx = (x as i32 + (dx as f32 * radius * 0.5).round() as i32)
                        .clamp(0, width as i32 - 1) as usize;
                    let ny = (y as i32 + (dy as f32 * radius * 0.5).round() as i32)
                        .clamp(0, height as i32 - 1) as usize;
                    let next = colors[ny * width + nx];
                    let distance = center
                        .iter()
                        .zip(next)
                        .map(|(a, b)| (a - b).powi(2))
                        .sum::<f32>();
                    let w = (-0.5 * distance / (sigma * sigma) - 0.25 * (dx * dx + dy * dy) as f32)
                        .exp();
                    sum += alpha[ny * width + nx] * w;
                    weight += w;
                }
            }
            output.push(
                (sum / weight.max(1e-12) * 65535.0)
                    .round()
                    .clamp(0.0, 65535.0) as u16,
            );
        }
    }
    Ok(AlphaMask {
        width: width as u32,
        height: height as u32,
        alpha: output.into(),
        source: format!("Color-guided edge refinement · {width}×{height}"),
        inverted: false,
        repairs: vec![],
    })
}

#[cfg(test)]
mod refinement_tests {
    use super::*;
    #[test]
    fn guided_refinement_reduces_spill_without_destroying_soft_coverage() {
        let width = 64;
        let height = 32;
        let guide = crate::source::RasterImage {
            width,
            height,
            linear_rec2020: (0..width * height)
                .map(|i| {
                    if i % width < 32 {
                        [0.1, 0.2, 0.1, 1.0]
                    } else {
                        [0.8, 0.5, 0.1, 1.0]
                    }
                })
                .collect(),
            bits_per_channel: 16,
            embedded_icc: None,
        };
        let mask = AlphaMask {
            width,
            height,
            alpha: (0..width * height)
                .map(|i| {
                    ((0.5 + (31.5 - (i % width) as f32) / 12.0).clamp(0.0, 1.0) * 65535.0) as u16
                })
                .collect(),
            source: "test".into(),
            inverted: false,
            repairs: vec![],
        };
        let output = refine_guided(&mask, &guide, 6.0, 0.05).unwrap();
        assert!(output.alpha[16 * 64 + 31] > mask.alpha[16 * 64 + 31] + 5000);
        assert!(output.alpha[16 * 64 + 32] + 5000 < mask.alpha[16 * 64 + 32]);
        let flat = AlphaMask {
            alpha: vec![20000; 64 * 32].into(),
            ..mask
        };
        let unchanged = refine_guided(&flat, &guide, 6.0, 0.05).unwrap();
        assert!(unchanged.alpha.iter().all(|a| a.abs_diff(20000) <= 1));
    }
}

/// Resident 16-bit alpha bytes for one edit, counting shared undo/duplicate
/// storage once within this edit. Does not include other history snapshots.
pub fn alpha_storage_bytes(edit: &crate::EditState) -> u64 {
    fn visit(
        mask: &crate::edit::LocalMask,
        seen: &mut std::collections::HashSet<usize>,
        depth: usize,
    ) -> u64 {
        use crate::edit::LocalMask;
        if depth >= 32 {
            return 0;
        }
        match mask {
            LocalMask::Alpha(alpha) => {
                if seen.insert(alpha.alpha.as_ptr() as usize) {
                    alpha.alpha.len() as u64 * 2
                } else {
                    0
                }
            }
            LocalMask::Composite { left, right, .. } => {
                visit(left, seen, depth + 1) + visit(right, seen, depth + 1)
            }
            LocalMask::Modified { mask, .. } => visit(mask, seen, depth + 1),
            _ => 0,
        }
    }
    let mut seen = std::collections::HashSet::new();
    edit.local_adjustments
        .iter()
        .map(|a| visit(&a.mask, &mut seen, 0))
        .sum()
}
