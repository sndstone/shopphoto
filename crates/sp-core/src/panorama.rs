//! Bounded photographic panorama stitching with explicit projection and
//! overlap validation. No extrapolated/content-aware pixels are generated.
use crate::source::RasterImage;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Projection {
    Planar,
    Cylindrical,
    Spherical,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Boundary {
    Crop,
    BlackCanvas,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Options {
    pub projection: Projection,
    pub horizontal_fov_degrees: f32,
    pub boundary: Boundary,
    /// Explicit left-to-right overlap when automatic feature matching is off.
    pub manual_overlap: Option<f32>,
    pub match_exposure: bool,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            projection: Projection::Cylindrical,
            horizontal_fov_degrees: 60.0,
            boundary: Boundary::Crop,
            manual_overlap: None,
            match_exposure: true,
        }
    }
}
#[derive(Debug)]
pub struct ResultImage {
    pub image: RasterImage,
    pub placements: Vec<[f32; 2]>,
    pub inliers: Vec<usize>,
    pub gains: Vec<f32>,
    pub crop: [u32; 4],
}

fn luminance(p: [f32; 4]) -> f32 {
    (p[0] * 0.2627 + p[1] * 0.6780 + p[2] * 0.0593).max(0.0)
}
fn sample(image: &RasterImage, x: f32, y: f32) -> [f32; 4] {
    if x < 0.0 || y < 0.0 || x > (image.width - 1) as f32 || y > (image.height - 1) as f32 {
        return [0.0; 4];
    }
    let (xx, yy) = (x as u32, y as u32);
    let (fx, fy) = (x - xx as f32, y - yy as f32);
    [0, 1, 2, 3].map(|c| {
        let a = image.linear_rec2020[(yy * image.width + xx) as usize][c] * (1.0 - fx)
            + image.linear_rec2020[(yy * image.width + (xx + 1).min(image.width - 1)) as usize][c]
                * fx;
        let b = image.linear_rec2020[((yy + 1).min(image.height - 1) * image.width + xx) as usize]
            [c]
            * (1.0 - fx)
            + image.linear_rec2020[((yy + 1).min(image.height - 1) * image.width
                + (xx + 1).min(image.width - 1)) as usize][c]
                * fx;
        a * (1.0 - fy) + b * fy
    })
}

pub fn project(
    image: &RasterImage,
    options: &Options,
    cancel: &AtomicBool,
) -> Result<RasterImage, String> {
    if !image.has_finite_opaque_pixels()
        || u64::from(image.width) * u64::from(image.height) > 150_000_000
    {
        return Err(
            "Panorama sources require complete finite opaque RGB pixels at most 150 MP".into(),
        );
    }
    if cancel.load(Ordering::Relaxed) {
        return Err("Panorama cancelled".into());
    }
    if !options.horizontal_fov_degrees.is_finite()
        || !(15.0..=150.0).contains(&options.horizontal_fov_degrees)
    {
        return Err("Panorama horizontal field of view must be 15–150 degrees".into());
    }
    if options.projection == Projection::Planar {
        return Ok(image.clone());
    }
    let f = image.width as f32 / (2.0 * (options.horizontal_fov_degrees.to_radians() * 0.5).tan());
    let width = (2.0 * f * ((image.width - 1) as f32 / (2.0 * f)).atan())
        .floor()
        .max(1.0) as u32;
    let height = image.height;
    let (cx, cy) = (
        (image.width - 1) as f32 * 0.5,
        (image.height - 1) as f32 * 0.5,
    );
    let mut pixels = Vec::with_capacity(width as usize * height as usize);
    for y in 0..height {
        if cancel.load(Ordering::Relaxed) {
            return Err("Panorama cancelled".into());
        }
        for x in 0..width {
            let theta = (x as f32 - (width - 1) as f32 * 0.5) / f;
            let sx = f * theta.tan();
            let vertical = (y as f32 - cy) / f;
            let sy = if options.projection == Projection::Spherical {
                vertical.tan() * (sx * sx + f * f).sqrt()
            } else {
                vertical * (sx * sx + f * f).sqrt()
            };
            pixels.push(sample(image, cx + sx, cy + sy));
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

struct Feature {
    x: f32,
    y: f32,
    patch: [f32; 49],
}
struct Proxy {
    width: usize,
    height: usize,
    scale: f32,
    gray: Vec<f32>,
    valid: Vec<bool>,
}
impl Proxy {
    fn new(image: &RasterImage) -> Self {
        let factor = (image.width.max(image.height) as usize)
            .div_ceil(1024)
            .max(1);
        let (width, height) = (
            image.width as usize / factor,
            image.height as usize / factor,
        );
        let mut gray = Vec::with_capacity(width * height);
        let mut valid = Vec::with_capacity(width * height);
        for y in 0..height {
            for x in 0..width {
                let p = sample(image, (x * factor) as f32, (y * factor) as f32);
                gray.push((luminance(p) + 0.001).ln());
                valid.push(p[3] > 0.999);
            }
        }
        Self {
            width,
            height,
            scale: factor as f32,
            gray,
            valid,
        }
    }
    fn features(&self) -> Vec<Feature> {
        if self.width < 24 || self.height < 24 {
            return Vec::new();
        }
        let mut corners = Vec::new();
        for y in 8..self.height - 8 {
            for x in 8..self.width - 8 {
                if ![
                    (x - 7, y - 7),
                    (x + 7, y - 7),
                    (x - 7, y + 7),
                    (x + 7, y + 7),
                ]
                .iter()
                .all(|(x, y)| self.valid[y * self.width + x])
                {
                    continue;
                }
                let (mut xx, mut yy, mut xy) = (0.0, 0.0, 0.0);
                for oy in y - 1..=y + 1 {
                    for ox in x - 1..=x + 1 {
                        let dx = self.gray[oy * self.width + ox + 1]
                            - self.gray[oy * self.width + ox - 1];
                        let dy = self.gray[(oy + 1) * self.width + ox]
                            - self.gray[(oy - 1) * self.width + ox];
                        xx += dx * dx;
                        yy += dy * dy;
                        xy += dx * dy;
                    }
                }
                let score = (xx * yy - xy * xy) / (xx + yy + 1e-6);
                if score > 0.001 {
                    corners.push((score, x, y));
                }
            }
        }
        corners.sort_by(|a, b| b.0.total_cmp(&a.0));
        let mut features: Vec<Feature> = Vec::new();
        for (_, x, y) in corners {
            if features
                .iter()
                .any(|p| (p.x - x as f32).powi(2) + (p.y - y as f32).powi(2) < 64.0)
            {
                continue;
            }
            let mut patch = [0.0; 49];
            for py in 0..7 {
                for px in 0..7 {
                    patch[py * 7 + px] = self.gray[(y + py * 2 - 6) * self.width + x + px * 2 - 6];
                }
            }
            let mean = patch.iter().sum::<f32>() / 49.0;
            let norm = patch.iter().map(|v| (v - mean).powi(2)).sum::<f32>().sqrt();
            if norm < 0.01 {
                continue;
            }
            for value in &mut patch {
                *value = (*value - mean) / norm;
            }
            features.push(Feature {
                x: x as f32,
                y: y as f32,
                patch,
            });
            if features.len() == 800 {
                break;
            }
        }
        features
    }
}

/// Displacement to place source over reference, with independent matched
/// features required across the overlap. Rejects textureless/unrelated views.
pub fn match_overlap(
    reference: &RasterImage,
    source: &RasterImage,
    cancel: &AtomicBool,
) -> Result<([f32; 2], usize), String> {
    let a = Proxy::new(reference);
    let b = Proxy::new(source);
    if (a.scale - b.scale).abs() > 0.01 {
        return Err("Panorama images need equal dimensions and field of view".into());
    }
    let fa = a.features();
    let fb = b.features();
    if fa.len() < 8 || fb.len() < 8 {
        return Err(
            "Too few panorama features; use textured overlapping frames or explicit manual overlap"
                .into(),
        );
    }
    let mut matches = Vec::new();
    for left in &fa {
        if cancel.load(Ordering::Relaxed) {
            return Err("Panorama cancelled".into());
        }
        let (mut best, mut second, mut index) = (f32::INFINITY, f32::INFINITY, 0usize);
        for (i, right) in fb.iter().enumerate() {
            let distance = left
                .patch
                .iter()
                .zip(right.patch)
                .map(|(a, b)| (a - b).powi(2))
                .sum::<f32>();
            if distance < best {
                second = best;
                best = distance;
                index = i;
            } else if distance < second {
                second = distance;
            }
        }
        if best < 0.4 && best < second * 0.6 {
            matches.push([left.x - fb[index].x, left.y - fb[index].y]);
        }
    }
    let mut consensus = Vec::new();
    for candidate in &matches {
        let inliers: Vec<_> = matches
            .iter()
            .copied()
            .filter(|m| (m[0] - candidate[0]).powi(2) + (m[1] - candidate[1]).powi(2) < 9.0)
            .collect();
        if inliers.len() > consensus.len() {
            consensus = inliers;
        }
    }
    if consensus.len() < 8 || consensus.len() * 4 < matches.len() {
        return Err("Panorama overlap could not be matched reliably; check frame order, field of view and parallax, or enter manual overlap".into());
    }
    let median = |axis| {
        let mut values: Vec<f32> = consensus.iter().map(|m| m[axis]).collect();
        values.sort_by(f32::total_cmp);
        values[values.len() / 2] * a.scale
    };
    let shift = refine_shift(reference, source, [median(0), median(1)], a.scale, cancel)?;
    if shift[0].abs() > reference.width as f32 * 0.9
        || shift[1].abs() > reference.height as f32 * 0.6
    {
        return Err("Panorama overlap is too small".into());
    }
    Ok((shift, consensus.len()))
}

fn refine_shift(
    reference: &RasterImage,
    source: &RasterImage,
    mut shift: [f32; 2],
    initial_step: f32,
    cancel: &AtomicBool,
) -> Result<[f32; 2], String> {
    let score = |shift: [f32; 2]| {
        let step = (reference.width.max(reference.height) / 256).max(1) as usize;
        let (mut n, mut sa, mut sb, mut aa, mut bb, mut ab) =
            (0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64);
        for y in (2..reference.height.saturating_sub(2)).step_by(step) {
            for x in (2..reference.width.saturating_sub(2)).step_by(step) {
                let a = reference.linear_rec2020[(y * reference.width + x) as usize];
                let b = sample(source, x as f32 - shift[0], y as f32 - shift[1]);
                if a[3] < 0.999 || b[3] < 0.999 {
                    continue;
                }
                let la = (luminance(a) + 0.001).ln() as f64;
                let lb = (luminance(b) + 0.001).ln() as f64;
                n += 1.0;
                sa += la;
                sb += lb;
                aa += la * la;
                bb += lb * lb;
                ab += la * lb;
            }
        }
        let variance = (aa - sa * sa / n.max(1.0)) * (bb - sb * sb / n.max(1.0));
        if n < 64.0 || variance < 1e-10 {
            -1.0
        } else {
            ((ab - sa * sb / n) / variance.sqrt()) as f32
        }
    };
    for refinement in 0..8 {
        if cancel.load(Ordering::Relaxed) {
            return Err("Panorama cancelled".into());
        }
        let step = (initial_step * 2.0f32.powi(-refinement)).max(0.0625);
        for _ in 0..8 {
            let before = score(shift);
            let mut best = before;
            let mut next = shift;
            for axis in 0..2 {
                for direction in [-1.0, 1.0] {
                    let mut candidate = shift;
                    candidate[axis] += direction * step;
                    let value = score(candidate);
                    if value > best {
                        best = value;
                        next = candidate;
                    }
                }
            }
            shift = next;
            if best - before < 1e-6 {
                break;
            }
        }
    }
    if score(shift) < 0.7 {
        return Err("Panorama overlap fails full-resolution verification; check moving objects, parallax and projection".into());
    }
    Ok(shift)
}

fn exposure_gain(reference: &RasterImage, source: &RasterImage, shift: [f32; 2]) -> f32 {
    let mut ratios = Vec::new();
    let step = (reference.width.max(reference.height) / 128).max(1);
    for y in (0..reference.height).step_by(step as usize) {
        for x in (0..reference.width).step_by(step as usize) {
            let a = reference.linear_rec2020[(y * reference.width + x) as usize];
            let b = sample(source, x as f32 - shift[0], y as f32 - shift[1]);
            let (ya, yb) = (luminance(a), luminance(b));
            if a[3] > 0.999 && b[3] > 0.999 && ya > 0.01 && yb > 0.01 && ya < 0.9 && yb < 0.9 {
                ratios.push((ya / yb).ln());
            }
        }
    }
    if ratios.len() < 16 {
        return 1.0;
    }
    ratios.sort_by(f32::total_cmp);
    ratios[ratios.len() / 2].exp().clamp(0.25, 4.0)
}

fn largest_rectangle(pixels: &[[f32; 4]], width: usize, height: usize) -> [u32; 4] {
    let mut heights = vec![0usize; width];
    let mut best = [0u32; 4];
    for y in 0..height {
        for x in 0..width {
            heights[x] = if pixels[y * width + x][3] > 1e-8 {
                heights[x] + 1
            } else {
                0
            };
        }
        let mut stack: Vec<(usize, usize)> = Vec::new();
        for (x, h) in heights
            .iter()
            .copied()
            .chain(std::iter::once(0))
            .enumerate()
        {
            let mut start = x;
            while stack.last().is_some_and(|(_, previous)| *previous > h) {
                let (left, previous) = stack.pop().unwrap();
                if (x - left) * previous > (best[2] * best[3]) as usize {
                    best = [
                        left as u32,
                        (y + 1 - previous) as u32,
                        (x - left) as u32,
                        previous as u32,
                    ];
                }
                start = left;
            }
            if h > 0 && stack.last().is_none_or(|(_, previous)| *previous < h) {
                stack.push((start, h));
            }
        }
    }
    best
}

pub fn stitch(
    images: Vec<RasterImage>,
    options: &Options,
    cancel: &AtomicBool,
    mut progress: impl FnMut(f32),
) -> Result<ResultImage, String> {
    if !(2..=16).contains(&images.len()) {
        return Err("Select 2–16 ordered panorama photographs".into());
    }
    let dimensions = (images[0].width, images[0].height);
    if images.iter().any(|image| {
        (image.width, image.height) != dimensions
            || image.linear_rec2020.len() != image.width as usize * image.height as usize
    }) {
        return Err("Panorama sources must have equal upright dimensions".into());
    }
    if let Some(overlap) = options.manual_overlap {
        if !overlap.is_finite() || !(0.1..=0.9).contains(&overlap) {
            return Err("Manual panorama overlap must be 10–90%".into());
        }
    }
    let count = images.len();
    let mut projected = Vec::with_capacity(count);
    for (index, image) in images.into_iter().enumerate() {
        projected.push(project(&image, options, cancel)?);
        progress(0.2 * (index + 1) as f32 / count as f32);
    }
    let mut placements = vec![[0.0f32; 2]; count];
    let mut inliers = vec![0; count];
    let mut gains = vec![1.0f32; count];
    for i in 1..count {
        let (shift, matches) = if let Some(overlap) = options.manual_overlap {
            ([projected[i].width as f32 * (1.0 - overlap), 0.0], 0)
        } else {
            match_overlap(&projected[i - 1], &projected[i], cancel)?
        };
        placements[i] = [
            placements[i - 1][0] + shift[0],
            placements[i - 1][1] + shift[1],
        ];
        inliers[i] = matches;
        if options.match_exposure {
            gains[i] = (gains[i - 1] * exposure_gain(&projected[i - 1], &projected[i], shift))
                .clamp(0.125, 8.0);
        }
        progress(0.2 + 0.3 * i as f32 / (count - 1) as f32);
    }
    let left = placements
        .iter()
        .map(|p| p[0])
        .fold(f32::INFINITY, f32::min)
        .floor();
    let top = placements
        .iter()
        .map(|p| p[1])
        .fold(f32::INFINITY, f32::min)
        .floor();
    let right = placements
        .iter()
        .zip(&projected)
        .map(|(p, image)| p[0] + image.width as f32)
        .fold(f32::NEG_INFINITY, f32::max)
        .ceil();
    let bottom = placements
        .iter()
        .zip(&projected)
        .map(|(p, image)| p[1] + image.height as f32)
        .fold(f32::NEG_INFINITY, f32::max)
        .ceil();
    let (width, height) = ((right - left) as u32, (bottom - top) as u32);
    if u64::from(width) * u64::from(height) > 150_000_000 {
        return Err("Panorama canvas exceeds 150 MP; use fewer or smaller sources".into());
    }
    let mut pixels = vec![[0.0f32; 4]; width as usize * height as usize];
    for (index, (image, position)) in projected.iter().zip(&placements).enumerate() {
        let (ox, oy) = (position[0] - left, position[1] - top);
        let feather = (image.width.min(image.height) as f32 * 0.12).max(1.0);
        for y in oy.floor().max(0.0) as u32..((oy + image.height as f32).ceil() as u32).min(height)
        {
            if cancel.load(Ordering::Relaxed) {
                return Err("Panorama cancelled".into());
            }
            for x in
                ox.floor().max(0.0) as u32..((ox + image.width as f32).ceil() as u32).min(width)
            {
                let (sx, sy) = (x as f32 - ox, y as f32 - oy);
                let sample = sample(image, sx, sy);
                if sample[3] < 0.999 {
                    continue;
                }
                let weight = ((sx + 1.0)
                    .min(image.width as f32 - sx)
                    .min(sy + 1.0)
                    .min(image.height as f32 - sy)
                    / feather)
                    .clamp(0.001, 1.0);
                let dst = &mut pixels[(y * width + x) as usize];
                for c in 0..3 {
                    dst[c] += sample[c] * gains[index] * weight;
                }
                dst[3] += weight;
            }
        }
        progress(0.5 + 0.4 * (index + 1) as f32 / count as f32);
    }
    let crop = if options.boundary == Boundary::Crop {
        largest_rectangle(&pixels, width as usize, height as usize)
    } else {
        [0, 0, width, height]
    };
    if crop[2] < 8 || crop[3] < 8 {
        return Err("Panorama has no usable common opaque rectangle".into());
    }
    let mut output = Vec::with_capacity(crop[2] as usize * crop[3] as usize);
    for y in crop[1]..crop[1] + crop[3] {
        if cancel.load(Ordering::Relaxed) {
            return Err("Panorama cancelled".into());
        }
        for x in crop[0]..crop[0] + crop[2] {
            let p = pixels[(y * width + x) as usize];
            output.push(if p[3] > 1e-8 {
                [p[0] / p[3], p[1] / p[3], p[2] / p[3], 1.0]
            } else {
                [0.0, 0.0, 0.0, 1.0]
            });
        }
    }
    progress(1.0);
    Ok(ResultImage {
        image: RasterImage {
            width: crop[2],
            height: crop[3],
            linear_rec2020: output,
            bits_per_channel: 32,
            embedded_icc: None,
        },
        placements,
        inliers,
        gains,
        crop,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projection_rejects_malformed_and_transparent_sources_for_every_projection() {
        let cancel = AtomicBool::new(false);
        for projection in [
            Projection::Planar,
            Projection::Cylindrical,
            Projection::Spherical,
        ] {
            let options = Options {
                projection,
                ..Default::default()
            };
            let mut image = frame(0, 1.0);
            image.linear_rec2020.pop();
            assert!(project(&image, &options, &cancel).is_err());
            image = frame(0, 1.0);
            image.width = 0;
            assert!(project(&image, &options, &cancel).is_err());
            image = frame(0, 1.0);
            image.linear_rec2020[0][1] = f32::INFINITY;
            assert!(project(&image, &options, &cancel).is_err());
            image = frame(0, 1.0);
            image.linear_rec2020[0][3] = 0.0;
            assert!(project(&image, &options, &cancel).is_err());
        }
    }

    fn scene(x: u32, y: u32) -> f32 {
        let cell = (x / 5).wrapping_mul(73856093) ^ (y / 5).wrapping_mul(19349663);
        0.1 + ((cell.wrapping_mul(83492791) >> 17) % 700) as f32 / 1000.0
    }
    fn frame(offset: u32, gain: f32) -> RasterImage {
        RasterImage {
            width: 128,
            height: 96,
            linear_rec2020: (0..96)
                .flat_map(|y| {
                    (0..128).map(move |x| {
                        let p = scene(x + offset, y) * gain;
                        [p, p, p, 1.0]
                    })
                })
                .collect(),
            bits_per_channel: 32,
            embedded_icc: None,
        }
    }
    #[test]
    fn matches_large_overlap_displacement_and_normalizes_exposure() {
        let a = frame(0, 1.0);
        let b = frame(64, 0.65);
        let cancel = AtomicBool::new(false);
        let (shift, inliers) = match_overlap(&a, &b, &cancel).unwrap();
        assert!(
            (shift[0] - 64.0).abs() < 1.0 && shift[1].abs() < 1.0,
            "{shift:?}"
        );
        assert!(inliers >= 8);
        let result = stitch(
            vec![a, b],
            &Options {
                projection: Projection::Planar,
                ..Default::default()
            },
            &cancel,
            |_| {},
        )
        .unwrap();
        assert_eq!((result.image.width, result.image.height), (192, 96));
        assert!((result.gains[1] - 1.0 / 0.65).abs() < 0.01);
        for x in [10, 80, 170] {
            assert!(
                (result.image.linear_rec2020[40 * 192 + x][0] - scene(x as u32, 40)).abs() < 0.003
            );
        }
    }
    #[test]
    fn projections_curve_boundaries_and_crop_to_opaque_rectangle() {
        let source = frame(0, 1.0);
        let cancel = AtomicBool::new(false);
        for projection in [Projection::Cylindrical, Projection::Spherical] {
            let options = Options {
                projection,
                manual_overlap: Some(0.5),
                ..Default::default()
            };
            let projected = project(&source, &options, &cancel).unwrap();
            assert!(projected.width < source.width);
            assert_eq!(projected.linear_rec2020[0][3], 0.0);
            let result = stitch(
                vec![source.clone(), source.clone()],
                &options,
                &cancel,
                |_| {},
            )
            .unwrap();
            assert!(result.image.height < source.height);
            assert!(result
                .image
                .linear_rec2020
                .iter()
                .all(|p| p[3] == 1.0 && p[0] > 0.0));
        }
    }
    #[test]
    fn explicit_canvas_keeps_empty_edges_and_cancel_stops_work() {
        let options = Options {
            manual_overlap: Some(0.5),
            boundary: Boundary::BlackCanvas,
            ..Default::default()
        };
        let result = stitch(
            vec![frame(0, 1.0), frame(64, 1.0)],
            &options,
            &AtomicBool::new(false),
            |_| {},
        )
        .unwrap();
        assert_eq!(result.image.linear_rec2020[0], [0.0, 0.0, 0.0, 1.0]);
        assert!(stitch(
            vec![frame(0, 1.0), frame(64, 1.0)],
            &options,
            &AtomicBool::new(true),
            |_| {}
        )
        .is_err());
    }
}
