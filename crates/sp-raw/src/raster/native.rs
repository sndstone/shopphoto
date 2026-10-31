//! Optional libheif input. Native library/decoder plugins are a packaging
//! responsibility; portable builds do not advertise these extensions.

use super::*;
use libheif_rs::{ColorSpace, DecodingOptions, HeifContext, LibHeif, RgbChroma};

pub(super) fn decode(path: &Path) -> Result<SourceOutput, SpError> {
    let fail = |message: String| SpError::decode(path.display().to_string(), message);
    let filename = path
        .to_str()
        .ok_or_else(|| fail("Native codec requires a UTF-8 path".into()))?;
    let context = HeifContext::read_from_file(filename).map_err(|e| fail(e.to_string()))?;
    let handle = context
        .primary_image_handle()
        .map_err(|e| fail(e.to_string()))?;
    if u64::from(handle.width()) * u64::from(handle.height()) > 150_000_000 {
        return Err(fail("Raster source exceeds the 150 MP memory limit".into()));
    }
    let embedded_icc = handle.color_profile_raw().map(|p| p.data);
    let nclx = handle.color_profile_nclx();
    let mut options =
        DecodingOptions::new().ok_or_else(|| fail("Could not initialize HEIF decoder".into()))?;
    options.set_convert_hdr_to_8bit(false);
    options.set_ignore_transformations(false);
    let high_depth = handle.luma_bits_per_pixel() > 8;
    let chroma = if high_depth {
        RgbChroma::HdrRgbaLe
    } else {
        RgbChroma::Rgba
    };
    let decoded = LibHeif::new()
        .decode(&handle, ColorSpace::Rgb(chroma), Some(options))
        .map_err(|e| {
            fail(format!(
                "Native HEIF/AVIF decoder unavailable or rejected this file: {e}"
            ))
        })?;
    let plane = decoded
        .planes()
        .interleaved
        .ok_or_else(|| fail("HEIF decoder returned no RGB plane".into()))?;
    let (width, height) = (plane.width, plane.height);
    let depth = plane.bits_per_pixel;
    if width == 0 || height == 0 || !(1..=16).contains(&depth) {
        return Err(fail(
            "Unsupported HEIF pixel dimensions or bit depth".into(),
        ));
    }
    let maximum = ((1u32 << depth) - 1) as f32;
    let bytes_per_pixel = if high_depth { 8 } else { 4 };
    let row_bytes = width as usize * bytes_per_pixel;
    if plane.stride < row_bytes || plane.data.len() < plane.stride * height as usize {
        return Err(fail("Truncated HEIF pixel plane".into()));
    }
    let mut encoded = Vec::with_capacity(width as usize * height as usize * 3);
    for row in plane.data.chunks(plane.stride).take(height as usize) {
        for pixel in row[..row_bytes].chunks_exact(bytes_per_pixel) {
            let rgba = if high_depth {
                [0, 1, 2, 3]
                    .map(|c| u16::from_le_bytes([pixel[c * 2], pixel[c * 2 + 1]]) as f32 / maximum)
            } else {
                [0, 1, 2, 3].map(|c| pixel[c] as f32 / maximum)
            };
            if rgba[3] < 1.0 {
                return Err(fail("Transparent photographs are unsupported; flatten against your chosen background before opening".into()));
            }
            encoded.extend_from_slice(&rgba[..3]);
        }
    }
    // Honor NCLX when there is no ICC; assuming sRGB for a tagged P3 or
    // Rec.2020 file would silently shift its colors.
    let derived_icc = if embedded_icc.is_none() {
        nclx.as_ref().map(|profile| {
            let primaries_id = profile.color_primaries() as u8;
            let transfer_id = profile.transfer_characteristics() as u8;
            if transfer_id == 18 {
                return Err("HLG HEIF/AVIF input needs an explicit display-reference interpretation; convert to an ICC-tagged RGB TIFF before opening".to_owned());
            }
            let primaries = moxcms::CicpColorPrimaries::try_from(if primaries_id == 2 { 1 } else { primaries_id }).map_err(|e| e.to_string())?;
            let transfer = moxcms::TransferCharacteristics::try_from(if transfer_id == 2 { 13 } else { transfer_id }).map_err(|e| e.to_string())?;
            let cicp = moxcms::CicpProfile { color_primaries: primaries, transfer_characteristics: transfer,
                matrix_coefficients: moxcms::MatrixCoefficients::Identity, full_range: true };
            let _: moxcms::ColorPrimaries = primaries.try_into().map_err(|e: moxcms::CmsError| e.to_string())?;
            let _: ToneReprCurve = transfer.try_into().map_err(|e: moxcms::CmsError| e.to_string())?;
            // moxcms 0.8's update method returns false even on success;
            // validate both components before constructing the profile.
            let color = ColorProfile::new_from_cicp(cicp);
            color.encode().map_err(|e| e.to_string())
        }).transpose().map_err(fail)?
    } else {
        None
    };
    let converted =
        convert_to_working(&encoded, embedded_icc.as_deref().or(derived_icc.as_deref()))
            .map_err(fail)?;
    drop(encoded);
    let pixels: Vec<_> = converted
        .chunks_exact(3)
        .map(|p| [p[0], p[1], p[2], 1.0])
        .collect();
    drop(converted);
    let meta = handle
        .all_metadata()
        .iter()
        .find_map(|item| {
            if item.item_type.to_string() != "Exif" || item.raw_data.len() < 4 {
                return None;
            }
            let offset = u32::from_be_bytes(item.raw_data[..4].try_into().ok()?) as usize;
            let bytes = item.raw_data.get(4usize.checked_add(offset)?..)?;
            exif::Reader::new()
                .read_raw(bytes.to_vec())
                .ok()
                .as_ref()
                .map(capture_metadata)
        })
        .unwrap_or_default();
    Ok(SourceOutput {
        preview: Some(preview(width, height, &pixels)),
        meta,
        source: ImageSource::Raster(Arc::new(RasterImage {
            width,
            height,
            linear_rec2020: pixels,
            bits_per_channel: depth,
            embedded_icc,
        })),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exported_avif_metadata_and_pixel_offsets_work_in_independent_decoder() {
        let directory = tempfile::tempdir().unwrap();
        for hdr in [false, true] {
            let path = directory.path().join(format!("metadata-{hdr}.avif"));
            let signal = if hdr {
                (sp_core::color::pq_oetf(1000.0 / 10000.0) * 65535.0).round() as u16
            } else {
                32768
            };
            let source =
                sp_export::ExportImage::new_16(16, 16, [signal, signal, signal, 65535].repeat(256))
                    .unwrap();
            let settings = sp_export::ExportSettings {
                format: sp_export::ExportFormat::Avif { quality: 100 },
                photo_metadata: sp_export::PhotoMetadata {
                    caption: "Native AVIF metadata validation".into(),
                    keywords: vec!["roundtrip".into()],
                    ..Default::default()
                },
                ..Default::default()
            };
            sp_export::encode_file_with_metadata(&source, &settings, &path, hdr, None).unwrap();
            let context = HeifContext::read_from_file(path.to_str().unwrap()).unwrap();
            let handle = context.primary_image_handle().unwrap();
            let metadata = handle.all_metadata();
            assert!(
                metadata
                    .iter()
                    .any(|item| String::from_utf8_lossy(&item.raw_data)
                        .contains("Native AVIF metadata validation")),
                "XMP metadata item absent"
            );
            if hdr {
                let color = handle.color_profile_nclx().expect("PQ NCLX");
                assert_eq!(color.transfer_characteristics() as u8, 16);
            } else {
                let icc = handle.color_profile_raw().expect("SDR ICC");
                ColorProfile::new_from_slice(&icc.data).expect("valid embedded ICC");
            }
            let decoded = LibHeif::new()
                .decode(&handle, ColorSpace::Rgb(RgbChroma::HdrRgbLe), None)
                .unwrap();
            let planes = decoded.planes();
            let plane = planes.interleaved.expect("RGB plane");
            assert_eq!((plane.width, plane.height), (16, 16));
            let actual = u16::from_le_bytes([plane.data[0], plane.data[1]]) as f32
                / ((1u32 << plane.bits_per_pixel) - 1) as f32;
            assert!(
                (actual - signal as f32 / 65535.0).abs() < 0.005,
                "relocated image data did not decode: {actual}"
            );
        }
    }

    #[test]
    fn pq_avif_reopens_with_hdr_headroom() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("hdr.avif");
        let signal = (sp_core::color::pq_oetf(1000.0 / 10000.0) * 65535.0).round() as u16;
        let source =
            sp_export::ExportImage::new_16(16, 16, [signal, signal, signal, 65535].repeat(256))
                .unwrap();
        let settings = sp_export::ExportSettings {
            format: sp_export::ExportFormat::Avif { quality: 100 },
            ..Default::default()
        };
        sp_export::encode_file_hdr(&source, &settings, &path, true).unwrap();
        let output = decode(&path).unwrap();
        let ImageSource::Raster(image) = output.source else {
            panic!("raster")
        };
        for channel in &image.linear_rec2020[0][..3] {
            // AV1 remains lossy at quality 100; a couple of 10-bit PQ
            // code values around 1000 nits represents roughly 2–3%.
            assert!(
                (*channel - 1000.0 / 203.0).abs() < 0.15,
                "HDR headroom lost: {channel}"
            );
        }
    }

    #[test]
    fn avif_ten_bit_export_reopens_with_its_color_description() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("native.avif");
        let source =
            sp_export::ExportImage::new_16(16, 16, [25000, 35000, 45000, 65535].repeat(16 * 16))
                .unwrap();
        let settings = sp_export::ExportSettings {
            format: sp_export::ExportFormat::Avif { quality: 100 },
            color_space: sp_export::ExportColorSpace::DisplayP3,
            ..Default::default()
        };
        sp_export::encode_file(&source, &settings, &path).unwrap();
        let output = decode(&path).unwrap();
        let ImageSource::Raster(image) = output.source else {
            panic!("raster")
        };
        assert!(image.bits_per_channel >= 10);
        let p3 = ColorProfile::new_display_p3().encode().unwrap();
        let expected = convert_to_working(
            &[25000.0 / 65535.0, 35000.0 / 65535.0, 45000.0 / 65535.0],
            Some(&p3),
        )
        .unwrap();
        for (expected, actual) in expected.iter().zip(&image.linear_rec2020[0][..3]) {
            assert!((expected - actual).abs() < 0.015);
        }
    }

    #[test]
    fn hevc_heif_rgb_decodes_without_a_bayer_stage() {
        use libheif_rs::{Channel, CompressionFormat, EncoderQuality, Image};
        let mut source = Image::new(16, 16, ColorSpace::Rgb(RgbChroma::Rgb)).unwrap();
        source
            .create_plane(Channel::Interleaved, 16, 16, 8)
            .unwrap();
        let planes = source.planes_mut();
        let plane = planes.interleaved.unwrap();
        for row in plane.data.chunks_mut(plane.stride).take(16) {
            for pixel in row[..16 * 3].chunks_exact_mut(3) {
                pixel.copy_from_slice(&[100, 150, 200]);
            }
        }
        let lib = LibHeif::new();
        let mut encoder = lib.encoder_for_format(CompressionFormat::Hevc).unwrap();
        encoder.set_quality(EncoderQuality::Lossy(100)).unwrap();
        let mut context = HeifContext::new().unwrap();
        context.encode_image(&source, &mut encoder, None).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("native.heic");
        std::fs::write(&path, context.write_to_bytes().unwrap()).unwrap();
        let output = decode(&path).unwrap();
        assert_eq!(output.source.dimensions(), (16, 16));
        assert!(!output.source.is_raw());
        let preview = output.preview.unwrap();
        for c in 0..3 {
            assert!(preview.rgba8[c].abs_diff([100, 150, 200][c]) < 10);
        }
    }
}
