//! Final-size photographic attribution. No editable document text or layers.
use crate::{ExportImage, ExportSettings, TextWatermark, WatermarkPosition};
use ab_glyph::{point, Font, FontRef, PxScale, ScaleFont};
use sp_core::color::{mat3_apply, mat3_inverse, pq_eotf, pq_oetf, srgb_eotf};

/// Convert an sRGB mark to linear target RGB. HDR marks use 203 cd/m² white.
pub(crate) fn mark_color(rgb: [u8; 3], settings: &ExportSettings, hdr: bool) -> [f32; 3] {
    let xyz = mat3_apply(
        &[
            [0.412391, 0.357584, 0.180481],
            [0.212639, 0.715169, 0.072192],
            [0.019331, 0.119195, 0.950532],
        ],
        rgb.map(|v| srgb_eotf(v as f32 / 255.0)),
    );
    let matrix = if hdr {
        Some(sp_core::color::XYZ_TO_REC2020)
    } else {
        match settings.color_space {
            crate::ExportColorSpace::Srgb => None,
            crate::ExportColorSpace::Rec2020 => Some(sp_core::color::XYZ_TO_REC2020),
            crate::ExportColorSpace::DisplayP3 => mat3_inverse(&[
                [0.486571, 0.265668, 0.198217],
                [0.228975, 0.691739, 0.079287],
                [0.0, 0.045113, 1.043944],
            ]),
            crate::ExportColorSpace::AdobeRgb => Some(sp_core::color::XYZ_TO_ADOBERGB),
        }
    };
    matrix
        .map_or_else(
            || rgb.map(|v| srgb_eotf(v as f32 / 255.0)),
            |m| mat3_apply(&m, xyz),
        )
        .map(|v| v.max(0.0) * if hdr { 203.0 / 10000.0 } else { 1.0 })
}
pub(crate) fn blend(
    image: &mut ExportImage,
    offset: usize,
    color: [f32; 3],
    alpha: f32,
    settings: &ExportSettings,
    hdr: bool,
) {
    if alpha <= 0.0 {
        return;
    }
    let alpha = alpha.clamp(0.0, 1.0);
    for c in 0..3 {
        let current = image
            .rgba16
            .as_ref()
            .map_or(image.rgba8[offset + c] as f32 / 255.0, |p| {
                p[offset + c] as f32 / 65535.0
            });
        let current = if hdr {
            pq_eotf(current)
        } else {
            settings.color_space.transfer().eotf(current)
        };
        let mixed = current * (1.0 - alpha) + color[c] * alpha;
        let encoded = if hdr {
            pq_oetf(mixed)
        } else {
            settings.color_space.transfer().oetf(mixed)
        }
        .clamp(0.0, 1.0);
        image.rgba8[offset + c] = (encoded * 255.0).round() as u8;
        if let Some(p) = &mut image.rgba16 {
            p[offset + c] = (encoded * 65535.0).round() as u16;
        }
    }
}
fn origin(image: &ExportImage, w: u32, h: u32, position: WatermarkPosition) -> (u32, u32) {
    let m = (image.width.min(image.height) / 50).max(1);
    match position {
        WatermarkPosition::TopLeft => (m, m),
        WatermarkPosition::TopRight => (image.width.saturating_sub(w + m), m),
        WatermarkPosition::BottomLeft => (m, image.height.saturating_sub(h + m)),
        WatermarkPosition::BottomRight => (
            image.width.saturating_sub(w + m),
            image.height.saturating_sub(h + m),
        ),
        WatermarkPosition::Center => (
            image.width.saturating_sub(w) / 2,
            image.height.saturating_sub(h) / 2,
        ),
    }
}
pub(crate) fn apply(mut image: ExportImage, settings: &ExportSettings, hdr: bool) -> ExportImage {
    let Some(TextWatermark {
        text,
        position,
        opacity,
        size_percent,
        rgb,
    }) = &settings.text_watermark
    else {
        return image;
    };
    if text.is_empty() || *opacity <= 0.0 {
        return image;
    }
    let font =
        FontRef::try_from_slice(epaint_default_fonts::UBUNTU_LIGHT).expect("bundled Ubuntu font");
    let text: String = text
        .chars()
        .filter(|c| !c.is_control())
        .take(1024)
        .collect();
    let requested =
        (image.width.min(image.height) as f32 * size_percent.clamp(0.5, 25.0) / 100.0).max(1.0);
    let mut scale = PxScale::from(requested);
    let measure = |scale| {
        let scaled = font.as_scaled(scale);
        let mut width = 0.0;
        let mut prev = None;
        for c in text.chars() {
            let id = font.glyph_id(c);
            if let Some(p) = prev {
                width += scaled.kern(p, id);
            }
            width += scaled.h_advance(id);
            prev = Some(id);
        }
        width
    };
    let available = image.width as f32 * 0.96;
    let width = measure(scale);
    if width > available {
        scale = PxScale::from(requested * available / width);
    }
    let scaled = font.as_scaled(scale);
    let (ox, oy) = origin(
        &image,
        measure(scale).ceil() as u32,
        scaled.height().ceil() as u32,
        *position,
    );
    let mut x = ox as f32;
    let baseline = oy as f32 + scaled.ascent();
    let mut previous = None;
    let color = mark_color(*rgb, settings, hdr);
    for c in text.chars() {
        let id = font.glyph_id(c);
        if let Some(p) = previous {
            x += scaled.kern(p, id);
        }
        let glyph = id.with_scale_and_position(scale, point(x, baseline));
        if let Some(outline) = font.outline_glyph(glyph) {
            let bounds = outline.px_bounds();
            outline.draw(|gx, gy, coverage| {
                let px = bounds.min.x as i32 + gx as i32;
                let py = bounds.min.y as i32 + gy as i32;
                if px >= 0 && py >= 0 && px < image.width as i32 && py < image.height as i32 {
                    let offset = (py as usize * image.width as usize + px as usize) * 4;
                    blend(
                        &mut image,
                        offset,
                        color,
                        coverage * opacity.clamp(0.0, 1.0),
                        settings,
                        hdr,
                    );
                }
            });
        }
        x += scaled.h_advance(id);
        previous = Some(id);
    }
    image
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mark_respects_position_opacity_and_preserves_unmarked_sixteen_bit_samples() {
        let input =
            ExportImage::new_16(320, 120, [12001, 12001, 12001, 65535].repeat(320 * 120)).unwrap();
        let mut settings = ExportSettings {
            text_watermark: Some(TextWatermark {
                text: "© Test".into(),
                size_percent: 18.0,
                ..Default::default()
            }),
            ..Default::default()
        };
        let marked = apply(input.clone(), &settings, false);
        assert_ne!(marked, input);
        assert_eq!(
            &marked.rgba16.as_ref().unwrap()[..320 * 60 * 4],
            &input.rgba16.as_ref().unwrap()[..320 * 60 * 4]
        );
        settings.text_watermark.as_mut().unwrap().opacity = 0.0;
        assert_eq!(apply(input.clone(), &settings, false), input);
    }
    #[test]
    fn hdr_mark_uses_diffuse_white_and_linear_alpha() {
        let settings = ExportSettings::default();
        let color = mark_color([255; 3], &settings, true);
        assert!((color[0] * 10000.0 - 203.0).abs() < 0.1);
        let mut image = ExportImage::new_16(1, 1, vec![0, 0, 0, 65535]).unwrap();
        blend(&mut image, 0, color, 0.5, &settings, true);
        let luminance = pq_eotf(image.rgba16.unwrap()[0] as f32 / 65535.0) * 10000.0;
        assert!((luminance - 101.5).abs() < 0.1);
    }
}
