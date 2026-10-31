//! Decoded sources. Rendered photographs enter the develop pipeline in
//! linear Rec.2020, without pretending to contain sensor/CFA samples.

use crate::RawImage;
use std::sync::Arc;

pub const SUPPORTED_RASTER_EXTENSIONS: &[&str] = &[
    "jpg",
    "jpeg",
    "png",
    "tif",
    "tiff",
    "webp",
    "bmp",
    "jxl",
    #[cfg(any(feature = "native-codecs", target_os = "macos"))]
    "avif",
    #[cfg(any(feature = "native-codecs", target_os = "macos"))]
    "heic",
    #[cfg(any(feature = "native-codecs", target_os = "macos"))]
    "heif",
];

#[derive(Debug, Clone)]
pub struct RasterImage {
    pub width: u32,
    pub height: u32,
    /// Upright, row-major linear Rec.2020 RGBA; alpha is always 1.0.
    /// Values are kept as floats, including out-of-gamut values.
    pub linear_rec2020: Vec<[f32; 4]>,
    pub bits_per_channel: u8,
    /// Original profile retained for provenance; pixels are already converted.
    pub embedded_icc: Option<Vec<u8>>,
}

impl RasterImage {
    /// Processing engines require complete, finite, opaque source frames.
    pub(crate) fn has_finite_opaque_pixels(&self) -> bool {
        self.width > 0
            && self.height > 0
            && (self.width as usize).checked_mul(self.height as usize)
                == Some(self.linear_rec2020.len())
            && self
                .linear_rec2020
                .iter()
                .all(|pixel| pixel.iter().all(|value| value.is_finite()) && pixel[3] == 1.0)
    }

    /// Creative WB relative to the already-rendered colors. 5500 K / zero
    /// tint is neutral in this control; it is not the camera's capture CCT.
    pub fn white_balance_gains(temp: Option<f32>, tint: f32) -> [f32; 3] {
        let matrix = crate::color::REC2020_TO_XYZ;
        let baseline = crate::wb::wb_gains_for_temp(&matrix, 5500.0, 0.0).unwrap_or([1.0; 3]);
        let gains =
            crate::wb::wb_gains_for_temp(&matrix, temp.unwrap_or(5500.0), tint).unwrap_or(baseline);
        [
            gains[0] / baseline[0],
            gains[1] / baseline[1],
            gains[2] / baseline[2],
        ]
    }

    pub fn neutral_white_balance(&self, u: f32, v: f32) -> Option<(f32, f32)> {
        if !u.is_finite() || !v.is_finite() || self.width == 0 || self.height == 0 {
            return None;
        }
        let x = (u.clamp(0.0, 1.0) * (self.width - 1) as f32).round() as u32;
        let y = (v.clamp(0.0, 1.0) * (self.height - 1) as f32).round() as u32;
        let mut sum = [0.0; 3];
        let mut count = 0.0;
        for sy in y.saturating_sub(2)..=(y + 2).min(self.height - 1) {
            for sx in x.saturating_sub(2)..=(x + 2).min(self.width - 1) {
                let pixel = self.linear_rec2020.get((sy * self.width + sx) as usize)?;
                if pixel[..3].iter().any(|c| *c >= 0.98 || *c <= 1e-5) {
                    continue;
                }
                for c in 0..3 {
                    sum[c] += pixel[c];
                }
                count += 1.0;
            }
        }
        if count == 0.0 {
            return None;
        }
        let matrix = crate::color::REC2020_TO_XYZ;
        let baseline = crate::wb::wb_gains_for_temp(&matrix, 5500.0, 0.0)?;
        crate::wb::temp_tint_for_neutral(
            &matrix,
            [
                sum[0] / count / baseline[0],
                sum[1] / count / baseline[1],
                sum[2] / count / baseline[2],
            ],
        )
    }
}

#[derive(Debug, Clone)]
pub enum ImageSource {
    Raw(Arc<RawImage>),
    Raster(Arc<RasterImage>),
}

impl ImageSource {
    pub fn as_raw(&self) -> Option<&Arc<RawImage>> {
        match self {
            Self::Raw(raw) => Some(raw),
            Self::Raster(_) => None,
        }
    }

    pub fn is_raw(&self) -> bool {
        matches!(self, Self::Raw(_))
    }

    pub fn dimensions(&self) -> (u32, u32) {
        match self {
            Self::Raw(raw) => (raw.active_area.width, raw.active_area.height),
            Self::Raster(image) => (image.width, image.height),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rendered_white_balance_is_neutral_until_adjusted_and_picker_neutralizes_rgb() {
        assert_eq!(RasterImage::white_balance_gains(None, 0.0), [1.0; 3]);
        assert_eq!(
            RasterImage::white_balance_gains(Some(5500.0), 0.0),
            [1.0; 3]
        );
        assert_ne!(RasterImage::white_balance_gains(None, 20.0), [1.0; 3]);
        let image = RasterImage {
            width: 3,
            height: 3,
            linear_rec2020: vec![[0.25, 0.3, 0.36, 1.0]; 9],
            bits_per_channel: 16,
            embedded_icc: None,
        };
        let (temp, tint) = image.neutral_white_balance(0.5, 0.5).unwrap();
        let gain = RasterImage::white_balance_gains(Some(temp), tint);
        assert!((0.25 * gain[0] - 0.3).abs() < 0.001);
        assert!((0.36 * gain[2] - 0.3).abs() < 0.001);
    }
}
