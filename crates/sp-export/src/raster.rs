//! Export-side geometry and linear-light resampling.

use crate::{ExportColorSpace, ExportSettings, OutputSharpening};
use sp_core::color::{mat3_apply, mat3_inverse, srgb_eotf, srgb_oetf, Mat3};
use sp_core::edit::GeometryParams;

/// Interleaved RGBA8 export handoff. Pixels are sRGB-encoded because the
/// display renderer produces the same bytes; conversion to P3/Rec.2020 is
/// performed here before container encoding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportImage {
    pub width: u32,
    pub height: u32,
    pub rgba8: Vec<u8>,
    pub rgba16: Option<Vec<u16>>,
}

impl ExportImage {
    /// Construct after validating the exact buffer length.
    pub fn new(width: u32, height: u32, rgba8: Vec<u8>) -> Option<Self> {
        let expected = usize::try_from(
            u64::from(width)
                .checked_mul(u64::from(height))?
                .checked_mul(4)?,
        )
        .ok()?;
        (width > 0 && height > 0 && rgba8.len() == expected).then_some(Self {
            width,
            height,
            rgba8,
            rgba16: None,
        })
    }

    /// Construct from a true high-precision GPU handoff.
    pub fn new_16(width: u32, height: u32, rgba16: Vec<u16>) -> Option<Self> {
        let expected = usize::try_from(
            u64::from(width)
                .checked_mul(u64::from(height))?
                .checked_mul(4)?,
        )
        .ok()?;
        (width > 0 && height > 0 && rgba16.len() == expected).then(|| Self {
            width,
            height,
            rgba8: rgba16.iter().map(|value| (value / 257) as u8).collect(),
            rgba16: Some(rgba16),
        })
    }

    /// Accept both GPU buffers without regenerating or copying either one.
    pub fn from_buffers(
        width: u32,
        height: u32,
        rgba8: Vec<u8>,
        rgba16: Option<Vec<u16>>,
    ) -> Option<Self> {
        let expected = usize::try_from(
            u64::from(width)
                .checked_mul(u64::from(height))?
                .checked_mul(4)?,
        )
        .ok()?;
        (width > 0
            && height > 0
            && rgba8.len() == expected
            && rgba16
                .as_ref()
                .is_none_or(|pixels| pixels.len() == expected))
        .then_some(Self {
            width,
            height,
            rgba8,
            rgba16,
        })
    }

    /// Opaque RGB8 bytes for JPEG/AVIF.
    #[must_use]
    pub fn rgb8(&self) -> Vec<u8> {
        self.rgba8
            .chunks_exact(4)
            .flat_map(|px| [px[0], px[1], px[2]])
            .collect()
    }

    /// Promote encoded channel values exactly to 16-bit (`v * 257`).
    #[must_use]
    pub fn rgb16_bytes(&self) -> Vec<u8> {
        self.rgb16()
            .into_iter()
            .flat_map(u16::to_ne_bytes)
            .collect()
    }

    #[must_use]
    pub fn rgb16(&self) -> Vec<u16> {
        self.rgba16.as_ref().map_or_else(
            || {
                self.rgba8
                    .chunks_exact(4)
                    .flat_map(|px| [px[0], px[1], px[2]].map(|value| u16::from(value) * 257))
                    .collect()
            },
            |rgba16| {
                rgba16
                    .chunks_exact(4)
                    .flat_map(|px| [px[0], px[1], px[2]])
                    .collect()
            },
        )
    }
}

const REC709_TO_XYZ: Mat3 = [
    [0.412_391, 0.357_584, 0.180_481],
    [0.212_639, 0.715_169, 0.072_192],
    [0.019_331, 0.119_195, 0.950_532],
];
const P3_TO_XYZ: Mat3 = [
    [0.486_571, 0.265_668, 0.198_217],
    [0.228_975, 0.691_739, 0.079_287],
    [0.0, 0.045_113, 1.043_944],
];

fn convert_space(image: &mut ExportImage, space: ExportColorSpace) {
    let target_to_xyz = match space {
        ExportColorSpace::Srgb => return,
        ExportColorSpace::DisplayP3 => P3_TO_XYZ,
        ExportColorSpace::Rec2020 => sp_core::color::REC2020_TO_XYZ,
        ExportColorSpace::AdobeRgb => sp_core::color::ADOBERGB_TO_XYZ,
    };
    let transfer = space.transfer();
    let Some(xyz_to_target) = mat3_inverse(&target_to_xyz) else {
        return;
    };
    for index in 0..(image.width * image.height) as usize {
        let off = index * 4;
        let load = |channel| {
            image.rgba16.as_ref().map_or_else(
                || f32::from(image.rgba8[off + channel]) / 255.0,
                |pixels| f32::from(pixels[off + channel]) / 65535.0,
            )
        };
        let linear709 = [srgb_eotf(load(0)), srgb_eotf(load(1)), srgb_eotf(load(2))];
        let target = mat3_apply(&xyz_to_target, mat3_apply(&REC709_TO_XYZ, linear709));
        for channel in 0..3 {
            let encoded = transfer.oetf(target[channel]).clamp(0.0, 1.0);
            image.rgba8[off + channel] = (encoded * 255.0).round() as u8;
            if let Some(pixels) = &mut image.rgba16 {
                pixels[off + channel] = (encoded * 65535.0).round() as u16;
            }
        }
    }
}

#[derive(Clone, Copy)]
enum Transfer {
    Srgb,
    Pq,
    AdobeRgb,
}

impl Transfer {
    /// The SDR transfer the encoded bytes of `space` use.
    fn for_space(space: ExportColorSpace) -> Self {
        match space.transfer() {
            sp_core::color::Transfer::Srgb => Self::Srgb,
            sp_core::color::Transfer::AdobeRgb => Self::AdobeRgb,
        }
    }
}

fn decode_channel(value: f32, transfer: Transfer) -> f32 {
    match transfer {
        Transfer::Srgb => srgb_eotf(value),
        Transfer::Pq => sp_core::color::pq_eotf(value),
        Transfer::AdobeRgb => sp_core::color::adobe_rgb_eotf(value),
    }
}

fn encode_channel(value: f32, transfer: Transfer) -> f32 {
    match transfer {
        Transfer::Srgb => srgb_oetf(value),
        Transfer::Pq => sp_core::color::pq_oetf(value),
        Transfer::AdobeRgb => sp_core::color::adobe_rgb_oetf(value),
    }
}

fn bilinear(image: &ExportImage, x: f32, y: f32, transfer: Transfer) -> [f32; 4] {
    let x = x.clamp(0.0, image.width.saturating_sub(1) as f32);
    let y = y.clamp(0.0, image.height.saturating_sub(1) as f32);
    let x0 = x.floor() as u32;
    let y0 = y.floor() as u32;
    let x1 = (x0 + 1).min(image.width - 1);
    let y1 = (y0 + 1).min(image.height - 1);
    let fx = x - x0 as f32;
    let fy = y - y0 as f32;
    let load = |px: u32, py: u32| {
        let off = ((py * image.width + px) * 4) as usize;
        let scale = if image.rgba16.is_some() {
            65535.0
        } else {
            255.0
        };
        let value = |channel| {
            image.rgba16.as_ref().map_or_else(
                || f32::from(image.rgba8[off + channel]),
                |pixels| f32::from(pixels[off + channel]),
            ) / scale
        };
        [
            decode_channel(value(0), transfer),
            decode_channel(value(1), transfer),
            decode_channel(value(2), transfer),
            value(3),
        ]
    };
    let (a, b, c, d) = (load(x0, y0), load(x1, y0), load(x0, y1), load(x1, y1));
    let mut out = [0.0; 4];
    for channel in 0..4 {
        let top = a[channel] + (b[channel] - a[channel]) * fx;
        let bottom = c[channel] + (d[channel] - c[channel]) * fx;
        out[channel] = top + (bottom - top) * fy;
    }
    out
}

fn geometry(image: &ExportImage, params: GeometryParams, transfer: Transfer) -> ExportImage {
    let map = sp_core::geometry::GeometryMap::new([image.width, image.height], params);
    let [out_w, out_h] = map.output_size;
    let mut rgba8 = vec![0; (out_w * out_h * 4) as usize];
    let mut rgba16 = image
        .rgba16
        .as_ref()
        .map(|_| vec![0; (out_w * out_h * 4) as usize]);
    for oy in 0..out_h {
        for ox in 0..out_w {
            let uv = map.source_uv([
                (ox as f32 + 0.5) / out_w as f32,
                (oy as f32 + 0.5) / out_h as f32,
            ]);
            let sx = uv[0] * image.width as f32 - 0.5;
            let sy = uv[1] * image.height as f32 - 0.5;
            let sample = bilinear(image, sx, sy, transfer);
            let off = ((oy * out_w + ox) * 4) as usize;
            for channel in 0..3 {
                let encoded = encode_channel(sample[channel], transfer).clamp(0.0, 1.0);
                rgba8[off + channel] = (encoded * 255.0).round() as u8;
                if let Some(pixels) = &mut rgba16 {
                    pixels[off + channel] = (encoded * 65535.0).round() as u16;
                }
            }
            rgba8[off + 3] = (sample[3] * 255.0).round() as u8;
            if let Some(pixels) = &mut rgba16 {
                pixels[off + 3] = (sample[3] * 65535.0).round() as u16;
            }
        }
    }
    ExportImage {
        width: out_w,
        height: out_h,
        rgba8,
        rgba16,
    }
}

fn lanczos(x: f32) -> f32 {
    let x = x.abs();
    if x < 1e-6 {
        1.0
    } else if x >= 3.0 {
        0.0
    } else {
        let pix = std::f32::consts::PI * x;
        (pix.sin() / pix) * ((pix / 3.0).sin() / (pix / 3.0))
    }
}

// Bound both axis caches together to 16 MiB. Extreme aspect ratios use
// computed taps instead; decoded transfer tables never exceed 256 KiB.
const AXIS_CACHE_BYTES: usize = 8 * 1024 * 1024;
#[derive(Clone, Copy)]
struct Tap {
    index: usize,
    weight: f32,
}
struct Axis {
    source: u32,
    scale: f32,
    radius: f32,
    cached: Option<Vec<Vec<Tap>>>,
}
enum Taps<'a> {
    Cached(std::slice::Iter<'a, Tap>),
    Computed {
        range: std::ops::RangeInclusive<i32>,
        center: f32,
        radius: f32,
        source: u32,
    },
}
impl Iterator for Taps<'_> {
    type Item = Tap;
    fn next(&mut self) -> Option<Tap> {
        match self {
            Self::Cached(taps) => taps.next().copied(),
            Self::Computed {
                range,
                center,
                radius,
                source,
            } => range.next().map(|i| Tap {
                index: i.clamp(0, source.saturating_sub(1) as i32) as usize,
                weight: lanczos((i as f32 - *center) / *radius),
            }),
        }
    }
}
impl Axis {
    fn new(source: u32, output: u32) -> Self {
        let scale = source as f32 / output as f32;
        let radius = scale.max(1.0);
        let mut axis = Self {
            source,
            scale,
            radius,
            cached: None,
        };
        let max_taps = (6.0 * radius).ceil() as usize + 3;
        let bytes = max_taps
            .saturating_mul(std::mem::size_of::<Tap>())
            .saturating_add(std::mem::size_of::<Vec<Tap>>())
            .saturating_mul(output as usize);
        if bytes <= AXIS_CACHE_BYTES {
            axis.cached = Some((0..output).map(|o| axis.taps(o).collect()).collect());
        }
        axis
    }
    fn taps(&self, output: u32) -> Taps<'_> {
        if let Some(cached) = &self.cached {
            return Taps::Cached(cached[output as usize].iter());
        }
        let center = (output as f32 + 0.5) * self.scale - 0.5;
        Taps::Computed {
            range: (center - 3.0 * self.radius).floor() as i32
                ..=(center + 3.0 * self.radius).ceil() as i32,
            center,
            radius: self.radius,
            source: self.source,
        }
    }
}

fn resize_linear(image: &ExportImage, out_w: u32, out_h: u32, transfer: Transfer) -> ExportImage {
    if (out_w, out_h) == (image.width, image.height) {
        return image.clone();
    }
    let mut rgba8 = vec![0; (out_w * out_h * 4) as usize];
    let mut rgba16 = image
        .rgba16
        .as_ref()
        .map(|_| vec![0; (out_w * out_h * 4) as usize]);
    let scale = if image.rgba16.is_some() {
        65535.0
    } else {
        255.0
    };
    let decoded: Vec<f32> = (0..=scale as u32)
        .map(|value| decode_channel(value as f32 / scale, transfer))
        .collect();
    let xs = Axis::new(image.width, out_w);
    let ys = Axis::new(image.height, out_h);
    for oy in 0..out_h {
        for ox in 0..out_w {
            let mut sum = [0.0f32; 4];
            let mut weights = 0.0;
            for y in ys.taps(oy) {
                for x in xs.taps(ox) {
                    let weight = y.weight * x.weight;
                    if weight == 0.0 {
                        continue;
                    }
                    let offset = (y.index * image.width as usize + x.index) * 4;
                    let code = |channel| {
                        image.rgba16.as_ref().map_or_else(
                            || usize::from(image.rgba8[offset + channel]),
                            |pixels| usize::from(pixels[offset + channel]),
                        )
                    };
                    // Lanczos taps lie at integer pixel positions: bilinear
                    // interpolation at these positions is exactly a direct load.
                    let sample = [
                        decoded[code(0)],
                        decoded[code(1)],
                        decoded[code(2)],
                        code(3) as f32 / scale,
                    ];
                    for channel in 0..4 {
                        sum[channel] += sample[channel] * weight;
                    }
                    weights += weight;
                }
            }
            let off = ((oy * out_w + ox) * 4) as usize;
            let denom = weights.max(1e-6);
            for channel in 0..3 {
                let encoded = encode_channel(sum[channel] / denom, transfer).clamp(0.0, 1.0);
                rgba8[off + channel] = (encoded * 255.0).round() as u8;
                if let Some(pixels) = &mut rgba16 {
                    pixels[off + channel] = (encoded * 65535.0).round() as u16;
                }
            }
            rgba8[off + 3] = ((sum[3] / denom).clamp(0.0, 1.0) * 255.0).round() as u8;
            if let Some(pixels) = &mut rgba16 {
                pixels[off + 3] = ((sum[3] / denom).clamp(0.0, 1.0) * 65535.0).round() as u16;
            }
        }
    }
    ExportImage {
        width: out_w,
        height: out_h,
        rgba8,
        rgba16,
    }
}

fn output_sharpen(
    image: &ExportImage,
    amount: OutputSharpening,
    transfer: Transfer,
) -> ExportImage {
    let gain = match amount {
        OutputSharpening::None => return image.clone(),
        OutputSharpening::Low => 0.35,
        OutputSharpening::Standard => 0.7,
        OutputSharpening::High => 1.1,
    };
    if image.width < 3 || image.height < 3 {
        return image.clone();
    }

    let mut output = image.clone();
    let scale = if image.rgba16.is_some() {
        65_535.0
    } else {
        255.0
    };
    let load = |x: u32, y: u32, channel: usize| {
        let offset = ((y * image.width + x) * 4) as usize + channel;
        let encoded = image.rgba16.as_ref().map_or_else(
            || f32::from(image.rgba8[offset]),
            |pixels| f32::from(pixels[offset]),
        ) / scale;
        decode_channel(encoded, transfer)
    };
    const KERNEL: [[f32; 3]; 3] = [[1.0, 2.0, 1.0], [2.0, 4.0, 2.0], [1.0, 2.0, 1.0]];
    const LUMA: [f32; 3] = [0.2126, 0.7152, 0.0722];

    for y in 1..image.height - 1 {
        for x in 1..image.width - 1 {
            let mut center = [0.0; 3];
            let mut blurred = [0.0; 3];
            for channel in 0..3 {
                center[channel] = load(x, y, channel);
                for (ky, row) in KERNEL.iter().enumerate() {
                    for (kx, weight) in row.iter().enumerate() {
                        blurred[channel] +=
                            load(x + kx as u32 - 1, y + ky as u32 - 1, channel) * weight;
                    }
                }
                blurred[channel] /= 16.0;
            }
            let detail = (0..3)
                .map(|channel| (center[channel] - blurred[channel]) * LUMA[channel])
                .sum::<f32>()
                * gain;
            let offset = ((y * image.width + x) * 4) as usize;
            for channel in 0..3 {
                let encoded =
                    encode_channel((center[channel] + detail).max(0.0), transfer).clamp(0.0, 1.0);
                output.rgba8[offset + channel] = (encoded * 255.0).round() as u8;
                if let Some(pixels) = &mut output.rgba16 {
                    pixels[offset + channel] = (encoded * 65_535.0).round() as u16;
                }
            }
        }
    }
    output
}

fn apply_border(mut image: ExportImage, settings: &ExportSettings, hdr: bool) -> ExportImage {
    let border = settings.border;
    let Some(border) = border else {
        return image;
    };
    let thickness = ((image.width.min(image.height) as f32 * border.width_percent.clamp(0.0, 25.0)
        / 100.0)
        .round() as u32)
        .min(image.width.min(image.height) / 2);
    if thickness == 0 {
        return image;
    }
    let color = crate::text_watermark::mark_color(border.rgb, settings, hdr);
    for y in 0..image.height {
        for x in 0..image.width {
            if x < thickness
                || y < thickness
                || x >= image.width - thickness
                || y >= image.height - thickness
            {
                let offset = ((y * image.width + x) * 4) as usize;
                crate::text_watermark::blend(&mut image, offset, color, 1.0, settings, hdr);
            }
        }
    }
    image
}

fn apply_watermark(mut output: ExportImage, settings: &ExportSettings, hdr: bool) -> ExportImage {
    let watermark = settings.watermark.as_ref();
    let Some(mark) = watermark else {
        return output;
    };
    let Ok(decoded) = image::open(&mark.png) else {
        return output;
    };
    let source = decoded.to_rgba8();
    let width = ((output.width as f32 * mark.scale_percent.clamp(1.0, 100.0) / 100.0).round()
        as u32)
        .max(1);
    let height = ((source.height() as f32 * width as f32 / source.width().max(1) as f32).round()
        as u32)
        .max(1);
    let resized = image::imageops::resize(
        &source,
        width,
        height,
        image::imageops::FilterType::Lanczos3,
    );
    let margin = (output.width.min(output.height) / 50).max(2);
    let (origin_x, origin_y) = match mark.position {
        crate::WatermarkPosition::TopLeft => (margin, margin),
        crate::WatermarkPosition::TopRight => (output.width.saturating_sub(width + margin), margin),
        crate::WatermarkPosition::BottomLeft => {
            (margin, output.height.saturating_sub(height + margin))
        }
        crate::WatermarkPosition::BottomRight => (
            output.width.saturating_sub(width + margin),
            output.height.saturating_sub(height + margin),
        ),
        crate::WatermarkPosition::Center => (
            output.width.saturating_sub(width) / 2,
            output.height.saturating_sub(height) / 2,
        ),
    };
    for (x, y, pixel) in resized.enumerate_pixels() {
        let dx = origin_x + x;
        let dy = origin_y + y;
        if dx >= output.width || dy >= output.height {
            continue;
        }
        let offset = ((dy * output.width + dx) * 4) as usize;
        let alpha = f32::from(pixel[3]) / 255.0 * mark.opacity.clamp(0.0, 1.0);
        let color =
            crate::text_watermark::mark_color([pixel[0], pixel[1], pixel[2]], settings, hdr);
        crate::text_watermark::blend(&mut output, offset, color, alpha, settings, hdr);
    }
    output
}

fn finish(image: ExportImage, settings: &ExportSettings, hdr: bool) -> ExportImage {
    crate::text_watermark::apply(
        apply_watermark(apply_border(image, settings, hdr), settings, hdr),
        settings,
        hdr,
    )
}

fn prepare(
    mut image: std::borrow::Cow<'_, ExportImage>,
    geometry_params: GeometryParams,
    settings: &ExportSettings,
    transfer: Transfer,
    hdr: bool,
    convert: bool,
) -> ExportImage {
    if geometry_params != GeometryParams::default() {
        image = std::borrow::Cow::Owned(geometry(&image, geometry_params, transfer));
    }
    let (width, height) = settings.size.output_dims(image.width, image.height);
    if (width, height) != (image.width, image.height) {
        image = std::borrow::Cow::Owned(resize_linear(&image, width, height, transfer));
    }
    if convert && settings.color_space != ExportColorSpace::Srgb {
        convert_space(image.to_mut(), settings.color_space);
    }
    let target = if hdr {
        Transfer::Pq
    } else {
        Transfer::for_space(settings.color_space)
    };
    if settings.output_sharpening != OutputSharpening::None && image.width >= 3 && image.height >= 3
    {
        image = std::borrow::Cow::Owned(output_sharpen(&image, settings.output_sharpening, target));
    }
    finish(image.into_owned(), settings, hdr)
}

/// Apply geometry, linear-light sizing and target-space conversion.
#[must_use]
pub fn prepare_image(
    source: &ExportImage,
    geometry: GeometryParams,
    settings: &ExportSettings,
) -> ExportImage {
    prepare(
        std::borrow::Cow::Borrowed(source),
        geometry,
        settings,
        Transfer::Srgb,
        false,
        true,
    )
}
/// Prepare GPU-encoded target-space pixels, preserving unchanged samples exactly.
#[must_use]
pub fn prepare_image_already_converted(
    source: &ExportImage,
    geometry: GeometryParams,
    settings: &ExportSettings,
) -> ExportImage {
    prepare(
        std::borrow::Cow::Borrowed(source),
        geometry,
        settings,
        Transfer::for_space(settings.color_space),
        false,
        false,
    )
}
/// Prepare Rec.2100 PQ in absolute linear light without changing primaries.
#[must_use]
pub fn prepare_pq_image(
    source: &ExportImage,
    geometry: GeometryParams,
    settings: &ExportSettings,
) -> ExportImage {
    prepare(
        std::borrow::Cow::Borrowed(source),
        geometry,
        settings,
        Transfer::Pq,
        true,
        false,
    )
}
/// Consume GPU-encoded pixels so unchanged stages do not copy the full image.
#[must_use]
pub fn prepare_image_already_converted_owned(
    source: ExportImage,
    geometry: GeometryParams,
    settings: &ExportSettings,
) -> ExportImage {
    prepare(
        std::borrow::Cow::Owned(source),
        geometry,
        settings,
        Transfer::for_space(settings.color_space),
        false,
        false,
    )
}
/// Consume Rec.2100 PQ pixels without copying unchanged stages.
#[must_use]
pub fn prepare_pq_image_owned(
    source: ExportImage,
    geometry: GeometryParams,
    settings: &ExportSettings,
) -> ExportImage {
    prepare(
        std::borrow::Cow::Owned(source),
        geometry,
        settings,
        Transfer::Pq,
        true,
        false,
    )
}

#[cfg(test)]
#[path = "raster_reference.rs"]
mod reference;

#[cfg(test)]
mod tests {
    #[test]
    fn cached_resizing_matches_original_across_depths_transfers_and_edges() {
        for high in [false, true] {
            for transfer in [Transfer::Srgb, Transfer::AdobeRgb, Transfer::Pq] {
                for (w, h, ow, oh) in [
                    (1, 19, 1, 6),
                    (23, 1, 7, 1),
                    (29, 17, 13, 9),
                    (41, 31, 1, 1),
                    (17, 13, 17, 7),
                    (13, 11, 21, 17),
                ] {
                    let values: Vec<u16> = (0..w * h * 4)
                        .map(|i| ((u64::from(i) * 17_123 + 97) % 65_536) as u16)
                        .collect();
                    let image = if high {
                        ExportImage::new_16(w, h, values).unwrap()
                    } else {
                        ExportImage::new(w, h, values.iter().map(|v| (v / 257) as u8).collect())
                            .unwrap()
                    };
                    assert_eq!(
                        resize_linear(&image, ow, oh, transfer),
                        reference::resize_linear(&image, ow, oh, transfer),
                        "{w}x{h} -> {ow}x{oh}, high={high}"
                    );
                }
            }
        }
    }

    #[test]
    fn extreme_axis_uses_bounded_uncached_taps() {
        let axis = Axis::new(100_000_000, 1);
        assert!(axis.cached.is_none());
        let mut small = Axis::new(29, 13);
        let expected: Vec<_> = small.taps(6).map(|tap| (tap.index, tap.weight)).collect();
        small.cached = None;
        assert_eq!(
            small
                .taps(6)
                .map(|tap| (tap.index, tap.weight))
                .collect::<Vec<_>>(),
            expected
        );
    }

    #[test]
    fn neutral_owned_preparation_keeps_buffers_and_exact_samples() {
        for hdr in [false, true] {
            let image = ExportImage::new_16(
                401,
                257,
                (0..401 * 257 * 4)
                    .map(|i| ((i as u64 * 17_123 + 97) % 65_536) as u16)
                    .collect(),
            )
            .unwrap();
            let expected = image.clone();
            let (eight, sixteen) = (
                image.rgba8.as_ptr(),
                image.rgba16.as_ref().unwrap().as_ptr(),
            );
            let settings = ExportSettings::default();
            let prepared = if hdr {
                prepare_pq_image_owned(image, Default::default(), &settings)
            } else {
                prepare_image_already_converted_owned(image, Default::default(), &settings)
            };
            assert_eq!(prepared, expected);
            assert_eq!(prepared.rgba8.as_ptr(), eight);
            assert_eq!(prepared.rgba16.as_ref().unwrap().as_ptr(), sixteen);
        }
    }
    #[test]
    fn convert_space_encodes_adobe_rgb_with_its_own_gamma() {
        use super::*;
        // sRGB red, green and mid grey → Adobe RGB (1998) 8-bit, computed
        // from the published matrices and the 563/256 gamma.
        let cases: [([u8; 3], [u8; 3]); 3] = [
            ([255, 0, 0], [219, 0, 0]),
            ([0, 255, 0], [144, 255, 60]),
            ([128, 128, 128], [127, 127, 127]),
        ];
        for (input, expected) in cases {
            let mut image =
                ExportImage::new(1, 1, vec![input[0], input[1], input[2], 255]).expect("image");
            convert_space(&mut image, ExportColorSpace::AdobeRgb);
            for channel in 0..3 {
                let got = image.rgba8[channel];
                assert!(
                    (i32::from(got) - i32::from(expected[channel])).abs() <= 2,
                    "{input:?} → {:?}, expected {expected:?}",
                    &image.rgba8[..3]
                );
            }
        }
    }

    use super::*;
    use crate::{ExportSettings, SizeMode};

    #[test]
    fn constructors_reject_empty_and_overflowing_dimensions() {
        for (width, height) in [(0, 1), (1, 0), (u32::MAX, u32::MAX)] {
            assert!(ExportImage::new(width, height, vec![]).is_none());
            assert!(ExportImage::new_16(width, height, vec![]).is_none());
        }
    }

    fn asym() -> ExportImage {
        ExportImage::new(2, 1, vec![255, 0, 0, 255, 0, 0, 255, 255]).expect("valid")
    }

    #[test]
    fn horizontal_flip_maps_pixels() {
        let geometry = GeometryParams {
            flip_horizontal: true,
            ..Default::default()
        };
        let out = prepare_image(&asym(), geometry, &ExportSettings::default());
        assert!(out.rgba8[2] > 240, "left became blue");
        assert!(out.rgba8[4] > 240, "right became red");
    }

    #[test]
    fn quarter_turn_swaps_dimensions_and_vertical_flip_maps_pixels() {
        let turned = prepare_image(
            &ExportImage::new(2, 1, vec![255, 0, 0, 255, 0, 0, 255, 255]).unwrap(),
            GeometryParams {
                quarter_turns: 1,
                ..Default::default()
            },
            &ExportSettings::default(),
        );
        assert_eq!((turned.width, turned.height), (1, 2));
        let flipped = prepare_image(
            &ExportImage::new(1, 2, vec![255, 0, 0, 255, 0, 0, 255, 255]).unwrap(),
            GeometryParams {
                flip_vertical: true,
                ..Default::default()
            },
            &ExportSettings::default(),
        );
        assert!(flipped.rgba8[2] > 240);
    }

    #[test]
    fn border_is_applied_at_final_size() {
        let image = ExportImage::new(10, 10, vec![128; 400]).unwrap();
        let settings = ExportSettings {
            border: Some(crate::Border {
                width_percent: 10.0,
                rgb: [1, 2, 3],
            }),
            ..Default::default()
        };
        let output = prepare_image(&image, GeometryParams::default(), &settings);
        assert_eq!(&output.rgba8[..3], &[1, 2, 3]);
        let center = ((5 * 10 + 5) * 4) as usize;
        assert_ne!(&output.rgba8[center..center + 3], &[1, 2, 3]);
    }

    #[test]
    fn sizing_is_applied_after_crop() {
        let settings = ExportSettings {
            size: SizeMode::Exact {
                width: 1,
                height: 1,
            },
            ..Default::default()
        };
        let out = prepare_image(&asym(), GeometryParams::default(), &settings);
        assert_eq!((out.width, out.height), (1, 1));
    }

    #[test]
    fn identity_geometry_preserves_sixteen_bit_samples() {
        let source = ExportImage::new_16(
            2,
            1,
            vec![123, 12_345, 65_000, 65_535, 32_001, 40_003, 50_007, 65_535],
        )
        .expect("fixture");
        let prepared = prepare_image(
            &source,
            GeometryParams::default(),
            &ExportSettings::default(),
        );
        let actual = prepared.rgba16.expect("high precision retained");
        let expected = source.rgba16.expect("source");
        assert!(actual
            .iter()
            .zip(expected)
            .all(|(actual, expected)| actual.abs_diff(expected) <= 1));
    }

    #[test]
    fn output_sharpening_preserves_flat_color() {
        let source = ExportImage::new(3, 3, [128, 96, 64, 255].repeat(9)).expect("fixture");
        let settings = ExportSettings {
            output_sharpening: OutputSharpening::High,
            ..ExportSettings::default()
        };
        let output = prepare_image(&source, GeometryParams::default(), &settings);
        assert_eq!(output.rgba8, source.rgba8);
    }

    #[test]
    fn output_sharpening_increases_edge_contrast() {
        let mut pixels = Vec::new();
        for _ in 0..3 {
            for value in [64, 96, 128, 160, 192] {
                pixels.extend_from_slice(&[value, value, value, 255]);
            }
        }
        let source = ExportImage::new(5, 3, pixels).expect("fixture");
        let sharpened = output_sharpen(&source, OutputSharpening::High, Transfer::Srgb);
        const LEFT: usize = 24;
        const RIGHT: usize = 32;
        let before = i16::from(source.rgba8[RIGHT]) - i16::from(source.rgba8[LEFT]);
        let after = i16::from(sharpened.rgba8[RIGHT]) - i16::from(sharpened.rgba8[LEFT]);
        assert!(after > before, "{after} should exceed {before}");
    }
}
