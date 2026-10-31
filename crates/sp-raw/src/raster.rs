//! Color-managed rendered-image decoding. Original pixels/profile remain
//! untouched on disk; the develop source is linear Rec.2020 float RGB.

use crate::{Preview, SourceOutput};
use image::{DynamicImage, ImageDecoder, ImageReader};
use moxcms::{ColorProfile, Layout, ToneReprCurve, TransformOptions};
use sp_core::source::{ImageSource, RasterImage};
use sp_core::{ImageMeta, SpError};
use std::path::Path;
use std::sync::Arc;

#[cfg(target_os = "macos")]
mod imageio;
#[cfg(feature = "native-codecs")]
mod native;

#[cfg(any(feature = "native-codecs", target_os = "macos"))]
fn is_heif(path: &Path) -> bool {
    path.extension()
        .and_then(|v| v.to_str())
        .is_some_and(|v| ["avif", "heic", "heif"].contains(&v.to_ascii_lowercase().as_str()))
}

#[cfg(target_os = "macos")]
pub(super) fn decode_preview(path: &Path) -> Result<crate::PreviewOutput, SpError> {
    let output =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| imageio::decode(path, true)))
            .map_err(|_| {
            SpError::decode(
                path.display().to_string(),
                "image decoder panicked on malformed input",
            )
        })??;
    Ok(crate::PreviewOutput {
        preview: output.preview,
        meta: output.meta,
    })
}

#[cfg(target_os = "macos")]
pub(super) fn has_system_preview(path: &Path) -> bool {
    is_heif(path)
}

pub(super) fn decode(path: &Path) -> Result<SourceOutput, SpError> {
    let fail = |error: String| SpError::decode(path.display().to_string(), error);
    #[cfg(any(feature = "native-codecs", target_os = "macos"))]
    if is_heif(path) {
        #[cfg(feature = "native-codecs")]
        match native::decode(path) {
            Ok(output) => return Ok(output),
            #[cfg(not(target_os = "macos"))]
            Err(error) => return Err(error),
            #[cfg(target_os = "macos")]
            Err(error) => tracing::debug!(%error, "libheif failed; trying macOS ImageIO"),
        }
        #[cfg(target_os = "macos")]
        return imageio::decode(path, false);
    }
    if path
        .extension()
        .and_then(|v| v.to_str())
        .is_some_and(|v| v.eq_ignore_ascii_case("jxl"))
    {
        let file =
            std::fs::File::open(path).map_err(|e| SpError::io(path.display().to_string(), e))?;
        let decoder =
            jxl_oxide::integration::JxlDecoder::new(file).map_err(|e| fail(e.to_string()))?;
        // JPEG XL's codestream orientation has already been applied by the
        // decoder. Its optional EXIF copy must not rotate the pixels twice.
        return decode_with_decoder(path, decoder, true);
    }
    let mut reader = ImageReader::open(path)
        .map_err(|e| SpError::io(path.display().to_string(), e))?
        .with_guessed_format()
        .map_err(|e| fail(e.to_string()))?;
    if reader.format() == Some(image::ImageFormat::Tiff) {
        // image 0.25's TIFF set_limits caps metadata allocation at the
        // pixel buffer size: on small files a perfectly valid ICC is then
        // silently returned as absent. Read metadata before applying the
        // pixel allocation limits in decode_with_decoder.
        let decoder = image::codecs::tiff::TiffDecoder::new(reader.into_inner())
            .map_err(|e| fail(e.to_string()))?;
        return decode_with_decoder(path, decoder, false);
    }
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(2_500_000_000);
    reader.limits(limits);
    let decoder = reader.into_decoder().map_err(|e| fail(e.to_string()))?;
    decode_with_decoder(path, decoder, false)
}

fn decode_with_decoder(
    path: &Path,
    mut decoder: impl ImageDecoder,
    already_oriented: bool,
) -> Result<SourceOutput, SpError> {
    let fail = |error: String| SpError::decode(path.display().to_string(), error);
    let color = decoder.color_type();
    let bits = color.bits_per_pixel() / u16::from(color.channel_count());
    let orientation = if already_oriented {
        image::metadata::Orientation::NoTransforms
    } else {
        decoder.orientation().map_err(|e| fail(e.to_string()))?
    };
    let exif = decoder.exif_metadata().ok().flatten();
    // TIFF's ImageDecoder does not expose a raw EXIF block, so also allow
    // the metadata reader to inspect its TIFF IFDs directly.
    let metadata = exif
        .as_ref()
        .and_then(|bytes| exif::Reader::new().read_raw(bytes.clone()).ok())
        .or_else(|| {
            std::fs::File::open(path).ok().and_then(|file| {
                exif::Reader::new()
                    .read_from_container(&mut std::io::BufReader::new(file))
                    .ok()
            })
        });
    let embedded_icc = decoder.icc_profile().map_err(|e| fail(e.to_string()))?;
    if bits == 32 && embedded_icc.is_none() {
        return Err(fail("Floating-point photographs require an embedded RGB ICC profile so their transfer function is unambiguous".into()));
    }
    let (width, height) = decoder.dimensions();
    if u64::from(width) * u64::from(height) > 150_000_000 {
        return Err(fail("Raster source exceeds the 150 MP memory limit".into()));
    }
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(2_500_000_000);
    decoder
        .set_limits(limits)
        .map_err(|e| fail(e.to_string()))?;
    let mut decoded = DynamicImage::from_decoder(decoder).map_err(|e| fail(e.to_string()))?;
    decoded.apply_orientation(orientation);
    let rgba = decoded.into_rgba32f();
    let (width, height) = rgba.dimensions();
    if width == 0 || height == 0 {
        return Err(fail("Image has no pixels".into()));
    }
    if rgba
        .pixels()
        .any(|p| !p.0.iter().all(|v| v.is_finite()) || p[3] < 1.0)
    {
        return Err(fail("Transparent or non-finite pixels are not supported by the photo develop pipeline. Flatten transparency against your chosen background before opening.".into()));
    }
    let encoded: Vec<f32> = rgba.pixels().flat_map(|p| [p[0], p[1], p[2]]).collect();
    drop(rgba);
    let linear = convert_to_working(&encoded, embedded_icc.as_deref()).map_err(fail)?;
    drop(encoded);
    let pixels: Vec<[f32; 4]> = linear
        .chunks_exact(3)
        .map(|p| [p[0], p[1], p[2], 1.0])
        .collect();
    drop(linear);
    let preview = preview(width, height, &pixels);
    Ok(SourceOutput {
        source: ImageSource::Raster(Arc::new(RasterImage {
            width,
            height,
            linear_rec2020: pixels,
            bits_per_channel: bits as u8,
            embedded_icc,
        })),
        meta: metadata.as_ref().map(capture_metadata).unwrap_or_default(),
        preview: Some(preview),
    })
}

fn capture_metadata(exif: &exif::Exif) -> ImageMeta {
    use exif::{In, Tag, Value};
    let text = |tag| {
        exif.get_field(tag, In::PRIMARY)
            .and_then(|field| match &field.value {
                Value::Ascii(parts) => parts.first().map(|v| {
                    String::from_utf8_lossy(v)
                        .trim_matches('\0')
                        .trim()
                        .to_owned()
                }),
                _ => None,
            })
    };
    let number = |tag| {
        exif.get_field(tag, In::PRIMARY)
            .and_then(|field| match &field.value {
                Value::Rational(values) => values
                    .first()
                    .filter(|v| v.denom != 0)
                    .map(|v| v.num as f32 / v.denom as f32),
                _ => field.value.get_uint(0).map(|v| v as f32),
            })
    };
    let coordinate = |tag, reference, negative| {
        let Value::Rational(values) = &exif.get_field(tag, In::PRIMARY)?.value else {
            return None;
        };
        if values.len() < 3 || values[..3].iter().any(|v| v.denom == 0) {
            return None;
        }
        let degrees = values[0].to_f64() + values[1].to_f64() / 60.0 + values[2].to_f64() / 3600.0;
        Some(if text(reference).as_deref() == Some(negative) {
            -degrees
        } else {
            degrees
        })
    };
    let gps = coordinate(Tag::GPSLatitude, Tag::GPSLatitudeRef, "S")
        .zip(coordinate(Tag::GPSLongitude, Tag::GPSLongitudeRef, "W"))
        .filter(|(lat, lon)| lat.abs() <= 90.0 && lon.abs() <= 180.0)
        .map(|(lat, lon)| [lat, lon]);
    let captured_at = text(Tag::DateTimeOriginal).and_then(|value| {
        if value.len() != 19 || !value.is_ascii() {
            return None;
        }
        let date = format!(
            "{}-{}-{}T{}",
            &value[0..4],
            &value[5..7],
            &value[8..10],
            &value[11..]
        );
        // A timestamp without a known timezone is retained without inventing
        // UTC. Original EXIF remains untouched on disk.
        Some(format!(
            "{date}{}",
            text(Tag::OffsetTimeOriginal).unwrap_or_default()
        ))
    });
    ImageMeta {
        camera_make: text(Tag::Make).unwrap_or_default(),
        camera_model: text(Tag::Model).unwrap_or_default(),
        lens_model: text(Tag::LensModel),
        iso: number(Tag::PhotographicSensitivity).map(|v| v as u32),
        shutter_s: number(Tag::ExposureTime),
        aperture_f: number(Tag::FNumber),
        focal_length_mm: number(Tag::FocalLength),
        captured_at,
        gps,
    }
}

fn convert_to_working(encoded: &[f32], icc: Option<&[u8]>) -> Result<Vec<f32>, String> {
    // Untagged photographic files use sRGB. Do not silently ignore an
    // invalid/unsupported embedded profile: that produces misleading color.
    let source = match icc {
        Some(bytes) => ColorProfile::new_from_slice(bytes)
            .map_err(|e| format!("Invalid embedded ICC profile: {e}"))?,
        None => ColorProfile::new_srgb(),
    };
    if source.color_space != moxcms::DataColorSpace::Rgb {
        return Err(
            "Embedded ICC profile is not RGB; convert the photograph to RGB before opening".into(),
        );
    }
    let luminance_scale = match source.cicp.as_ref().map(|c| c.transfer_characteristics) {
        Some(moxcms::TransferCharacteristics::Smpte2084) => 10000.0 / 203.0,
        Some(moxcms::TransferCharacteristics::Hlg) => return Err("HLG input needs an explicit display-reference interpretation; convert to an ICC-tagged RGB TIFF before opening".into()),
        _ => 1.0,
    };
    let mut working = ColorProfile::new_bt2020();
    let linear = ToneReprCurve::Lut(Vec::new());
    working.red_trc = Some(linear.clone());
    working.green_trc = Some(linear.clone());
    working.blue_trc = Some(linear);
    let transform = source
        .create_transform_f32(
            Layout::Rgb,
            &working,
            Layout::Rgb,
            TransformOptions {
                allow_extended_range_rgb_xyz: true,
                ..Default::default()
            },
        )
        .map_err(|e| format!("Embedded ICC transform failed: {e}"))?;
    let mut converted = vec![0.0; encoded.len()];
    transform
        .transform(encoded, &mut converted)
        .map_err(|e| e.to_string())?;
    if luminance_scale != 1.0 {
        for value in &mut converted {
            *value *= luminance_scale;
        }
    }
    if converted.iter().any(|v| !v.is_finite()) {
        return Err("ICC conversion produced non-finite pixels".into());
    }
    Ok(converted)
}

fn preview(width: u32, height: u32, pixels: &[[f32; 4]]) -> Preview {
    let scale = (512.0 / width.max(height) as f32).min(1.0);
    let w = (width as f32 * scale).round().max(1.0) as u32;
    let h = (height as f32 * scale).round().max(1.0) as u32;
    let mut rgba8 = Vec::with_capacity(w as usize * h as usize * 4);
    for y in 0..h {
        for x in 0..w {
            let source = pixels[((y as u64 * height as u64 / h as u64) * width as u64
                + x as u64 * width as u64 / w as u64) as usize];
            let rgb = sp_core::color::mat3_apply(
                &sp_core::color::REC2020_TO_REC709,
                [source[0], source[1], source[2]],
            );
            rgba8.extend(
                rgb.map(|v| (sp_core::color::srgb_oetf(v).clamp(0.0, 1.0) * 255.0).round() as u8),
            );
            rgba8.push(255);
        }
    }
    Preview {
        width: w,
        height: h,
        rgba8,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sixteen_bit_tiff_values_are_not_reduced_to_eight_bits() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("gradient.tiff");
        let image = image::ImageBuffer::<image::Rgb<u16>, _>::from_raw(
            2,
            1,
            vec![32768, 32768, 32768, 32800, 32800, 32800],
        )
        .unwrap();
        image.save(&path).unwrap();
        let output = decode(&path).unwrap();
        let ImageSource::Raster(image) = output.source else {
            panic!("raster source")
        };
        assert_eq!(image.bits_per_channel, 16);
        assert!(image.linear_rec2020[1][0] > image.linear_rec2020[0][0]);
        assert!(
            (image.linear_rec2020[0][0] - sp_core::color::srgb_eotf(32768.0 / 65535.0)).abs()
                < 0.0002
        );
    }

    #[test]
    fn embedded_p3_is_transformed_and_invalid_profiles_are_rejected() {
        let profile = ColorProfile::new_display_p3().encode().unwrap();
        let rgb = convert_to_working(&[1.0, 0.0, 0.0], Some(&profile)).unwrap();
        let srgb = convert_to_working(&[1.0, 0.0, 0.0], None).unwrap();
        assert!((rgb[0] - srgb[0]).abs() > 0.05);
        assert!(convert_to_working(&[0.5; 3], Some(b"broken")).is_err());
    }

    #[test]
    fn transparency_is_explicitly_rejected() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("cutout.png");
        image::RgbaImage::from_pixel(2, 2, image::Rgba([255, 0, 0, 127]))
            .save(&path)
            .unwrap();
        assert!(decode(&path)
            .unwrap_err()
            .to_string()
            .contains("Transparent"));
    }

    #[test]
    fn basic_formats_decode_as_raster_sources() {
        let directory = tempfile::tempdir().unwrap();
        for extension in ["jpg", "png", "tif", "webp", "bmp"] {
            let path = directory.path().join(format!("photo.{extension}"));
            image::RgbImage::from_pixel(3, 2, image::Rgb([60, 100, 180]))
                .save(&path)
                .unwrap();
            let output = crate::decode_source(&path).unwrap();
            assert_eq!(output.source.dimensions(), (3, 2));
            assert!(!output.source.is_raw());
        }
    }

    #[test]
    fn jpeg_xl_reopens_high_precision_color_managed_export() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("photo.jxl");
        let source = sp_export::ExportImage::new_16(
            2,
            1,
            vec![32768, 12000, 9000, 65535, 32832, 12000, 9000, 65535],
        )
        .unwrap();
        let settings = sp_export::ExportSettings {
            format: sp_export::ExportFormat::JpegXl {
                quality: 100,
                lossless: true,
            },
            color_space: sp_export::ExportColorSpace::DisplayP3,
            ..Default::default()
        };
        sp_export::encode_file(&source, &settings, &path).unwrap();
        let decoded = crate::decode_source(&path).unwrap();
        let ImageSource::Raster(image) = decoded.source else {
            panic!("raster source")
        };
        assert_eq!(image.bits_per_channel, 16);
        assert!(image.embedded_icc.is_some());
        assert!(image.linear_rec2020[1][0] > image.linear_rec2020[0][0]);
    }

    #[test]
    fn pq_jpeg_xl_input_uses_the_pipeline_203_nit_reference_white() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("hdr.jxl");
        let signal = (sp_core::color::pq_oetf(1000.0 / 10000.0) * 65535.0).round() as u16;
        let image = sp_export::ExportImage::new_16(2, 2, [signal, signal, signal, 65535].repeat(4))
            .unwrap();
        let settings = sp_export::ExportSettings {
            format: sp_export::ExportFormat::JpegXl {
                quality: 100,
                lossless: true,
            },
            ..Default::default()
        };
        sp_export::encode_file_hdr(&image, &settings, &path, true).unwrap();
        let output = decode(&path).unwrap();
        let ImageSource::Raster(image) = output.source else {
            panic!("raster")
        };
        for channel in &image.linear_rec2020[0][..3] {
            assert!(
                (*channel - 1000.0 / 203.0).abs() < 0.03,
                "wrong HDR source scale: {channel}"
            );
        }
    }

    #[test]
    fn png_exif_is_retained_and_orientation_applied_once() {
        use exif::{Field, In, Tag, Value};
        use image::ImageEncoder;
        let fields = vec![
            Field {
                tag: Tag::Make,
                ifd_num: In::PRIMARY,
                value: Value::Ascii(vec![b"Hasselblad".to_vec()]),
            },
            Field {
                tag: Tag::Model,
                ifd_num: In::PRIMARY,
                value: Value::Ascii(vec![b"X2D II 100C".to_vec()]),
            },
            Field {
                tag: Tag::LensModel,
                ifd_num: In::PRIMARY,
                value: Value::Ascii(vec![b"XCD 55V".to_vec()]),
            },
            Field {
                tag: Tag::Orientation,
                ifd_num: In::PRIMARY,
                value: Value::Short(vec![6]),
            },
            Field {
                tag: Tag::PhotographicSensitivity,
                ifd_num: In::PRIMARY,
                value: Value::Short(vec![800]),
            },
            Field {
                tag: Tag::DateTimeOriginal,
                ifd_num: In::PRIMARY,
                value: Value::Ascii(vec![b"2026:09:29 12:34:56".to_vec()]),
            },
            Field {
                tag: Tag::OffsetTimeOriginal,
                ifd_num: In::PRIMARY,
                value: Value::Ascii(vec![b"+02:00".to_vec()]),
            },
            Field {
                tag: Tag::GPSLatitudeRef,
                ifd_num: In::PRIMARY,
                value: Value::Ascii(vec![b"N".to_vec()]),
            },
            Field {
                tag: Tag::GPSLongitudeRef,
                ifd_num: In::PRIMARY,
                value: Value::Ascii(vec![b"W".to_vec()]),
            },
            Field {
                tag: Tag::GPSLatitude,
                ifd_num: In::PRIMARY,
                value: Value::Rational(vec![(52, 1).into(), (15, 1).into(), (0, 1).into()]),
            },
            Field {
                tag: Tag::GPSLongitude,
                ifd_num: In::PRIMARY,
                value: Value::Rational(vec![(4, 1).into(), (30, 1).into(), (0, 1).into()]),
            },
        ];
        let mut writer = exif::experimental::Writer::new();
        for field in &fields {
            writer.push_field(field);
        }
        let mut exif = std::io::Cursor::new(Vec::new());
        writer.write(&mut exif, true).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("portrait.png");
        let mut encoder =
            image::codecs::png::PngEncoder::new(std::fs::File::create(&path).unwrap());
        encoder.set_exif_metadata(exif.into_inner()).unwrap();
        encoder
            .write_image(&[100; 3 * 2 * 3], 3, 2, image::ExtendedColorType::Rgb8)
            .unwrap();
        let decoded = crate::decode_source(&path).unwrap();
        assert_eq!(decoded.source.dimensions(), (2, 3));
        assert!(
            !decoded.source.is_raw(),
            "camera EXIF does not make rendered pixels RAW"
        );
        assert_eq!(decoded.meta.camera_model, "X2D II 100C");
        assert_eq!(decoded.meta.iso, Some(800));
        assert_eq!(decoded.meta.lens_model.as_deref(), Some("XCD 55V"));
        assert_eq!(
            decoded.meta.captured_at.as_deref(),
            Some("2026-09-29T12:34:56+02:00")
        );
        assert_eq!(decoded.meta.gps, Some([52.25, -4.5]));
    }
}
