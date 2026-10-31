//! Registered scene-linear photographic merges. Registration supports small
//! translations, rotations and scale changes; it rejects unrelated views.
use crate::source::RasterImage;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MergeKind {
    Hdr,
    Burst,
    Focus,
    Panorama,
    Astro,
    PixelShift,
}

impl MergeKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Hdr => "HDR",
            Self::Burst => "Burst average",
            Self::Focus => "Focus stack",
            Self::Panorama => "Panorama",
            Self::Astro => "Astro stack",
            Self::PixelShift => "Pixel shift (expert)",
        }
    }
    pub fn suffix(self) -> &'static str {
        match self {
            Self::Hdr => "hdr",
            Self::Burst => "burst",
            Self::Focus => "focus",
            Self::Panorama => "panorama",
            Self::Astro => "astro",
            Self::PixelShift => "pixel-shift",
        }
    }
}

/// Maps reference pixel coordinates into a source, about the image center.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Registration {
    pub dx: f32,
    pub dy: f32,
    pub rotation_radians: f32,
    pub scale: f32,
    pub correlation: f32,
}

impl Default for Registration {
    fn default() -> Self {
        Self {
            dx: 0.0,
            dy: 0.0,
            rotation_radians: 0.0,
            scale: 1.0,
            correlation: 1.0,
        }
    }
}

impl Registration {
    pub fn point(self, x: f32, y: f32, width: u32, height: u32) -> (f32, f32) {
        let (cx, cy) = ((width - 1) as f32 * 0.5, (height - 1) as f32 * 0.5);
        let (s, c) = self.rotation_radians.sin_cos();
        (
            cx + self.scale * (c * (x - cx) - s * (y - cy)) + self.dx,
            cy + self.scale * (s * (x - cx) + c * (y - cy)) + self.dy,
        )
    }
}

/// Explicit source choice in normalized, uncropped reference coordinates.
/// Later regions blend over earlier ones; radii are half-width/half-height.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FocusRepair {
    pub source: usize,
    pub center: [f32; 2],
    pub radii: [f32; 2],
    pub feather: f32,
}
impl FocusRepair {
    fn coverage(&self, x: f32, y: f32) -> f32 {
        let distance = (((x - self.center[0]) / self.radii[0]).powi(2)
            + ((y - self.center[1]) / self.radii[1]).powi(2))
        .sqrt();
        let t = ((1.0 - distance) / self.feather.max(0.001)).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    }
}

#[derive(Debug, Clone)]
pub struct MergeOptions {
    pub kind: MergeKind,
    pub reference: usize,
    /// Exposure stops relative to any common origin, one per HDR input.
    pub exposure_ev: Vec<f32>,
    /// Relative difference above which moving samples are rejected.
    pub motion_threshold: f32,
    pub align: bool,
    pub focus_repairs: Vec<FocusRepair>,
}

#[derive(Debug)]
pub struct MergeResult {
    pub image: RasterImage,
    pub registrations: Vec<Registration>,
    /// Upright reference pixels removed at left, top; final width and height.
    pub crop: [u32; 4],
    pub rejected_samples: u64,
}

fn luma(pixel: [f32; 4]) -> f32 {
    (pixel[0] * 0.2627 + pixel[1] * 0.6780 + pixel[2] * 0.0593).max(0.0)
}

pub fn sample(image: &RasterImage, x: f32, y: f32) -> [f32; 4] {
    let x = x.clamp(0.0, (image.width - 1) as f32);
    let y = y.clamp(0.0, (image.height - 1) as f32);
    let (x0, y0) = (x as u32, y as u32);
    let (x1, y1) = (
        (x0 + 1).min(image.width - 1),
        (y0 + 1).min(image.height - 1),
    );
    let (fx, fy) = (x - x0 as f32, y - y0 as f32);
    [0, 1, 2, 3].map(|c| {
        let top = image.linear_rec2020[(y0 * image.width + x0) as usize][c] * (1.0 - fx)
            + image.linear_rec2020[(y0 * image.width + x1) as usize][c] * fx;
        let bottom = image.linear_rec2020[(y1 * image.width + x0) as usize][c] * (1.0 - fx)
            + image.linear_rec2020[(y1 * image.width + x1) as usize][c] * fx;
        top * (1.0 - fy) + bottom * fy
    })
}

struct Gray {
    width: usize,
    height: usize,
    pixels: Vec<f32>,
}

impl Gray {
    fn from_image(image: &RasterImage, edge: usize) -> Self {
        let factor = (image.width.max(image.height) as usize)
            .div_ceil(edge)
            .max(1);
        let (width, height) = (
            image.width as usize / factor,
            image.height as usize / factor,
        );
        let mut pixels = Vec::with_capacity(width * height);
        for y in 0..height {
            for x in 0..width {
                let mut sum = 0.0;
                for yy in y * factor..(y + 1) * factor {
                    for xx in x * factor..(x + 1) * factor {
                        sum += luma(image.linear_rec2020[yy * image.width as usize + xx]);
                    }
                }
                pixels.push((0.001 + sum / (factor * factor) as f32).ln());
            }
        }
        Self {
            width,
            height,
            pixels,
        }
    }

    fn half(&self) -> Self {
        let (width, height) = (self.width / 2, self.height / 2);
        let pixels = (0..height)
            .flat_map(|y| {
                (0..width).map(move |x| {
                    (self.pixels[(y * 2) * self.width + x * 2]
                        + self.pixels[(y * 2) * self.width + x * 2 + 1]
                        + self.pixels[(y * 2 + 1) * self.width + x * 2]
                        + self.pixels[(y * 2 + 1) * self.width + x * 2 + 1])
                        * 0.25
                })
            })
            .collect();
        Self {
            width,
            height,
            pixels,
        }
    }

    fn at(&self, x: f32, y: f32) -> f32 {
        let (ix, iy) = (x as usize, y as usize);
        let (fx, fy) = (x - ix as f32, y - iy as f32);
        let a = self.pixels[iy * self.width + ix] * (1.0 - fx)
            + self.pixels[iy * self.width + (ix + 1).min(self.width - 1)] * fx;
        let b = self.pixels[(iy + 1).min(self.height - 1) * self.width + ix] * (1.0 - fx)
            + self.pixels
                [(iy + 1).min(self.height - 1) * self.width + (ix + 1).min(self.width - 1)]
                * fx;
        a * (1.0 - fy) + b * fy
    }
}

fn correlation(reference: &Gray, source: &Gray, transform: Registration) -> f32 {
    let mut count = 0.0f64;
    let (mut sum_a, mut sum_b, mut aa, mut bb, mut ab) = (0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64);
    let step = (reference.width.max(reference.height) / 160).max(1);
    for y in (2..reference.height.saturating_sub(2)).step_by(step) {
        for x in (2..reference.width.saturating_sub(2)).step_by(step) {
            let (sx, sy) = transform.point(
                x as f32,
                y as f32,
                source.width as u32,
                source.height as u32,
            );
            if sx < 1.0
                || sy < 1.0
                || sx >= (source.width - 2) as f32
                || sy >= (source.height - 2) as f32
            {
                continue;
            }
            let a = reference.pixels[y * reference.width + x] as f64;
            let b = source.at(sx, sy) as f64;
            count += 1.0;
            sum_a += a;
            sum_b += b;
            aa += a * a;
            bb += b * b;
            ab += a * b;
        }
    }
    if count < 64.0 {
        return -1.0;
    }
    let variance = (aa - sum_a * sum_a / count) * (bb - sum_b * sum_b / count);
    if variance < 1e-10 {
        return -1.0;
    }
    ((ab - sum_a * sum_b / count) / variance.sqrt()) as f32
}

/// Exposure-insensitive pyramid correlation. Intended for approximately
/// matching views (tripod or small handheld offsets), not panorama alignment.
pub fn register(
    reference: &RasterImage,
    source: &RasterImage,
    cancel: &AtomicBool,
) -> Result<Registration, String> {
    if !reference.has_finite_opaque_pixels() || !source.has_finite_opaque_pixels() {
        return Err("Registration requires complete finite opaque source pixels".into());
    }
    if (reference.width, reference.height) != (source.width, source.height) {
        return Err("Merge sources must have equal upright dimensions".into());
    }
    if reference.width.min(reference.height) < 32 {
        return Err("Registration needs photographs at least 32 pixels wide and high".into());
    }
    let mut a = vec![Gray::from_image(reference, 768)];
    let mut b = vec![Gray::from_image(source, 768)];
    while a.last().unwrap().width.max(a.last().unwrap().height) > 80
        && a.last().unwrap().width.min(a.last().unwrap().height) >= 32
    {
        a.push(a.last().unwrap().half());
        b.push(b.last().unwrap().half());
    }
    let mut transform = Registration::default();
    for level in (0..a.len()).rev() {
        if cancel.load(Ordering::Relaxed) {
            return Err("Merge cancelled".into());
        }
        if level == a.len() - 1 {
            let mut best = -1.0;
            for dy in -8..=8 {
                for dx in -8..=8 {
                    let candidate = Registration {
                        dx: dx as f32,
                        dy: dy as f32,
                        ..Default::default()
                    };
                    let score = correlation(&a[level], &b[level], candidate);
                    if score > best {
                        best = score;
                        transform = candidate;
                    }
                }
            }
        } else {
            transform.dx *= 2.0;
            transform.dy *= 2.0;
        }
        for refinement in 0..4 {
            let step = 2.0f32.powi(-refinement);
            for _ in 0..12 {
                let before = correlation(&a[level], &b[level], transform);
                let mut best = before;
                let mut candidate_best = transform;
                for axis in 0..4 {
                    for direction in [-1.0, 1.0] {
                        let mut candidate = transform;
                        match axis {
                            0 => candidate.dx += direction * step,
                            1 => candidate.dy += direction * step,
                            2 => candidate.rotation_radians += direction * step * 0.01,
                            _ => candidate.scale += direction * step * 0.01,
                        };
                        if candidate.rotation_radians.abs() > 0.08
                            || !(0.94..=1.06).contains(&candidate.scale)
                        {
                            continue;
                        }
                        let score = correlation(&a[level], &b[level], candidate);
                        if score > best {
                            best = score;
                            candidate_best = candidate;
                        }
                    }
                }
                transform = candidate_best;
                if best - before < 1e-6 {
                    break;
                }
            }
        }
    }
    transform.correlation = correlation(&a[0], &b[0], transform);
    if transform.correlation < 0.65 {
        return Err(format!("Sources could not be aligned reliably (correlation {:.2}); use matching views or disable alignment for a locked tripod",transform.correlation));
    }
    let factor = (reference.width.max(reference.height) as usize)
        .div_ceil(768)
        .max(1) as f32;
    transform.dx *= factor;
    transform.dy *= factor;
    Ok(transform)
}

fn focus(image: &RasterImage, x: f32, y: f32) -> f32 {
    let center = luma(sample(image, x, y));
    let mut score = 0.0;
    for radius in [1.0, 2.0] {
        let laplacian = 4.0 * center
            - luma(sample(image, x - radius, y))
            - luma(sample(image, x + radius, y))
            - luma(sample(image, x, y - radius))
            - luma(sample(image, x, y + radius));
        score += laplacian.abs();
    }
    score
}

pub fn merge(
    images: &[RasterImage],
    options: &MergeOptions,
    cancel: &AtomicBool,
    mut progress: impl FnMut(f32),
) -> Result<MergeResult, String> {
    if matches!(options.kind, MergeKind::Panorama | MergeKind::PixelShift) {
        return Err(
            "Panorama and pixel-shift inputs require their dedicated reconstruction engines".into(),
        );
    }
    if !(2..=16).contains(&images.len()) || options.reference >= images.len() {
        return Err("Select 2–16 photographs and a valid reference".into());
    }
    if options.kind == MergeKind::Astro
        && (images.len() < 3
            || !options.motion_threshold.is_finite()
            || !(1.0..=10.0).contains(&options.motion_threshold))
    {
        return Err(
            "Astro stacking needs at least three lights and a sigma threshold from 1 to 10".into(),
        );
    }
    if options.focus_repairs.len() > 128
        || options.focus_repairs.iter().any(|repair| {
            repair.source >= images.len()
                || repair
                    .center
                    .iter()
                    .any(|v| !v.is_finite() || !(0.0..=1.0).contains(v))
                || repair
                    .radii
                    .iter()
                    .any(|v| !v.is_finite() || *v <= 0.0 || *v > 2.0)
                || !repair.feather.is_finite()
                || !(0.0..=1.0).contains(&repair.feather)
        })
    {
        return Err(
            "Focus repair needs a valid source, center, radii and feather (maximum 128 regions)"
                .into(),
        );
    }
    let reference = &images[options.reference];
    let (width, height) = (reference.width, reference.height);
    if width < 8
        || height < 8
        || images.iter().any(|image| {
            image.width != width || image.height != height || !image.has_finite_opaque_pixels()
        })
    {
        return Err("Merge sources need equal dimensions and finite opaque RGB pixels".into());
    }
    if options.kind == MergeKind::Hdr
        && (options.exposure_ev.len() != images.len()
            || options
                .exposure_ev
                .iter()
                .any(|v| !v.is_finite() || v.abs() > 30.0))
    {
        return Err("Supply an exposure value for every HDR frame".into());
    }
    let mut registrations = Vec::with_capacity(images.len());
    for (index, image) in images.iter().enumerate() {
        registrations.push(if options.align && index != options.reference {
            register(reference, image, cancel)?
        } else {
            Registration::default()
        });
        progress(0.3 * (index + 1) as f32 / images.len() as f32);
    }
    // Conservative common rectangle: all four corners must be inside each
    // affine source footprint. This guarantees opaque pixels without fill.
    let mut border = 0u32;
    let valid = |x: f32, y: f32| {
        registrations.iter().all(|t| {
            let (sx, sy) = t.point(x, y, width, height);
            sx >= 0.0 && sy >= 0.0 && sx <= (width - 1) as f32 && sy <= (height - 1) as f32
        })
    };
    while border * 2 + 8 < width.min(height)
        && ![
            (border, border),
            (width - 1 - border, border),
            (border, height - 1 - border),
            (width - 1 - border, height - 1 - border),
        ]
        .into_iter()
        .all(|(x, y)| valid(x as f32, y as f32))
    {
        border += 1;
    }
    if border * 2 + 8 >= width.min(height) {
        return Err("Aligned sources have too little common area".into());
    }
    let (out_width, out_height) = (width - border * 2, height - border * 2);
    let mut pixels = Vec::with_capacity(out_width as usize * out_height as usize);
    let mut rejected_samples = 0;
    let mut sampled = vec![[0.0; 4]; images.len()];
    let mut focus_scores = vec![0.0f32; images.len()];
    for y in border..height - border {
        if cancel.load(Ordering::Relaxed) {
            return Err("Merge cancelled".into());
        }
        for x in border..width - border {
            for (i, image) in images.iter().enumerate() {
                let (sx, sy) = registrations[i].point(x as f32, y as f32, width, height);
                sampled[i] = sample(image, sx, sy);
                if options.kind == MergeKind::Focus {
                    // Averaging a neighborhood stabilizes low-contrast focus
                    // choices; exponent weighting feathers transitions.
                    focus_scores[i] =
                        [(0.0, 0.0), (-2.0, 0.0), (2.0, 0.0), (0.0, -2.0), (0.0, 2.0)]
                            .iter()
                            .map(|(dx, dy)| focus(image, sx + dx, sy + dy))
                            .sum::<f32>()
                            / 5.0;
                }
            }
            if options.kind == MergeKind::Astro {
                let mut medians = [0.0; 3];
                let mut sigmas = [0.0; 3];
                for c in 0..3 {
                    let mut scratch = [0.0f32; 16];
                    let values = &mut scratch[..sampled.len()];
                    for (v, p) in values.iter_mut().zip(&sampled) {
                        *v = p[c];
                    }
                    values.sort_by(f32::total_cmp);
                    medians[c] = values[values.len() / 2];
                    for v in values.iter_mut() {
                        *v = (*v - medians[c]).abs();
                    }
                    values.sort_by(f32::total_cmp);
                    sigmas[c] = (values[values.len() / 2] * 1.4826).max(1e-5);
                }
                let mut sum = [0.0; 3];
                let mut count = 0;
                for pixel in &sampled {
                    if (0..3).any(|c| {
                        (pixel[c] - medians[c]).abs() > options.motion_threshold * sigmas[c]
                    }) {
                        rejected_samples += 1;
                        continue;
                    }
                    for c in 0..3 {
                        sum[c] += pixel[c];
                    }
                    count += 1;
                }
                pixels.push(if count > 0 {
                    [
                        sum[0] / count as f32,
                        sum[1] / count as f32,
                        sum[2] / count as f32,
                        1.0,
                    ]
                } else {
                    [medians[0], medians[1], medians[2], 1.0]
                });
                continue;
            }
            let anchor = sampled[options.reference];
            let mut sum = [0.0f32; 3];
            let mut weights = 0.0f32;
            let max_focus = focus_scores
                .iter()
                .copied()
                .fold(0.0f32, f32::max)
                .max(1e-8);
            for (i, original) in sampled.iter().copied().enumerate() {
                let factor = if options.kind == MergeKind::Hdr {
                    2.0f32.powf(options.exposure_ev[options.reference] - options.exposure_ev[i])
                } else {
                    1.0
                };
                let normalized = original.map(|v| v * factor);
                let mut weight = if options.kind == MergeKind::Focus {
                    // Equal coverage in truly flat areas. Ratios of rounding
                    // noise must not become an arbitrary winner after powi(8).
                    if max_focus <= 1e-5 * luma(anchor).abs().max(1.0) {
                        1.0
                    } else {
                        (focus_scores[i] / max_focus).powi(8) + 1e-8
                    }
                } else if options.kind == MergeKind::Hdr {
                    let value = original[..3].iter().copied().fold(0.0f32, f32::max);
                    (value - 0.003).min(0.98 - value).max(0.0).powi(2)
                } else {
                    1.0
                };
                if options.kind != MergeKind::Focus && i != options.reference {
                    let difference = (0..3)
                        .map(|c| (normalized[c] - anchor[c]).abs())
                        .fold(0.0f32, f32::max);
                    let anchor_valid = options.kind != MergeKind::Hdr
                        || anchor[..3].iter().all(|v| *v > 0.008 && *v < 0.96);
                    if anchor_valid
                        && difference
                            > 0.015 + options.motion_threshold.max(0.0) * luma(anchor).max(0.05)
                    {
                        weight = 0.0;
                        rejected_samples += 1;
                    }
                }
                for c in 0..3 {
                    sum[c] += normalized[c] * weight;
                }
                weights += weight;
            }
            let mut result = if weights > 1e-12 {
                [sum[0] / weights, sum[1] / weights, sum[2] / weights, 1.0]
            } else {
                anchor
            };
            if options.kind == MergeKind::Focus {
                for repair in &options.focus_repairs {
                    let coverage = repair.coverage(
                        x as f32 / (width - 1) as f32,
                        y as f32 / (height - 1) as f32,
                    );
                    let source = sampled[repair.source];
                    for c in 0..3 {
                        result[c] = result[c] * (1.0 - coverage) + source[c] * coverage;
                    }
                }
            }
            pixels.push(result);
        }
        if (y - border).is_multiple_of(32) {
            progress(0.3 + 0.7 * (y - border + 1) as f32 / out_height as f32);
        }
    }
    progress(1.0);
    Ok(MergeResult {
        image: RasterImage {
            width: out_width,
            height: out_height,
            linear_rec2020: pixels,
            bits_per_channel: 32,
            embedded_icc: None,
        },
        registrations,
        crop: [border, border, out_width, out_height],
        rejected_samples,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registration_rejects_incomplete_nonfinite_and_transparent_frames() {
        let cancel = AtomicBool::new(false);
        let reference = image(32, 32, |x, y| (x + y) as f32 / 100.0);
        let mut broken = reference.clone();
        broken.linear_rec2020.pop();
        assert!(register(&reference, &broken, &cancel).is_err());
        assert!(register(&broken, &reference, &cancel).is_err());
        broken = reference.clone();
        broken.linear_rec2020[0][0] = f32::NAN;
        assert!(register(&reference, &broken, &cancel).is_err());
        broken = reference.clone();
        broken.linear_rec2020[0][3] = 0.5;
        assert!(register(&reference, &broken, &cancel).is_err());
    }

    fn image(width: u32, height: u32, mut pixel: impl FnMut(u32, u32) -> f32) -> RasterImage {
        RasterImage {
            width,
            height,
            linear_rec2020: (0..height)
                .flat_map(|y| (0..width).map(|x| (x, y)).collect::<Vec<_>>())
                .map(|(x, y)| {
                    let p = pixel(x, y);
                    [p, p, p, 1.0]
                })
                .collect(),
            bits_per_channel: 32,
            embedded_icc: None,
        }
    }
    fn scene(x: f32, y: f32) -> f32 {
        0.4 + 0.08 * (x * 0.13).sin()
            + 0.09 * (y * 0.21).cos()
            + 0.12 * ((x + y) * 0.31).sin()
            + if x > 30.0 && x < 57.0 && y > 17.0 && y < 48.0 {
                0.15
            } else {
                0.0
            }
    }
    fn options(kind: MergeKind) -> MergeOptions {
        MergeOptions {
            kind,
            reference: 0,
            exposure_ev: vec![],
            motion_threshold: 0.15,
            align: false,
            focus_repairs: Vec::new(),
        }
    }

    #[test]
    fn registration_recovers_subpixel_shift_despite_exposure_change() {
        let a = image(128, 96, |x, y| scene(x as f32, y as f32));
        let b = image(128, 96, |x, y| scene(x as f32 - 3.25, y as f32 + 2.0) * 1.7);
        let result = register(&a, &b, &AtomicBool::new(false)).unwrap();
        assert!((result.dx - 3.25).abs() < 0.5, "{result:?}");
        assert!((result.dy + 2.0).abs() < 0.5, "{result:?}");
        assert!(result.correlation > 0.98);
    }

    #[test]
    fn registration_recovers_rotation_and_focus_breathing_scale() {
        let a = image(128, 96, |x, y| scene(x as f32, y as f32));
        let expected = Registration {
            rotation_radians: 0.023,
            scale: 1.025,
            dx: 1.5,
            dy: -1.0,
            correlation: 1.0,
        };
        let b = image(128, 96, |x, y| {
            let (cx, cy) = (63.5, 47.5);
            let (s, c) = expected.rotation_radians.sin_cos();
            let (xx, yy) = (
                (x as f32 - cx - expected.dx) / expected.scale,
                (y as f32 - cy - expected.dy) / expected.scale,
            );
            scene(cx + c * xx + s * yy, cy - s * xx + c * yy)
        });
        let result = register(&a, &b, &AtomicBool::new(false)).unwrap();
        assert!(
            (result.rotation_radians - expected.rotation_radians).abs() < 0.008,
            "{result:?}"
        );
        assert!((result.scale - expected.scale).abs() < 0.01, "{result:?}");
    }

    #[test]
    fn focus_source_region_overrides_registered_samples_with_feather() {
        let a = image(33, 33, |_, _| 0.2);
        let b = image(33, 33, |_, _| 0.8);
        let mut settings = options(MergeKind::Focus);
        settings.focus_repairs.push(FocusRepair {
            source: 1,
            center: [0.5, 0.5],
            radii: [0.4, 0.4],
            feather: 0.5,
        });
        let result = merge(&[a, b], &settings, &AtomicBool::new(false), |_| {}).unwrap();
        let at = |x: usize, y: usize| result.image.linear_rec2020[y * 33 + x][0];
        assert!((at(16, 16) - 0.8).abs() < 1e-5);
        assert!((at(0, 0) - 0.5).abs() < 1e-5);
        assert!(at(25, 16) > 0.5 && at(25, 16) < 0.8);
        settings.focus_repairs[0].source = 5;
        let frames = [image(16, 16, |_, _| 0.2), image(16, 16, |_, _| 0.8)];
        assert!(merge(&frames, &settings, &AtomicBool::new(false), |_| {}).is_err());
    }

    #[test]
    fn astro_sigma_clipping_removes_transients_without_reference_bias() {
        let frames: Vec<_> = [-0.002, 0.0, 0.001, 0.002, 0.4]
            .into_iter()
            .map(|noise| image(16, 16, |_, _| 0.1 + noise))
            .collect();
        let mut settings = options(MergeKind::Astro);
        settings.motion_threshold = 3.0;
        settings.reference = 4;
        let result = merge(&frames, &settings, &AtomicBool::new(false), |_| {}).unwrap();
        assert!((result.image.linear_rec2020[0][0] - 0.10025).abs() < 1e-6);
        assert_eq!(result.rejected_samples, 256);
        assert!(merge(&frames[..2], &settings, &AtomicBool::new(false), |_| {}).is_err());
    }
    #[test]
    fn sparse_star_field_recovers_dither_and_rejects_unrelated_sky() {
        let stars = |x: f32, y: f32| {
            let mut flux = 0.004;
            for (cx, cy, brightness) in [
                (18.0, 22.0, 0.3),
                (35.0, 62.0, 0.15),
                (75.0, 31.0, 0.5),
                (102.0, 74.0, 0.4),
                (50.0, 45.0, 0.2),
            ] {
                flux += brightness * (-((x - cx).powi(2) + (y - cy).powi(2)) / 3.0).exp();
            }
            flux
        };
        let a = image(128, 96, |x, y| stars(x as f32, y as f32));
        let b = image(128, 96, |x, y| stars(x as f32 - 2.25, y as f32 + 1.5));
        let registration = register(&a, &b, &AtomicBool::new(false)).unwrap();
        assert!((registration.dx - 2.25).abs() < 0.3, "{registration:?}");
        assert!((registration.dy + 1.5).abs() < 0.3, "{registration:?}");
    }

    #[test]
    fn hdr_recovers_highlight_radiance_from_short_exposure() {
        let a = image(16, 16, |x, _| if x > 7 { 1.0 } else { 0.2 });
        let b = image(16, 16, |x, _| if x > 7 { 0.5 } else { 0.05 });
        let mut options = options(MergeKind::Hdr);
        options.exposure_ev = vec![0.0, -2.0];
        let merged = merge(&[a, b], &options, &AtomicBool::new(false), |_| {}).unwrap();
        assert!((merged.image.linear_rec2020[10][0] - 2.0).abs() < 1e-5);
        assert!((merged.image.linear_rec2020[2][0] - 0.2).abs() < 1e-5);
    }

    #[test]
    fn burst_averages_noise_but_rejects_moving_object() {
        let frames: Vec<_> = (0..4)
            .map(|i| {
                image(32, 24, |x, y| {
                    if i == 3 && x > 20 {
                        return 0.8;
                    }
                    0.2 + if ((x * 13 + y * 7 + i * 19) % 31) < 15 {
                        0.006
                    } else {
                        -0.006
                    }
                })
            })
            .collect();
        let merged = merge(
            &frames,
            &options(MergeKind::Burst),
            &AtomicBool::new(false),
            |_| {},
        )
        .unwrap();
        let variance = merged
            .image
            .linear_rec2020
            .iter()
            .map(|p| (p[0] - 0.2).powi(2))
            .sum::<f32>()
            / 768.0;
        assert!(variance < 0.006f32.powi(2) * 0.5, "{variance}");
        assert!(merged.rejected_samples > 0);
        assert!(merged.image.linear_rec2020.iter().all(|p| p[0] < 0.22));
    }

    #[test]
    fn focus_stack_selects_sharp_regions_from_both_frames() {
        let value = |x: u32, y: u32| {
            if (x / 2 + y / 2).is_multiple_of(2) {
                0.2
            } else {
                0.7
            }
        };
        let a = image(48, 32, |x, y| if x < 24 { value(x, y) } else { 0.45 });
        let b = image(48, 32, |x, y| if x >= 24 { value(x, y) } else { 0.45 });
        let merged = merge(
            &[a, b],
            &options(MergeKind::Focus),
            &AtomicBool::new(false),
            |_| {},
        )
        .unwrap();
        for x in [8, 36] {
            assert!(
                (merged.image.linear_rec2020[12 * 48 + x][0] - value(x as u32, 12)).abs() < 0.01
            );
        }
    }

    #[test]
    fn rejects_untextured_alignment_and_honors_cancellation() {
        let a = image(64, 64, |_, _| 0.2);
        let b = a.clone();
        assert!(register(&a, &b, &AtomicBool::new(false)).is_err());
        assert!(merge(
            &[a, b],
            &options(MergeKind::Burst),
            &AtomicBool::new(true),
            |_| {}
        )
        .unwrap_err()
        .contains("cancelled"));
    }
}
