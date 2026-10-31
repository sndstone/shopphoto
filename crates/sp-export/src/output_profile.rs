//! Final RGB delivery transforms, using the same CMS as printer proofing.
use crate::{ExportError, ExportImage, ExportSettings};
use lcms2::{Flags, Intent, PixelFormat, Profile, Transform};
use serde::{Deserialize, Serialize};
use std::{borrow::Cow, io::Read, path::PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum OutputProfile {
    ProPhotoRgb,
    Custom(PathBuf),
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum OutputIntent {
    Perceptual,
    #[default]
    Relative,
    Saturation,
    Absolute,
}
impl OutputIntent {
    fn cms(self) -> Intent {
        match self {
            Self::Perceptual => Intent::Perceptual,
            Self::Relative => Intent::RelativeColorimetric,
            Self::Saturation => Intent::Saturation,
            Self::Absolute => Intent::AbsoluteColorimetric,
        }
    }
}
fn error(reason: impl ToString) -> ExportError {
    ExportError::Encode {
        path: "<output-profile>".into(),
        reason: reason.to_string(),
    }
}
impl OutputProfile {
    pub fn icc_bytes(&self) -> Result<Vec<u8>, ExportError> {
        let bytes = match self {
            Self::ProPhotoRgb => moxcms::ColorProfile::new_pro_photo_rgb()
                .encode()
                .map_err(error)?,
            Self::Custom(path) => {
                let file = std::fs::File::open(path).map_err(|source| ExportError::Io {
                    path: path.clone(),
                    source,
                })?;
                let mut bytes = Vec::new();
                file.take(32 * 1024 * 1024 + 1)
                    .read_to_end(&mut bytes)
                    .map_err(error)?;
                bytes
            }
        };
        if !(128..=32 * 1024 * 1024).contains(&bytes.len()) {
            return Err(error("ICC profile must be between 128 bytes and 32 MiB"));
        }
        if &bytes[16..20] != b"RGB " || !matches!(&bytes[12..16], b"mntr" | b"prtr" | b"spac") {
            return Err(error(
                "Export requires an RGB display, output, or color-space ICC profile",
            ));
        }
        Profile::new_icc(&bytes).map_err(error)?;
        Ok(bytes)
    }
}
pub(crate) fn convert<'a>(
    image: &'a ExportImage,
    settings: &ExportSettings,
    hdr: bool,
) -> Result<(Cow<'a, ExportImage>, Vec<u8>), ExportError> {
    let source_bytes = crate::icc_profile(settings.color_space)?;
    let Some(profile) = &settings.output_profile else {
        return Ok((Cow::Borrowed(image), source_bytes));
    };
    if hdr {
        return Err(error("RGB output profiles apply to SDR exports; choose the standard Rec.2100 profile for HDR"));
    }
    let bytes = profile.icc_bytes()?;
    let source = Profile::new_icc(&source_bytes).map_err(error)?;
    let target = Profile::new_icc(&bytes).map_err(error)?;
    let mut flags = Flags::COPY_ALPHA | Flags::NO_OPTIMIZE;
    if settings.output_black_point_compensation {
        flags = flags | Flags::BLACKPOINT_COMPENSATION;
    }
    let transform: Transform<[u16; 4], [u16; 4]> = Transform::new_flags(
        &source,
        PixelFormat::RGBA_16,
        &target,
        PixelFormat::RGBA_16,
        settings.output_intent.cms(),
        flags,
    )
    .map_err(error)?;
    let mut output = image.clone();
    let values = output
        .rgba16
        .get_or_insert_with(|| image.rgba8.iter().map(|v| u16::from(*v) * 257).collect());
    // Fixed row-sized scratch avoids another full floating-point image.
    let mut input = vec![[0; 4]; image.width as usize];
    let mut converted = input.clone();
    for row in values.chunks_exact_mut(image.width as usize * 4) {
        for (pixel, source) in input.iter_mut().zip(row.chunks_exact(4)) {
            pixel.copy_from_slice(source);
        }
        transform.transform_pixels(&input, &mut converted);
        for (pixel, target) in converted.iter().zip(row.chunks_exact_mut(4)) {
            target.copy_from_slice(pixel);
        }
    }
    for (small, large) in output.rgba8.iter_mut().zip(values) {
        *small = ((*large as u32 + 128) / 257) as u8;
    }
    Ok((Cow::Owned(output), bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn prophoto_conversion_retains_sixteen_bit_precision_and_round_trips() {
        let samples = vec![12001, 24003, 36005, 65535, 12031, 24033, 36035, 65535];
        let image = ExportImage::new_16(2, 1, samples.clone()).unwrap();
        let settings = ExportSettings {
            output_profile: Some(OutputProfile::ProPhotoRgb),
            ..Default::default()
        };
        let (converted, icc) = convert(&image, &settings, false).unwrap();
        assert_ne!(
            converted.rgba16.as_ref().unwrap()[0],
            converted.rgba16.as_ref().unwrap()[4]
        );
        let input = Profile::new_icc(&icc).unwrap();
        let target = Profile::new_icc(&crate::icc_profile(settings.color_space).unwrap()).unwrap();
        let transform: Transform<[u16; 4], [u16; 4]> = Transform::new_flags(
            &input,
            PixelFormat::RGBA_16,
            &target,
            PixelFormat::RGBA_16,
            Intent::RelativeColorimetric,
            Flags::COPY_ALPHA | Flags::NO_OPTIMIZE,
        )
        .unwrap();
        let pixels: Vec<[u16; 4]> = converted
            .rgba16
            .as_ref()
            .unwrap()
            .chunks_exact(4)
            .map(|p| p.try_into().unwrap())
            .collect();
        let mut back = [[0; 4]; 2];
        transform.transform_pixels(&pixels, &mut back);
        for (a, b) in back.into_iter().flatten().zip(samples) {
            assert!((i32::from(a) - i32::from(b)).abs() < 32, "{a} vs {b}");
        }
        assert!(convert(&image, &settings, true).is_err());
    }
    #[test]
    fn custom_profile_is_embedded_verbatim_and_invalid_profile_fails() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("output.icc");
        let profile = moxcms::ColorProfile::new_display_p3().encode().unwrap();
        std::fs::write(&path, &profile).unwrap();
        let settings = ExportSettings {
            output_profile: Some(OutputProfile::Custom(path.clone())),
            ..Default::default()
        };
        let image = ExportImage::new(1, 1, vec![190, 50, 75, 255]).unwrap();
        let (output, bytes) = convert(&image, &settings, false).unwrap();
        assert_eq!(bytes, profile);
        assert_ne!(output.rgba8, image.rgba8);
        std::fs::write(path, b"not a profile").unwrap();
        assert!(convert(&image, &settings, false).is_err());
    }
}
