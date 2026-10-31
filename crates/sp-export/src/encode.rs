//! Container encoders with ICC embedding and atomic destination replacement.

use crate::{ExportColorSpace, ExportError, ExportFormat, ExportImage, ExportSettings, TiffDepth};
use gamut_core::{Dimensions, EncodeImage, ImageRef, Rgb16};
use image::ImageEncoder;
use sp_core::ImageMeta;
use std::io::{BufWriter, Write};
use std::path::Path;

// A unique sibling keeps concurrent exports isolated and cleans up on errors.
fn temporary_output(destination: &Path) -> Result<tempfile::NamedTempFile, ExportError> {
    let parent = destination
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent).map_err(|source| ExportError::Io {
        path: parent.to_path_buf(),
        source,
    })?;
    tempfile::NamedTempFile::new_in(parent).map_err(|source| ExportError::Io {
        path: destination.to_path_buf(),
        source,
    })
}

fn validate_image(
    image: &ExportImage,
    format: ExportFormat,
    path: &Path,
) -> Result<(), ExportError> {
    let expected = u64::from(image.width)
        .checked_mul(u64::from(image.height))
        .and_then(|n| n.checked_mul(4));
    if image.width == 0
        || image.height == 0
        || expected != Some(image.rgba8.len() as u64)
        || image
            .rgba16
            .as_ref()
            .is_some_and(|pixels| Some(pixels.len() as u64) != expected)
    {
        return Err(image_error(
            path,
            "Invalid image dimensions or pixel buffer",
        ));
    }
    if matches!(
        format,
        ExportFormat::Jpeg { .. } | ExportFormat::UltraHdr { .. }
    ) && (image.width > u16::MAX as u32 || image.height > u16::MAX as u32)
    {
        return Err(image_error(
            path,
            "JPEG dimensions must not exceed 65535 pixels per side",
        ));
    }
    Ok(())
}

/// Generate the matching matrix-shaper ICC profile.
pub fn icc_profile(space: ExportColorSpace) -> Result<Vec<u8>, ExportError> {
    let profile = match space {
        ExportColorSpace::Srgb => moxcms::ColorProfile::new_srgb(),
        ExportColorSpace::DisplayP3 => moxcms::ColorProfile::new_display_p3(),
        ExportColorSpace::Rec2020 => {
            let mut profile = moxcms::ColorProfile::new_bt2020();
            // SDR renderers use the sRGB transfer curve with Rec.2020
            // primaries. The stock BT.2020 profile uses a Rec.709 curve.
            let srgb = moxcms::ColorProfile::new_srgb();
            profile.red_trc = srgb.red_trc;
            profile.green_trc = srgb.green_trc;
            profile.blue_trc = srgb.blue_trc;
            profile.cicp = None;
            profile
        }
        ExportColorSpace::AdobeRgb => moxcms::ColorProfile::new_adobe_rgb(),
    };
    profile.encode().map_err(|error| ExportError::Encode {
        path: "<icc-profile>".into(),
        reason: error.to_string(),
    })
}

fn image_error(path: &Path, error: impl ToString) -> ExportError {
    ExportError::Encode {
        path: path.to_path_buf(),
        reason: error.to_string(),
    }
}

/// Encode one prepared image and atomically replace the destination.
pub fn encode_file(
    image: &ExportImage,
    settings: &ExportSettings,
    destination: &Path,
) -> Result<(), ExportError> {
    encode_file_hdr(image, settings, destination, false)
}

/// Encode with optional Rec.2100 PQ signaling. HDR AVIF is written as a
/// 10-bit AV1 payload by ravif and tagged BT.2020/ST-2084; HDR PNG receives
/// the standardized cICP chunk.
pub fn encode_file_hdr(
    image: &ExportImage,
    settings: &ExportSettings,
    destination: &Path,
    hdr_pq: bool,
) -> Result<(), ExportError> {
    encode_file_with_metadata(image, settings, destination, hdr_pq, None)
}

/// Encode and optionally embed the decoded capture metadata. GPS fields are
/// never synthesized; consequently `strip_gps` is honored by construction.
/// Write an Ultra HDR JPEG: the SDR rendition as a baseline JPEG plus a
/// gain map derived from the PQ HDR rendition of the same edit.
///
/// `sdr` and `hdr` must be the SAME image at the same output size — the SDR
/// one sRGB/Display-P3/Rec.2020 encoded per `settings`, the HDR one PQ
/// encoded. The caller renders both; see the export worker in `sp-app`.
pub fn encode_ultra_hdr(
    sdr: &ExportImage,
    hdr: &ExportImage,
    settings: &ExportSettings,
    destination: &Path,
    metadata: Option<&ImageMeta>,
) -> Result<(), ExportError> {
    let ExportFormat::UltraHdr { quality } = settings.format else {
        return Err(ExportError::Encode {
            path: destination.to_path_buf(),
            reason: "encode_ultra_hdr called for a different format".into(),
        });
    };
    validate_image(sdr, settings.format, destination)?;
    validate_image(hdr, settings.format, destination)?;
    if settings.output_profile.is_some() {
        return Err(image_error(destination, "Ultra HDR uses its standard SDR base profile; custom output profiles are supported for SDR files"));
    }
    let map = crate::gainmap::compute_with_color_space(sdr, hdr, settings.color_space).ok_or_else(
        || ExportError::Encode {
            path: destination.to_path_buf(),
            reason: "SDR and HDR renditions differ in size".into(),
        },
    )?;

    let mut encoded = Vec::new();
    let mut encoder = jpeg_encoder::Encoder::new(&mut encoded, quality);
    // 4:4:4 always: an Ultra HDR base carries highlights that chroma
    // subsampling would smear across the gain map's own edges.
    encoder.set_sampling_factor(jpeg_encoder::SamplingFactor::F_1_1);
    encoder
        .encode(
            &sdr.rgb8(),
            u16::try_from(sdr.width).unwrap_or(u16::MAX),
            u16::try_from(sdr.height).unwrap_or(u16::MAX),
            jpeg_encoder::ColorType::Rgb,
        )
        .map_err(|error| image_error(destination, error))?;

    let exif = output_exif(metadata, settings);
    let description = crate::metadata::description(settings, metadata);
    let file = crate::ultrahdr::assemble_with_description(
        &encoded,
        &map,
        settings.color_space,
        exif.as_deref(),
        description.as_deref(),
        // The map is a low-frequency signal; blocking artifacts in it show
        // up as halos, so keep it well above the base quality.
        quality.max(90),
    )
    .map_err(|error| match error {
        ExportError::Encode { reason, .. } => ExportError::Encode {
            path: destination.to_path_buf(),
            reason,
        },
        other => other,
    })?;

    let mut temp = temporary_output(destination)?;
    temp.write_all(&file).map_err(|source| ExportError::Io {
        path: destination.to_path_buf(),
        source,
    })?;
    temp.persist(destination).map_err(|error| ExportError::Io {
        path: destination.to_path_buf(),
        source: error.error,
    })?;
    Ok(())
}

pub fn encode_file_with_metadata(
    image: &ExportImage,
    settings: &ExportSettings,
    destination: &Path,
    hdr_pq: bool,
    metadata: Option<&ImageMeta>,
) -> Result<(), ExportError> {
    let Some(limit) = settings.file_size_limit else {
        return encode_unlimited(image, settings, destination, hdr_pq, metadata);
    };
    let maximum = u64::from(limit.max_kib) * 1024;
    let mut settings = settings.clone();
    settings.file_size_limit = None;
    let maximum_quality = quality(&settings).ok_or_else(|| {
        image_error(
            destination,
            "File-size targets require JPEG, lossy WebP, AVIF, or lossy JPEG XL",
        )
    })?;
    let minimum = limit.minimum_quality.clamp(1, maximum_quality.max(1));
    let candidate = temporary_output(destination)?;
    let best = temporary_output(destination)?;
    let mut low = minimum;
    let mut high = maximum_quality.max(1);
    let mut selected = None;
    let mut smallest = u64::MAX;
    // Try the requested quality first. Then search for the best quality
    // within the byte budget, including actual ICC/EXIF/XMP overhead.
    let mut trial = high;
    loop {
        set_quality(&mut settings, trial);
        encode_unlimited(image, &settings, candidate.path(), hdr_pq, metadata)?;
        let length = std::fs::metadata(candidate.path())
            .map_err(|source| ExportError::Io {
                path: destination.into(),
                source,
            })?
            .len();
        smallest = smallest.min(length);
        if length <= maximum {
            std::fs::copy(candidate.path(), best.path()).map_err(|source| ExportError::Io {
                path: destination.into(),
                source,
            })?;
            selected = Some(trial);
            if trial == high {
                break;
            }
            low = trial + 1;
        } else {
            if trial <= low {
                break;
            }
            high = trial - 1;
        }
        if low > high {
            break;
        }
        trial = low + (high - low) / 2;
    }
    if selected.is_none() {
        return Err(image_error(destination, format!("Cannot meet {} KiB at quality {} or above (smallest trial: {} KiB). Reduce dimensions, metadata, or minimum quality.", limit.max_kib, minimum, smallest.div_ceil(1024))));
    }
    best.persist(destination).map_err(|error| ExportError::Io {
        path: destination.into(),
        source: error.error,
    })?;
    Ok(())
}

fn quality(settings: &ExportSettings) -> Option<u8> {
    match settings.format {
        ExportFormat::Jpeg { quality, .. }
        | ExportFormat::Avif { quality }
        | ExportFormat::JpegXl {
            quality,
            lossless: false,
        } => Some(quality.clamp(1, 100)),
        ExportFormat::WebP { lossless: false } => Some(settings.webp_quality.clamp(1, 100)),
        _ => None,
    }
}
fn set_quality(settings: &mut ExportSettings, value: u8) {
    match &mut settings.format {
        ExportFormat::Jpeg { quality, .. }
        | ExportFormat::Avif { quality }
        | ExportFormat::JpegXl { quality, .. } => *quality = value,
        ExportFormat::WebP { .. } => settings.webp_quality = value,
        _ => {}
    }
}

fn encode_unlimited(
    image: &ExportImage,
    settings: &ExportSettings,
    destination: &Path,
    hdr_pq: bool,
    metadata: Option<&ImageMeta>,
) -> Result<(), ExportError> {
    validate_image(image, settings.format, destination)?;
    if let Some(mark) = &settings.watermark {
        image::open(&mark.png)
            .map_err(|e| image_error(destination, format!("Could not read watermark: {e}")))?;
    }
    let (image, icc) = crate::output_profile::convert(image, settings, hdr_pq)?;
    let image = image.as_ref();
    let mut temp = temporary_output(destination)?;
    let tmp = temp.path().to_path_buf();
    let mut writer = BufWriter::new(temp.as_file_mut());
    let exif = output_exif(metadata, settings);
    let xmp = crate::metadata::packet(settings, metadata);
    match settings.format {
        ExportFormat::Jpeg {
            quality,
            chroma_444,
        } => {
            let mut encoded = Vec::new();
            let mut encoder = jpeg_encoder::Encoder::new(&mut encoded, quality);
            encoder.set_sampling_factor(if chroma_444 {
                jpeg_encoder::SamplingFactor::F_1_1
            } else {
                jpeg_encoder::SamplingFactor::F_2_1
            });
            encoder
                .encode(
                    &image.rgb8(),
                    image.width as u16,
                    image.height as u16,
                    jpeg_encoder::ColorType::Rgb,
                )
                .map_err(|error| image_error(destination, error))?;
            write_jpeg_with_metadata(&mut writer, &encoded, &icc, exif.as_deref(), xmp.as_deref())
                .map_err(|error| image_error(destination, error))?;
        }
        ExportFormat::Png { sixteen_bit } => {
            let mut encoder = image::codecs::png::PngEncoder::new(&mut writer);
            if !hdr_pq {
                encoder
                    .set_icc_profile(icc)
                    .map_err(|error| image_error(destination, error))?;
            }
            if let Some(exif) = exif.clone() {
                encoder
                    .set_exif_metadata(exif)
                    .map_err(|error| image_error(destination, error))?;
            }
            if sixteen_bit {
                encoder
                    .write_image(
                        &image.rgb16_bytes(),
                        image.width,
                        image.height,
                        image::ExtendedColorType::Rgb16,
                    )
                    .map_err(|error| image_error(destination, error))?;
            } else {
                encoder
                    .write_image(
                        &image.rgb8(),
                        image.width,
                        image.height,
                        image::ExtendedColorType::Rgb8,
                    )
                    .map_err(|error| image_error(destination, error))?;
            }
        }
        ExportFormat::Tiff {
            bit_depth,
            compressed,
        } => {
            let mut encoder = tiff::encoder::TiffEncoder::new(&mut writer)
                .map_err(|error| image_error(destination, error))?;
            if compressed {
                encoder = encoder.with_compression(tiff::encoder::Compression::Deflate(
                    tiff::encoder::DeflateLevel::default(),
                ));
            }
            match bit_depth {
                TiffDepth::Eight => {
                    let mut output = encoder
                        .new_image::<tiff::encoder::colortype::RGB8>(image.width, image.height)
                        .map_err(|error| image_error(destination, error))?;
                    write_tiff_metadata(output.encoder(), &icc, metadata, settings)?;
                    output
                        .write_data(&image.rgb8())
                        .map_err(|error| image_error(destination, error))?;
                }
                TiffDepth::Sixteen => {
                    let mut output = encoder
                        .new_image::<tiff::encoder::colortype::RGB16>(image.width, image.height)
                        .map_err(|error| image_error(destination, error))?;
                    write_tiff_metadata(output.encoder(), &icc, metadata, settings)?;
                    output
                        .write_data(&image.rgb16())
                        .map_err(|error| image_error(destination, error))?;
                }
            }
        }
        ExportFormat::Avif { quality } => {
            let encoder = ravif::Encoder::new()
                .with_speed(6)
                .with_quality(f32::from(quality.max(1)))
                // Ten-bit AV1 avoids encoder banding and is mandatory for
                // the Rec.2100 PQ path. Inputs are promoted losslessly.
                .with_bit_depth(ravif::BitDepth::Ten);
            let encoded = if hdr_pq {
                if let Some(rgba16) = &image.rgba16 {
                    let planes: Vec<[u16; 3]> = rgba16
                        .chunks_exact(4)
                        .map(|pixel| rec2020_pq_to_yuv10(pixel[0], pixel[1], pixel[2]))
                        .collect();
                    encoder
                        .encode_raw_planes_10_bit(
                            image.width as usize,
                            image.height as usize,
                            planes,
                            None::<[u16; 0]>,
                            ravif::PixelRange::Full,
                            ravif::MatrixCoefficients::BT2020NCL,
                        )
                        .map_err(|error| image_error(destination, error))?
                } else {
                    encode_avif_rgb8(&encoder, image, destination)?
                }
            } else {
                encode_avif_rgb8(&encoder, image, destination)?
            };
            let color_start = encoded
                .avif_file
                .len()
                .checked_sub(encoded.color_byte_size)
                .ok_or_else(|| image_error(destination, "invalid AVIF payload size"))?;
            let color_payload = &encoded.avif_file[color_start..];
            let mut muxer = avif_serialize::Aviffy::new();
            use avif_serialize::constants::{
                ColorPrimaries, MatrixCoefficients, TransferCharacteristics,
            };
            if hdr_pq {
                muxer
                    .set_color_primaries(ColorPrimaries::Bt2020)
                    .set_transfer_characteristics(TransferCharacteristics::Smpte2084)
                    .set_matrix_coefficients(MatrixCoefficients::Bt2020Ncl);
            } else {
                // CICP has no code point for Adobe RGB; leave it unspecified
                // so viewers fall back to the embedded ICC profile.
                let (primaries, transfer) = if settings.output_profile.is_some() {
                    (
                        ColorPrimaries::Unspecified,
                        TransferCharacteristics::Unspecified,
                    )
                } else {
                    match settings.color_space {
                        ExportColorSpace::Srgb => {
                            (ColorPrimaries::Bt709, TransferCharacteristics::Srgb)
                        }
                        ExportColorSpace::DisplayP3 => {
                            (ColorPrimaries::DisplayP3, TransferCharacteristics::Srgb)
                        }
                        ExportColorSpace::Rec2020 => {
                            (ColorPrimaries::Bt2020, TransferCharacteristics::Srgb)
                        }
                        ExportColorSpace::AdobeRgb => (
                            ColorPrimaries::Unspecified,
                            TransferCharacteristics::Unspecified,
                        ),
                    }
                };
                muxer
                    .set_color_primaries(primaries)
                    .set_transfer_characteristics(transfer)
                    .set_matrix_coefficients(MatrixCoefficients::Bt709);
            }
            if let Some(exif) = exif {
                muxer.set_exif(exif);
            }
            let avif_file = muxer.to_vec(color_payload, None, image.width, image.height, 10);
            let avif_file = crate::avif_metadata::attach(
                &avif_file,
                (!hdr_pq).then_some(icc.as_slice()),
                xmp.as_deref(),
            )
            .map_err(|error| image_error(destination, error))?;
            writer
                .write_all(&avif_file)
                .map_err(|error| image_error(destination, error))?;
        }
        // Ultra HDR needs both renditions, which this single-image entry
        // point cannot produce — callers go through `encode_ultra_hdr`.
        ExportFormat::UltraHdr { .. } => {
            return Err(ExportError::Encode {
                path: destination.to_path_buf(),
                reason: "Ultra HDR export requires the SDR and HDR renditions".into(),
            })
        }
        ExportFormat::WebP { lossless } => {
            if !lossless {
                let pixels = image.rgb8();
                let encoded = webp::Encoder::from_rgb(&pixels, image.width, image.height)
                    .encode_simple(false, settings.webp_quality.clamp(1, 100) as f32)
                    .map_err(|e| image_error(destination, format!("WebP: {e:?}")))?;
                let encoded =
                    webp_metadata(&encoded, image.width, image.height, &icc, exif.as_deref())
                        .map_err(|e| image_error(destination, e))?;
                writer
                    .write_all(&encoded)
                    .map_err(|e| image_error(destination, e))?;
            } else {
                let mut encoder = image::codecs::webp::WebPEncoder::new_lossless(&mut writer);
                encoder
                    .set_icc_profile(icc.clone())
                    .map_err(|error| image_error(destination, error))?;
                if let Some(exif) = exif.clone() {
                    encoder
                        .set_exif_metadata(exif)
                        .map_err(|error| image_error(destination, error))?;
                }
                encoder
                    .write_image(
                        &image.rgb8(),
                        image.width,
                        image.height,
                        image::ExtendedColorType::Rgb8,
                    )
                    .map_err(|error| image_error(destination, error))?;
            }
        }
        ExportFormat::JpegXl { quality, lossless } => {
            let distance = ((100u8.saturating_sub(quality.max(1))) as f32 / 10.0).clamp(0.1, 9.9);
            let encoder = if lossless {
                gamut_jxl::JxlEncoder::lossless()
            } else {
                gamut_jxl::JxlEncoder::lossy(
                    gamut_jxl::Distance::new(distance)
                        .map_err(|error| image_error(destination, error))?,
                )
            }
            .with_effort(gamut_jxl::Effort::Squirrel)
            .with_container(gamut_jxl::Container::IsoBmff)
            .with_color(if hdr_pq {
                gamut_jxl::ColorSpec::Pq
            } else {
                gamut_jxl::ColorSpec::Icc(icc.clone())
            });
            let encoder = exif
                .as_deref()
                .map_or(encoder.clone(), |metadata| encoder.with_exif(metadata));
            let encoder = xmp
                .as_deref()
                .map_or(encoder.clone(), |metadata| encoder.with_xmp(metadata));
            let pixels = image.rgb16();
            let image = ImageRef::<Rgb16>::new(
                &pixels,
                Dimensions {
                    width: image.width,
                    height: image.height,
                },
            )
            .map_err(|error| image_error(destination, error))?;
            let encoded = encoder
                .encode_to_vec(image)
                .map_err(|error| image_error(destination, error))?;
            writer
                .write_all(&encoded)
                .map_err(|error| image_error(destination, error))?;
        }
    }
    writer.flush().map_err(|source| ExportError::Io {
        path: tmp.clone(),
        source,
    })?;
    drop(writer);
    if let Some(xmp) = &xmp {
        match settings.format {
            ExportFormat::Png { .. } => insert_png_xmp(&tmp, xmp)?,
            ExportFormat::WebP { .. } => insert_webp_xmp(&tmp, xmp)?,
            _ => {}
        }
    }
    if hdr_pq {
        match settings.format {
            ExportFormat::Avif { .. } => patch_avif_cicp(&tmp)?,
            ExportFormat::Png { sixteen_bit: true } => insert_png_cicp(&tmp)?,
            _ => {}
        }
    }
    temp.persist(destination).map_err(|error| ExportError::Io {
        path: destination.to_path_buf(),
        source: error.error,
    })?;
    Ok(())
}

fn encode_avif_rgb8(
    encoder: &ravif::Encoder<'_>,
    image: &ExportImage,
    destination: &Path,
) -> Result<ravif::EncodedImage, ExportError> {
    let pixels: Vec<ravif::RGB8> = image
        .rgba8
        .chunks_exact(4)
        .map(|pixel| ravif::RGB8::new(pixel[0], pixel[1], pixel[2]))
        .collect();
    encoder
        .encode_rgb(ravif::Img::new(
            &pixels,
            image.width as usize,
            image.height as usize,
        ))
        .map_err(|error| image_error(destination, error))
}

fn webp_metadata(
    encoded: &[u8],
    width: u32,
    height: u32,
    icc: &[u8],
    exif: Option<&[u8]>,
) -> std::io::Result<Vec<u8>> {
    use std::io::{Error, ErrorKind};
    if encoded.len() < 12 || &encoded[..4] != b"RIFF" || &encoded[8..12] != b"WEBP" {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "Invalid WebP encoder output",
        ));
    }
    let chunk = |out: &mut Vec<u8>, tag: &[u8; 4], data: &[u8]| -> std::io::Result<()> {
        let len = u32::try_from(data.len())
            .map_err(|_| Error::new(ErrorKind::InvalidData, "WebP metadata too large"))?;
        out.extend_from_slice(tag);
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(data);
        if data.len() % 2 == 1 {
            out.push(0);
        }
        Ok(())
    };
    let mut out = b"RIFF\0\0\0\0WEBP".to_vec();
    let mut header = vec![0x20 | if exif.is_some() { 0x08 } else { 0 }, 0, 0, 0];
    header.extend_from_slice(&(width - 1).to_le_bytes()[..3]);
    header.extend_from_slice(&(height - 1).to_le_bytes()[..3]);
    chunk(&mut out, b"VP8X", &header)?;
    chunk(&mut out, b"ICCP", icc)?;
    // libwebp's opaque RGB encoder emits VP8 only; retain image chunks.
    let mut cursor = 12;
    while cursor + 8 <= encoded.len() {
        let len = u32::from_le_bytes(encoded[cursor + 4..cursor + 8].try_into().unwrap()) as usize;
        let end = cursor
            .checked_add(8 + len + len % 2)
            .filter(|end| *end <= encoded.len())
            .ok_or_else(|| Error::new(ErrorKind::InvalidData, "Truncated WebP chunk"))?;
        if &encoded[cursor..cursor + 4] != b"VP8X" {
            out.extend_from_slice(&encoded[cursor..end]);
        }
        cursor = end;
    }
    if let Some(exif) = exif {
        chunk(&mut out, b"EXIF", exif)?;
    }
    let len = u32::try_from(out.len() - 8)
        .map_err(|_| Error::new(ErrorKind::InvalidData, "WebP too large"))?;
    out[4..8].copy_from_slice(&len.to_le_bytes());
    Ok(out)
}

fn rec2020_pq_to_yuv10(r: u16, g: u16, b: u16) -> [u16; 3] {
    let scale = 1.0 / 65535.0;
    let (r, g, b) = (
        f32::from(r) * scale,
        f32::from(g) * scale,
        f32::from(b) * scale,
    );
    const KR: f32 = 0.2627;
    const KB: f32 = 0.0593;
    const KG: f32 = 1.0 - KR - KB;
    let y = KR * r + KG * g + KB * b;
    let cb = (b - y) / (2.0 * (1.0 - KB)) + 0.5;
    let cr = (r - y) / (2.0 * (1.0 - KR)) + 0.5;
    [y, cb, cr].map(|value| (value.clamp(0.0, 1.0) * 1023.0).round() as u16)
}

pub(crate) fn write_jpeg_segment(
    writer: &mut impl Write,
    marker: u8,
    data: &[u8],
) -> std::io::Result<()> {
    let length = data
        .len()
        .checked_add(2)
        .and_then(|n| u16::try_from(n).ok())
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "JPEG metadata exceeds the segment size limit",
            )
        })?;
    writer.write_all(&[0xff, marker])?;
    writer.write_all(&length.to_be_bytes())?;
    writer.write_all(data)
}

fn write_jpeg_with_metadata(
    writer: &mut impl Write,
    encoded: &[u8],
    icc: &[u8],
    exif: Option<&[u8]>,
    xmp: Option<&str>,
) -> std::io::Result<()> {
    if !encoded.starts_with(&[0xff, 0xd8]) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "JPEG encoder omitted SOI marker",
        ));
    }
    writer.write_all(&encoded[..2])?;
    if let Some(exif) = exif {
        let mut payload = b"Exif\0\0".to_vec();
        payload.extend_from_slice(exif);
        write_jpeg_segment(writer, 0xe1, &payload)?;
    }
    if let Some(xmp) = xmp {
        let mut payload = b"http://ns.adobe.com/xap/1.0/\0".to_vec();
        payload.extend_from_slice(xmp.as_bytes());
        write_jpeg_segment(writer, 0xe1, &payload)?;
    }
    const ICC_HEADER: usize = 14;
    const ICC_CHUNK: usize = u16::MAX as usize - 2 - ICC_HEADER;
    let count = icc.len().div_ceil(ICC_CHUNK).min(255);
    for (index, chunk) in icc.chunks(ICC_CHUNK).enumerate() {
        let mut payload = b"ICC_PROFILE\0".to_vec();
        payload.push((index + 1) as u8);
        payload.push(count as u8);
        payload.extend_from_slice(chunk);
        write_jpeg_segment(writer, 0xe2, &payload)?;
    }
    writer.write_all(&encoded[2..])
}

fn write_tiff_metadata<W: std::io::Write + std::io::Seek>(
    directory: &mut tiff::encoder::DirectoryEncoder<'_, W, tiff::encoder::TiffKindStandard>,
    icc: &[u8],
    metadata: Option<&ImageMeta>,
    settings: &ExportSettings,
) -> Result<(), ExportError> {
    use tiff::encoder::Rational;
    use tiff::tags::Tag;
    let map = |error: tiff::TiffError| image_error(Path::new("<tiff-metadata>"), error);
    directory.write_tag(Tag::Unknown(34675), icc).map_err(map)?;
    directory
        .write_tag(Tag::Software, "ShopPhoto")
        .map_err(map)?;
    if let Some(xmp) = crate::metadata::packet(settings, metadata) {
        directory
            .write_tag(Tag::Unknown(700), xmp.as_bytes())
            .map_err(map)?;
    }
    if settings.copy_descriptive_metadata {
        for (tag, value) in [
            (Tag::ImageDescription, &settings.photo_metadata.caption),
            (Tag::Artist, &settings.photo_metadata.creator),
            (Tag::Copyright, &settings.photo_metadata.copyright),
        ] {
            if !value.is_empty() && value.is_ascii() {
                directory.write_tag(tag, value.as_str()).map_err(map)?;
            }
        }
    }
    let Some(meta) = metadata.filter(|_| settings.copy_metadata) else {
        return Ok(());
    };
    if !meta.camera_make.is_empty() {
        directory
            .write_tag(Tag::Make, &*meta.camera_make)
            .map_err(map)?;
    }
    if !meta.camera_model.is_empty() {
        directory
            .write_tag(Tag::Model, &*meta.camera_model)
            .map_err(map)?;
    }
    if let Some(captured) = meta.captured_at.as_deref() {
        let captured = exif_datetime(captured);
        directory
            .write_tag(Tag::DateTime, &*captured)
            .map_err(map)?;
        directory
            .write_tag(Tag::Unknown(36867), &*captured)
            .map_err(map)?;
    }
    if let Some(iso) = meta.iso {
        directory
            .write_tag(Tag::Unknown(34855), iso.min(u32::from(u16::MAX)) as u16)
            .map_err(map)?;
    }
    for (tag, value) in [
        (33434, meta.shutter_s),
        (33437, meta.aperture_f),
        (37386, meta.focal_length_mm),
    ] {
        if let Some(value) = value.filter(|value| value.is_finite() && *value > 0.0) {
            let denominator = 1_000_000;
            directory
                .write_tag(
                    Tag::Unknown(tag),
                    Rational {
                        n: (value as f64 * f64::from(denominator))
                            .round()
                            .clamp(1.0, f64::from(u32::MAX)) as u32,
                        d: denominator,
                    },
                )
                .map_err(map)?;
        }
    }
    if let Some(lens) = meta.lens_model.as_deref() {
        directory
            .write_tag(Tag::Unknown(42036), lens)
            .map_err(map)?;
    }
    Ok(())
}

#[derive(Clone)]
struct ExifEntry {
    tag: u16,
    kind: u16,
    count: u32,
    data: Vec<u8>,
}

fn ascii_entry(tag: u16, value: &str) -> Option<ExifEntry> {
    if value.is_empty() || !value.is_ascii() || value.contains('\0') {
        return None;
    }
    let mut data = value.as_bytes().to_vec();
    data.push(0);
    Some(ExifEntry {
        tag,
        kind: 2,
        count: data.len() as u32,
        data,
    })
}

fn rational_entry(tag: u16, value: f32) -> Option<ExifEntry> {
    if !value.is_finite() || value <= 0.0 {
        return None;
    }
    let denominator = 1_000_000u32;
    let numerator = (value as f64 * f64::from(denominator))
        .round()
        .clamp(1.0, f64::from(u32::MAX)) as u32;
    let mut data = numerator.to_le_bytes().to_vec();
    data.extend_from_slice(&denominator.to_le_bytes());
    Some(ExifEntry {
        tag,
        kind: 5,
        count: 1,
        data,
    })
}

fn ifd_len(entries: &[ExifEntry]) -> usize {
    2 + entries.len() * 12
        + 4
        + entries
            .iter()
            .filter(|entry| entry.data.len() > 4)
            .map(|entry| (entry.data.len() + 1) & !1)
            .sum::<usize>()
}

fn append_ifd(output: &mut Vec<u8>, entries: &[ExifEntry], offset: usize) {
    debug_assert_eq!(output.len(), offset);
    output.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    let mut payload_offset = offset + 2 + entries.len() * 12 + 4;
    let mut payload = Vec::new();
    for entry in entries {
        output.extend_from_slice(&entry.tag.to_le_bytes());
        output.extend_from_slice(&entry.kind.to_le_bytes());
        output.extend_from_slice(&entry.count.to_le_bytes());
        if entry.data.len() <= 4 {
            output.extend_from_slice(&entry.data);
            output.resize(output.len() + 4 - entry.data.len(), 0);
        } else {
            output.extend_from_slice(&(payload_offset as u32).to_le_bytes());
            payload.extend_from_slice(&entry.data);
            if payload.len() % 2 != 0 {
                payload.push(0);
            }
            payload_offset = offset + 2 + entries.len() * 12 + 4 + payload.len();
        }
    }
    output.extend_from_slice(&0u32.to_le_bytes());
    output.extend_from_slice(&payload);
}

fn exif_datetime(value: &str) -> String {
    let head = value.get(..19).unwrap_or(value);
    head.replace('-', ":").replace('T', " ")
}

/// Build a little-endian TIFF/EXIF payload (without the JPEG `Exif\0\0`
/// prefix, which container encoders add themselves).
fn output_exif(meta: Option<&ImageMeta>, settings: &ExportSettings) -> Option<Vec<u8>> {
    let capture = meta
        .filter(|_| settings.copy_metadata)
        .cloned()
        .unwrap_or_default();
    let xmp = crate::metadata::packet(settings, meta);
    if meta.filter(|_| settings.copy_metadata).is_none() && xmp.is_none() {
        return None;
    }
    Some(build_exif(&capture, settings, xmp.as_deref()))
}

fn gps_entries([lat, lon]: [f64; 2]) -> Vec<ExifEntry> {
    let coordinate = |tag, value: f64| {
        let absolute = value.abs();
        let degrees = absolute.floor() as u32;
        let total_minutes = (absolute - f64::from(degrees)) * 60.0;
        let minutes = total_minutes.floor() as u32;
        let seconds = ((total_minutes - f64::from(minutes)) * 60_000_000.0).round() as u32;
        let mut data = Vec::with_capacity(24);
        for (n, d) in [(degrees, 1u32), (minutes, 1), (seconds, 1_000_000)] {
            data.extend_from_slice(&n.to_le_bytes());
            data.extend_from_slice(&d.to_le_bytes());
        }
        ExifEntry {
            tag,
            kind: 5,
            count: 3,
            data,
        }
    };
    vec![
        ExifEntry {
            tag: 0,
            kind: 1,
            count: 4,
            data: vec![2, 3, 0, 0],
        },
        ExifEntry {
            tag: 1,
            kind: 2,
            count: 2,
            data: vec![if lat < 0.0 { b'S' } else { b'N' }, 0],
        },
        coordinate(2, lat),
        ExifEntry {
            tag: 3,
            kind: 2,
            count: 2,
            data: vec![if lon < 0.0 { b'W' } else { b'E' }, 0],
        },
        coordinate(4, lon),
    ]
}

fn build_exif(meta: &ImageMeta, settings: &ExportSettings, xmp: Option<&str>) -> Vec<u8> {
    let mut exif = Vec::new();
    if let Some(value) = meta.shutter_s.and_then(|v| rational_entry(0x829A, v)) {
        exif.push(value);
    }
    if let Some(value) = meta.aperture_f.and_then(|v| rational_entry(0x829D, v)) {
        exif.push(value);
    }
    if let Some(iso) = meta.iso {
        exif.push(ExifEntry {
            tag: 0x8827,
            kind: 3,
            count: 1,
            data: (iso.min(u32::from(u16::MAX)) as u16).to_le_bytes().to_vec(),
        });
    }
    if let Some(captured) = meta.captured_at.as_deref() {
        if let Some(value) = ascii_entry(0x9003, &exif_datetime(captured)) {
            exif.push(value);
        }
    }
    if let Some(value) = meta.focal_length_mm.and_then(|v| rational_entry(0x920A, v)) {
        exif.push(value);
    }
    if let Some(lens) = meta.lens_model.as_deref() {
        if let Some(value) = ascii_entry(0xA434, lens) {
            exif.push(value);
        }
    }
    exif.sort_by_key(|entry| entry.tag);

    let mut primary: Vec<_> = [
        ascii_entry(0x010F, &meta.camera_make),
        ascii_entry(0x0110, &meta.camera_model),
        ascii_entry(0x0131, "ShopPhoto"),
        meta.captured_at
            .as_deref()
            .and_then(|value| ascii_entry(0x0132, &exif_datetime(value))),
    ]
    .into_iter()
    .flatten()
    .collect();
    if settings.copy_descriptive_metadata {
        primary.extend(
            [
                ascii_entry(0x010e, &settings.photo_metadata.caption),
                ascii_entry(0x013b, &settings.photo_metadata.creator),
                ascii_entry(0x8298, &settings.photo_metadata.copyright),
            ]
            .into_iter()
            .flatten(),
        );
    }
    // TIFF tag 700 also retains XMP in formats whose encoder only exposes
    // an EXIF item. Native XMP packets are additionally written where supported.
    if let Some(xmp) = xmp {
        primary.push(ExifEntry {
            tag: 700,
            kind: 1,
            count: xmp.len() as u32,
            data: xmp.as_bytes().to_vec(),
        });
    }
    let gps = crate::metadata::gps(settings, Some(meta)).map(gps_entries);
    primary.push(ExifEntry {
        tag: 0x8769,
        kind: 4,
        count: 1,
        data: vec![0; 4],
    });
    if gps.is_some() {
        primary.push(ExifEntry {
            tag: 0x8825,
            kind: 4,
            count: 1,
            data: vec![0; 4],
        });
    }
    let exif_offset = 8 + ifd_len(&primary);
    let gps_offset = exif_offset + ifd_len(&exif);
    for entry in &mut primary {
        if entry.tag == 0x8769 {
            entry.data = (exif_offset as u32).to_le_bytes().to_vec();
        }
        if entry.tag == 0x8825 {
            entry.data = (gps_offset as u32).to_le_bytes().to_vec();
        }
    }
    primary.sort_by_key(|entry| entry.tag);

    let mut output = b"II*\0\x08\0\0\0".to_vec();
    append_ifd(&mut output, &primary, 8);
    debug_assert_eq!(output.len(), exif_offset);
    append_ifd(&mut output, &exif, exif_offset);
    if let Some(gps) = gps {
        append_ifd(&mut output, &gps, gps_offset);
    }
    output
}

fn insert_png_xmp(path: &Path, packet: &str) -> Result<(), ExportError> {
    let bytes = std::fs::read(path).map_err(|e| image_error(path, e))?;
    if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") || bytes.len() < 33 {
        return Err(image_error(path, "PNG encoder returned an invalid header"));
    }
    let mut payload = b"XML:com.adobe.xmp\0\0\0\0\0".to_vec();
    payload.extend_from_slice(packet.as_bytes());
    let mut chunk = Vec::with_capacity(payload.len() + 12);
    chunk.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    chunk.extend_from_slice(b"iTXt");
    chunk.extend_from_slice(&payload);
    let crc = crc32fast::hash(&chunk[4..]);
    chunk.extend_from_slice(&crc.to_be_bytes());
    let mut file = std::fs::File::create(path).map_err(|e| image_error(path, e))?;
    file.write_all(&bytes[..33])
        .and_then(|_| file.write_all(&chunk))
        .and_then(|_| file.write_all(&bytes[33..]))
        .map_err(|e| image_error(path, e))
}

fn insert_webp_xmp(path: &Path, packet: &str) -> Result<(), ExportError> {
    let mut bytes = std::fs::read(path).map_err(|e| image_error(path, e))?;
    if !bytes.starts_with(b"RIFF") || bytes.get(8..16) != Some(b"WEBPVP8X") || bytes.len() < 30 {
        return Err(image_error(
            path,
            "WebP encoder omitted extended metadata header",
        ));
    }
    bytes[20] |= 0x04; // VP8X XMP-present bit.
    bytes.extend_from_slice(b"XMP ");
    bytes.extend_from_slice(&(packet.len() as u32).to_le_bytes());
    bytes.extend_from_slice(packet.as_bytes());
    if !packet.len().is_multiple_of(2) {
        bytes.push(0);
    }
    let length = u32::try_from(bytes.len() - 8).map_err(|e| image_error(path, e))?;
    bytes[4..8].copy_from_slice(&length.to_le_bytes());
    std::fs::write(path, bytes).map_err(|e| image_error(path, e))
}

fn patch_avif_cicp(path: &Path) -> Result<(), ExportError> {
    let mut bytes = std::fs::read(path).map_err(|source| ExportError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let Some(marker) = bytes.windows(4).position(|window| window == b"nclx") else {
        return Err(image_error(
            path,
            "AVIF encoder omitted nclx color metadata",
        ));
    };
    let start = marker + 4;
    if bytes.len() < start + 7 {
        return Err(image_error(path, "truncated AVIF nclx metadata"));
    }
    bytes[start..start + 2].copy_from_slice(&9u16.to_be_bytes());
    bytes[start + 2..start + 4].copy_from_slice(&16u16.to_be_bytes());
    bytes[start + 4..start + 6].copy_from_slice(&9u16.to_be_bytes());
    bytes[start + 6] |= 0x80;
    std::fs::write(path, bytes).map_err(|source| ExportError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn insert_png_cicp(path: &Path) -> Result<(), ExportError> {
    let bytes = std::fs::read(path).map_err(|source| ExportError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if bytes.len() < 33 || &bytes[..8] != b"\x89PNG\r\n\x1a\n" {
        return Err(image_error(path, "invalid PNG stream"));
    }
    let data = [9, 16, 0, 1];
    let mut chunk = Vec::with_capacity(16);
    chunk.extend_from_slice(&4u32.to_be_bytes());
    chunk.extend_from_slice(b"cICP");
    chunk.extend_from_slice(&data);
    let mut crc = crc32fast::Hasher::new();
    crc.update(b"cICP");
    crc.update(&data);
    chunk.extend_from_slice(&crc.finalize().to_be_bytes());
    let insert_at = 8 + 4 + 4 + 13 + 4;
    let mut tagged = Vec::with_capacity(bytes.len() + chunk.len());
    tagged.extend_from_slice(&bytes[..insert_at]);
    tagged.extend_from_slice(&chunk);
    tagged.extend_from_slice(&bytes[insert_at..]);
    std::fs::write(path, tagged).map_err(|source| ExportError::Io {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delivery_size_limits_include_metadata_and_preserve_destination_on_failure() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("delivery.jpg");
        let mut rgba = Vec::new();
        let mut seed = 17u32;
        for _ in 0..256 * 128 {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            rgba.extend_from_slice(&[
                (seed >> 24) as u8,
                (seed >> 16) as u8,
                (seed >> 8) as u8,
                255,
            ]);
        }
        let image = ExportImage::new(256, 128, rgba).unwrap();
        let mut settings = ExportSettings {
            format: ExportFormat::Jpeg {
                quality: 95,
                chroma_444: true,
            },
            ..Default::default()
        };
        settings.photo_metadata.caption = "Delivery test".repeat(15);
        encode_file(&image, &settings, &path).unwrap();
        let full = std::fs::metadata(&path).unwrap().len();
        let max_kib = (full / 1024 / 2) as u32;
        settings.file_size_limit = Some(crate::FileSizeLimit {
            max_kib,
            minimum_quality: 10,
        });
        encode_file(&image, &settings, &path).unwrap();
        assert!(std::fs::metadata(&path).unwrap().len() <= u64::from(max_kib) * 1024);
        assert_eq!(
            image::open(&path).unwrap().to_rgb8().dimensions(),
            (256, 128)
        );
        let good = std::fs::read(&path).unwrap();
        settings.file_size_limit = Some(crate::FileSizeLimit {
            max_kib: 1,
            minimum_quality: 90,
        });
        assert!(encode_file(&image, &settings, &path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), good);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn output_profile_is_embedded_and_rejects_invalid_profile_without_replacing_file() {
        use image::ImageDecoder;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("prophoto.png");
        let mut settings = ExportSettings {
            format: ExportFormat::Png { sixteen_bit: true },
            output_profile: Some(crate::OutputProfile::ProPhotoRgb),
            ..Default::default()
        };
        encode_file(&fixture(), &settings, &path).unwrap();
        let mut decoder = image::ImageReader::open(&path)
            .unwrap()
            .into_decoder()
            .unwrap();
        assert_eq!(
            decoder.icc_profile().unwrap().unwrap(),
            settings
                .output_profile
                .as_ref()
                .unwrap()
                .icc_bytes()
                .unwrap()
        );
        let good = std::fs::read(&path).unwrap();
        settings.output_profile =
            Some(crate::OutputProfile::Custom(dir.path().join("missing.icc")));
        assert!(encode_file(&fixture(), &settings, &path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), good);
    }
    use image::ImageDecoder;

    #[test]
    fn invalid_exports_preserve_existing_destination_and_leave_no_temps() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("existing.jpg");
        std::fs::write(&path, b"original").unwrap();
        let wide = ExportImage::new(65536, 1, vec![255; 65536 * 4]).unwrap();
        let settings = ExportSettings::default();
        assert!(encode_file(&wide, &settings, &path).is_err());
        let mut broken = fixture();
        broken.rgba16 = Some(vec![0]);
        assert!(encode_file(&broken, &settings, &path).is_err());
        let ultra = ExportSettings {
            format: ExportFormat::UltraHdr { quality: 90 },
            ..settings
        };
        assert!(encode_ultra_hdr(&wide, &wide, &ultra, &path, None).is_err());
        // This fails inside the encoder after the temporary file is created.
        assert!(encode_file(&fixture(), &ultra, &path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"original");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn temporary_exports_are_unique_and_clean_up() {
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("photo.jpg");
        let first = temporary_output(&destination).unwrap();
        let second = temporary_output(&destination).unwrap();
        assert_ne!(first.path(), second.path());
        drop(first);
        drop(second);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    fn fixture() -> ExportImage {
        ExportImage::new(
            2,
            2,
            [255, 0, 0, 255].into_iter().cycle().take(16).collect(),
        )
        .expect("fixture")
    }

    #[test]
    fn jpeg_png_tiff_round_trip_dimensions_and_icc() {
        let dir = tempfile::tempdir().expect("tempdir");
        let formats = [
            ExportFormat::Jpeg {
                quality: 90,
                chroma_444: false,
            },
            ExportFormat::Png { sixteen_bit: true },
            ExportFormat::Tiff {
                bit_depth: TiffDepth::Sixteen,
                compressed: true,
            },
        ];
        for format in formats {
            let settings = ExportSettings {
                format,
                ..Default::default()
            };
            let path = dir.path().join(format!("test.{}", format.extension()));
            encode_file(&fixture(), &settings, &path).expect("encode");
            let reader = image::ImageReader::open(&path)
                .expect("open")
                .with_guessed_format()
                .expect("guess");
            let mut decoder = reader.into_decoder().expect("decode");
            assert_eq!(decoder.dimensions(), (2, 2));
            let decoder_icc = decoder.icc_profile().expect("icc").is_some();
            let raw_icc = std::fs::read(&path)
                .expect("read")
                .windows(4)
                .any(|bytes| bytes == b"acsp");
            assert!(decoder_icc || raw_icc, "missing ICC for {format:?}");
        }
    }

    #[test]
    fn lossless_webp_round_trips_with_icc() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("test.webp");
        let settings = ExportSettings {
            format: ExportFormat::WebP { lossless: true },
            ..Default::default()
        };
        encode_file(&fixture(), &settings, &path).expect("encode");
        let decoded = image::open(&path).expect("decode").into_rgba8();
        assert_eq!(decoded.dimensions(), (2, 2));
        assert_eq!(&decoded.as_raw()[..4], &[255, 0, 0, 255], "lossless red");
        let bytes = std::fs::read(&path).expect("read");
        assert!(bytes.windows(4).any(|w| w == b"ICCP"), "missing ICCP chunk");

        let lossy = ExportSettings {
            format: ExportFormat::WebP { lossless: false },
            ..Default::default()
        };
        let path = dir.path().join("lossy.webp");
        encode_file(&fixture(), &lossy, &path).expect("lossy WebP");
        assert_eq!(image::open(&path).unwrap().to_rgb8().dimensions(), (2, 2));
        let bytes = std::fs::read(path).unwrap();
        assert!(bytes.windows(4).any(|w| w == b"VP8 "));
        assert!(bytes.windows(4).any(|w| w == b"ICCP"));
    }

    #[test]
    fn adobe_rgb_export_embeds_the_adobe_profile() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("adobe.jpg");
        let settings = ExportSettings {
            format: ExportFormat::Jpeg {
                quality: 90,
                chroma_444: true,
            },
            color_space: ExportColorSpace::AdobeRgb,
            ..Default::default()
        };
        encode_file(&fixture(), &settings, &path).expect("encode");
        let bytes = std::fs::read(&path).expect("read");
        // moxcms writes the profile description as UTF-16BE in an mluc tag.
        let description: Vec<u8> = "Adobe RGB 1998"
            .encode_utf16()
            .flat_map(u16::to_be_bytes)
            .collect();
        assert!(
            bytes
                .windows(description.len())
                .any(|window| window == description.as_slice()),
            "JPEG does not carry the Adobe RGB profile"
        );
        let profile = icc_profile(ExportColorSpace::AdobeRgb).expect("icc");
        let parsed = moxcms::ColorProfile::new_from_slice(&profile).expect("parse");
        // D50-adapted Adobe RGB red colorant: X ≈ 0.6097.
        assert!(
            (parsed.red_colorant.x - 0.6097).abs() < 0.01,
            "{:?}",
            parsed.red_colorant
        );
    }

    #[test]
    fn rec2020_icc_matches_the_renderers_srgb_transfer_curve() {
        let bytes = icc_profile(ExportColorSpace::Rec2020).unwrap();
        let profile = lcms2::Profile::new_icc(&bytes).unwrap();
        let transform: lcms2::Transform<[f32; 3], [f32; 3]> = lcms2::Transform::new_flags(
            &profile,
            lcms2::PixelFormat::RGB_FLT,
            &lcms2::Profile::new_xyz(),
            lcms2::PixelFormat::XYZ_FLT,
            lcms2::Intent::RelativeColorimetric,
            lcms2::Flags::NO_OPTIMIZE,
        )
        .unwrap();
        let input = [[0.02; 3], [0.18; 3], [0.5; 3], [1.0; 3]];
        let mut xyz = [[0.0; 3]; 4];
        transform.transform_pixels(&input, &mut xyz);
        for (encoded, converted) in input.iter().zip(xyz) {
            let expected = sp_core::color::srgb_eotf(encoded[0]);
            assert!(
                (converted[1] - expected).abs() < 0.0001,
                "{converted:?} vs {expected}"
            );
        }
    }

    #[test]
    fn hdr_png_has_bt2020_pq_cicp() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("hdr.png");
        let settings = ExportSettings {
            format: ExportFormat::Png { sixteen_bit: true },
            color_space: ExportColorSpace::Rec2020,
            ..Default::default()
        };
        encode_file_hdr(&fixture(), &settings, &path, true).expect("encode");
        let bytes = std::fs::read(path).expect("read");
        assert!(bytes
            .windows(8)
            .any(|window| window == b"cICP\x09\x10\x00\x01"));
    }

    #[test]
    fn hdr_avif_has_bt2020_pq_nclx() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("hdr.avif");
        let settings = ExportSettings {
            format: ExportFormat::Avif { quality: 80 },
            color_space: ExportColorSpace::Rec2020,
            ..Default::default()
        };
        encode_file_hdr(&fixture(), &settings, &path, true).expect("encode");
        let bytes = std::fs::read(path).expect("read");
        assert!(bytes
            .windows(10)
            .any(|window| { window == [b'n', b'c', b'l', b'x', 0, 9, 0, 16, 0, 9] }));
        let av1c = bytes
            .windows(4)
            .position(|window| window == b"av1C")
            .expect("av1C property");
        assert_ne!(bytes[av1c + 6] & 0x40, 0, "AVIF is not high-bit-depth");
    }

    #[test]
    fn jpeg_xl_round_trips_16_bit_and_embeds_sdr_icc() {
        use gamut_core::{DecodeImage, ImageBuf};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lossless.jxl");
        let source = ExportImage::new_16(
            2,
            2,
            vec![
                123, 4_567, 65_000, 65_535, 10_000, 20_000, 30_000, 65_535, 40_000, 50_000, 60_000,
                65_535, 1, 2, 3, 65_535,
            ],
        )
        .unwrap();
        let settings = ExportSettings {
            format: ExportFormat::JpegXl {
                quality: 100,
                lossless: true,
            },
            color_space: ExportColorSpace::DisplayP3,
            ..Default::default()
        };
        encode_file(&source, &settings, &path).unwrap();
        let bytes = std::fs::read(path).unwrap();
        assert_eq!(&bytes[..12], b"\0\0\0\x0cJXL \r\n\x87\n");
        let decoder = gamut_jxl::JxlDecoder::new();
        let info = decoder.info(&bytes).unwrap();
        assert_eq!((info.dimensions.width, info.dimensions.height), (2, 2));
        assert_eq!(info.bits_per_sample, 16);
        assert!(decoder.embedded_icc_profile(&bytes).unwrap().is_some());
        let decoded: ImageBuf<Rgb16> = decoder.decode_image(&bytes).unwrap();
        assert_eq!(decoded.as_samples(), source.rgb16());
    }

    #[test]
    fn hdr_jpeg_xl_uses_pq_without_an_sdr_icc() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hdr.jxl");
        let settings = ExportSettings {
            format: ExportFormat::JpegXl {
                quality: 90,
                lossless: false,
            },
            color_space: ExportColorSpace::Rec2020,
            ..Default::default()
        };
        encode_file_hdr(&fixture(), &settings, &path, true).unwrap();
        let bytes = std::fs::read(path).unwrap();
        let decoder = gamut_jxl::JxlDecoder::new();
        assert_eq!(decoder.info(&bytes).unwrap().bits_per_sample, 16);
        assert!(decoder.embedded_icc_profile(&bytes).unwrap().is_none());
    }

    #[test]
    fn jpeg_embeds_capture_exif() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("meta.jpg");
        let settings = ExportSettings::default();
        let meta = ImageMeta {
            camera_make: "Hasselblad".into(),
            camera_model: "X2D 100C".into(),
            lens_model: Some("XCD 55V".into()),
            iso: Some(400),
            shutter_s: Some(1.0 / 250.0),
            aperture_f: Some(4.0),
            focal_length_mm: Some(55.0),
            captured_at: Some("2026-07-20T12:34:56Z".into()),
            gps: None,
        };
        encode_file_with_metadata(&fixture(), &settings, &path, false, Some(&meta))
            .expect("encode");
        let reader = image::ImageReader::open(path)
            .expect("open")
            .with_guessed_format()
            .expect("guess");
        let mut decoder = reader.into_decoder().expect("decoder");
        let exif = decoder
            .exif_metadata()
            .expect("read exif")
            .expect("embedded exif");
        assert!(exif.windows(10).any(|value| value == b"Hasselblad"));
        assert!(exif.windows(8).any(|value| value == b"X2D 100C"));
        assert!(exif.windows(7).any(|value| value == b"XCD 55V"));
    }

    #[test]
    fn sixteen_bit_tiff_round_trips_within_one_lsb() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("precision.tif");
        let source = ExportImage::new_16(
            2,
            1,
            vec![123, 12_345, 65_000, 65_535, 32_001, 40_003, 50_007, 65_535],
        )
        .expect("fixture");
        let settings = ExportSettings {
            format: ExportFormat::Tiff {
                bit_depth: TiffDepth::Sixteen,
                compressed: true,
            },
            ..Default::default()
        };
        encode_file(&source, &settings, &path).expect("encode");
        let decoded = image::open(path).expect("decode").to_rgb16().into_raw();
        let expected = source.rgb16();
        assert_eq!(decoded.len(), expected.len());
        assert!(decoded
            .iter()
            .zip(expected)
            .all(|(actual, expected)| actual.abs_diff(expected) <= 1));
    }
}
