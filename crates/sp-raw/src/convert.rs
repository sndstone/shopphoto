//! rawler → sp-core type conversion. All rawler types stay inside this
//! module; the public surface of `sp-raw` exposes only `sp-core` types.

use crate::{DecodeOutput, Preview, PreviewOutput};
use rawler::decoders::{Decoder, Orientation as RawlerOrientation, RawDecodeParams, RawMetadata};
use rawler::imgop::xyz::Illuminant as RawlerIlluminant;
use rawler::rawsource::RawSource;
use rawler::{RawImage as RawlerImage, RawImageData};
use sp_core::color::{mat3_inverse, Mat3};
use sp_core::raw::{ActiveArea, CfaPattern, Orientation};
use sp_core::{ImageMeta, RawImage, SpError};
use std::path::Path;

/// Optional camera data must never discard successfully decoded sensor pixels.
/// Catch each preview source separately so a corrupt JPEG can fall back to a
/// different embedded thumbnail instead of terminating the entire decode.
fn optional_decode<T>(
    path: &str,
    component: &str,
    operation: impl FnOnce() -> rawler::Result<T>,
) -> Option<T> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(operation)) {
        Ok(Ok(value)) => Some(value),
        Ok(Err(error)) => {
            tracing::debug!(path, component, %error, "optional RAW data is unreadable");
            None
        }
        Err(_) => {
            tracing::warn!(
                path,
                component,
                "optional RAW decoder panicked on malformed input"
            );
            None
        }
    }
}

fn read_metadata(
    path: &str,
    decoder: &dyn Decoder,
    source: &RawSource,
    params: &RawDecodeParams,
) -> Option<(ImageMeta, Option<u16>)> {
    optional_decode(path, "metadata", || {
        decoder
            .raw_metadata(source, params)
            .map(|metadata| (convert_meta(&metadata), metadata.exif.orientation))
    })
}

fn read_preview(
    path: &str,
    decoder: &dyn Decoder,
    source: &RawSource,
    params: &RawDecodeParams,
    orientation: Option<u16>,
    thumbnail_only: bool,
) -> Option<Preview> {
    let prepare = |image: image::DynamicImage| {
        let image = if thumbnail_only {
            image.thumbnail(512, 512)
        } else {
            image
        };
        let rgba = image.to_rgba8();
        rotate_preview(
            Preview {
                width: rgba.width(),
                height: rgba.height(),
                rgba8: rgba.into_raw(),
            },
            orientation,
        )
    };
    optional_decode(path, "preview", || {
        decoder
            .preview_image(source, params)
            .map(|image| image.map(prepare))
    })
    .flatten()
    .or_else(|| {
        optional_decode(path, "thumbnail", || {
            decoder
                .thumbnail_image(source, params)
                .map(|image| image.map(prepare))
        })
        .flatten()
    })
    // Hasselblad's embedded JPEG is exposed only through full_image.
    .or_else(|| {
        optional_decode(path, "full image", || {
            decoder
                .full_image(source, params)
                .map(|image| image.map(prepare))
        })
        .flatten()
    })
}

pub(crate) fn decode_preview_impl(path: &Path) -> Result<PreviewOutput, SpError> {
    let path_str = path.display().to_string();
    let bytes = std::fs::read(path).map_err(|e| SpError::io(&path_str, e))?;
    crate::preflight::check(&path_str, &bytes)?;
    drop(bytes);
    let source = RawSource::new_lazy(path).map_err(|e| SpError::io(&path_str, e))?;
    let decoder = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rawler::get_decoder(&source)
    }))
    .map_err(|_| SpError::decode(&path_str, "decoder panicked on malformed input"))?
    .map_err(|e| SpError::decode(&path_str, e.to_string()))?;
    let params = RawDecodeParams::default();

    let (meta, exif_orientation) =
        read_metadata(&path_str, decoder.as_ref(), &source, &params).unwrap_or_default();
    // Bound CPU, memory and UI upload costs before crossing the worker channel.
    let preview = read_preview(
        &path_str,
        decoder.as_ref(),
        &source,
        &params,
        exif_orientation,
        true,
    );
    Ok(PreviewOutput { preview, meta })
}

pub(crate) fn decode_impl(path: &Path) -> Result<DecodeOutput, SpError> {
    let path_str = path.display().to_string();
    // Structural sanity first: a truncated file that still advertises its
    // original strip sizes would otherwise drive a multi-hundred-megabyte
    // allocation inside the decoder (found by the raw_decode fuzz target).
    let bytes = std::fs::read(path).map_err(|e| SpError::io(&path_str, e))?;
    crate::preflight::check(&path_str, &bytes)?;
    drop(bytes);
    let source = RawSource::new(path).map_err(|e| SpError::io(&path_str, e))?;
    // sp-raw's contract is that malformed input never panics (lib.rs), but
    // the panic paths are inside rawler — `get_decoder` reaches a `todo!()`
    // on a file whose Make tag is not an ASCII value, and other tag
    // accessors panic on type mismatches. The vendored copy fixes the ones
    // we have hit; this catches the ones we have not.
    let decoder = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rawler::get_decoder(&source)
    }))
    .map_err(|_| SpError::decode(&path_str, "decoder panicked on malformed input"))?
    .map_err(|e| SpError::decode(&path_str, e.to_string()))?;
    decode_with_decoder(&path_str, &source, decoder.as_ref())
}

fn decode_with_decoder(
    path_str: &str,
    source: &RawSource,
    decoder: &dyn Decoder,
) -> Result<DecodeOutput, SpError> {
    let params = RawDecodeParams::default();

    let rawler_image = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        decoder.raw_image(source, &params, false)
    }))
    .map_err(|_| SpError::decode(path_str, "decoder panicked on malformed input"))?
    .map_err(|e| SpError::decode(path_str, e.to_string()))?;

    // Metadata and preview are best-effort: their absence must not fail the
    // decode of otherwise-good sensor data.
    let (meta, exif_orientation) = read_metadata(path_str, decoder, source, &params)
        .unwrap_or_else(|| {
            (
                ImageMeta {
                    camera_make: rawler_image.clean_make.clone(),
                    camera_model: rawler_image.clean_model.clone(),
                    ..ImageMeta::default()
                },
                None,
            )
        });
    let preview = read_preview(path_str, decoder, source, &params, exif_orientation, false);

    let mut raw = convert_raw(rawler_image, path_str)?;
    // Hasselblad decoders report Normal even for portrait shots; the truth
    // is in EXIF tag 274 (1=normal, 3=180, 6=90 CW, 8=270 CW).
    if raw.orientation == Orientation::Normal {
        raw.orientation = match exif_orientation {
            Some(3) => Orientation::Rotate180,
            Some(6) => Orientation::Rotate90,
            Some(8) => Orientation::Rotate270,
            _ => Orientation::Normal,
        };
    }
    // Bake the rotation into the CFA so every downstream consumer (proxy,
    // tiles, crop, export) sees an upright image and the orientation field
    // is spent. ~100 ms one-time cost for 100 MP; far simpler than
    // orientation-aware coordinate math in the viewport and tiler.
    apply_orientation(&mut raw);
    Ok(DecodeOutput {
        raw: std::sync::Arc::new(raw),
        meta,
        preview,
    })
}

fn convert_raw(mut img: RawlerImage, display: &str) -> Result<RawImage, SpError> {
    if img.cpp != 1 {
        return Err(SpError::decode(
            display,
            match Path::new(display)
                .extension()
                .and_then(|value| value.to_str())
                .map(str::to_ascii_lowercase)
                .as_deref()
            {
                Some("dng") => "Linear DNG (e.g. Apple ProRAW) is not supported yet".to_owned(),
                Some("arw") => {
                    "Reduced-size three-channel Sony ARW is not supported yet".to_owned()
                }
                _ => format!(
                    "Multi-channel RAW layouts are not supported yet ({} components per pixel)",
                    img.cpp
                ),
            },
        ));
    }
    let width = u32::try_from(img.width)
        .map_err(|_| SpError::decode(display, "sensor width exceeds supported dimensions"))?;
    let height = u32::try_from(img.height)
        .map_err(|_| SpError::decode(display, "sensor height exceeds supported dimensions"))?;
    if width < 2 || height < 2 {
        return Err(SpError::decode(
            display,
            "Bayer sensor dimensions must be at least 2×2",
        ));
    }
    let sample_count = match &img.data {
        RawImageData::Integer(values) => values.len(),
        RawImageData::Float(values) => values.len(),
    };
    if img.width.checked_mul(img.height) != Some(sample_count) {
        return Err(SpError::decode(
            display,
            format!("CFA size mismatch: {sample_count} values for {width}×{height}"),
        ));
    }
    // Validate in rawler's native usize coordinates before narrowing to u32.
    // Rotation subtracts these extents from the full sensor dimensions.
    let active_area = if let Some(area) = img.active_area {
        if area.d.w < 2
            || area.d.h < 2
            || area
                .p
                .x
                .checked_add(area.d.w)
                .is_none_or(|end| end > img.width)
            || area
                .p
                .y
                .checked_add(area.d.h)
                .is_none_or(|end| end > img.height)
        {
            return Err(SpError::decode(
                display,
                "active area lies outside the Bayer sensor or is smaller than 2×2",
            ));
        }
        ActiveArea {
            x: area.p.x as u32,
            y: area.p.y as u32,
            width: area.d.w as u32,
            height: area.d.h as u32,
        }
    } else {
        ActiveArea {
            x: 0,
            y: 0,
            width,
            height,
        }
    };

    let cfa = parse_cfa(&img.camera.cfa.name).ok_or_else(|| {
        SpError::decode(
            display,
            if img.camera.cfa.width == 6 && img.camera.cfa.height == 6 {
                "Fujifilm X-Trans sensors are not supported yet".to_owned()
            } else {
                format!(
                    "Unsupported sensor color-filter layout '{}'",
                    img.camera.cfa.name
                )
            },
        )
    })?;

    // rawler provides XYZ→cam (rows = camera channels); we need cam→XYZ.
    // Primary source is the per-illuminant matrix map (prefer D65); the
    // legacy `xyz_to_cam` field is zeroed by some decode paths. Extracted
    // before `img.data` is moved out below.
    let xyz_to_cam: Mat3 = xyz_to_cam_matrix(&img)
        .ok_or_else(|| SpError::decode(display, "file carries no camera color matrix"))?;
    let cam_to_xyz = mat3_inverse(&xyz_to_cam)
        .ok_or_else(|| SpError::decode(display, "invalid or singular camera color matrix"))?;

    // Normalize float raws (rare DNGs) to u16 so the GPU upload path is one
    // format. rawler scales to the full u16 range.
    img.data.force_integer();
    let cfa_data = match img.data {
        RawImageData::Integer(v) => v,
        RawImageData::Float(_) => unreachable!("force_integer() converted above"),
    };

    // Rawler levels are RGBE-ordered vectors of flexible length; normalize
    // to per-channel RGBE arrays: [R, G, B, E/G2].
    let black_levels = spread4(
        &img.blacklevel
            .levels
            .iter()
            .map(|r| r.as_f32())
            .collect::<Vec<_>>(),
    );
    let white_levels = spread4(
        &img.whitelevel
            .0
            .iter()
            .map(|&w| w as f32)
            .collect::<Vec<_>>(),
    );

    // WB: rawler gives RGBE; E is NaN/0 for Bayer — mirror G into slot 4.
    let mut wb = img.wb_coeffs;
    if !wb[3].is_finite() || wb[3] == 0.0 {
        wb[3] = wb[1];
    }
    // Some files carry all-zero WB; fall back to neutral rather than black.
    if wb[..3].iter().any(|v| !v.is_finite() || *v <= 0.0) {
        wb = [1.0, 1.0, 1.0, 1.0];
    }

    Ok(RawImage {
        cfa_data,
        width,
        height,
        cfa,
        black_levels,
        white_levels,
        wb_coeffs: wb,
        cam_to_xyz,
        active_area,
        orientation: convert_orientation(img.orientation),
    })
}

/// Extract a 3×3 XYZ→cam matrix. Prefers the D65 entry of the illuminant
/// map, then any non-degenerate entry, then the legacy `xyz_to_cam` field.
fn xyz_to_cam_matrix(img: &RawlerImage) -> Option<Mat3> {
    let from_flat = |flat: &[f32]| -> Option<Mat3> {
        if flat.len() < 9 || flat.iter().all(|v| v.abs() < 1e-9) {
            return None;
        }
        Some([
            [flat[0], flat[1], flat[2]],
            [flat[3], flat[4], flat[5]],
            [flat[6], flat[7], flat[8]],
        ])
    };
    img.color_matrix
        .get(&RawlerIlluminant::D65)
        .and_then(|m| from_flat(m))
        .or_else(|| img.color_matrix.values().find_map(|m| from_flat(m)))
        .or_else(|| {
            let legacy: Mat3 = [img.xyz_to_cam[0], img.xyz_to_cam[1], img.xyz_to_cam[2]];
            let flat: Vec<f32> = legacy.iter().flatten().copied().collect();
            from_flat(&flat)
        })
}

/// Rotate a decoded RGBA preview per EXIF orientation (1/3/6/8).
fn rotate_preview(p: Preview, exif_orientation: Option<u16>) -> Preview {
    let (w, h) = (p.width as usize, p.height as usize);
    let rot = match exif_orientation {
        Some(3) => Orientation::Rotate180,
        Some(6) => Orientation::Rotate90,
        Some(8) => Orientation::Rotate270,
        _ => return p,
    };
    let (out_w, out_h) = match rot {
        Orientation::Rotate90 | Orientation::Rotate270 => (h, w),
        _ => (w, h),
    };
    let mut out = vec![0u8; w * h * 4];
    for y in 0..out_h {
        for x in 0..out_w {
            let (sx, sy) = match rot {
                Orientation::Rotate90 => (y, h - 1 - x),
                Orientation::Rotate180 => (w - 1 - x, h - 1 - y),
                Orientation::Rotate270 => (w - 1 - y, x),
                Orientation::Normal => (x, y),
            };
            let src = (sy * w + sx) * 4;
            let dst = (y * out_w + x) * 4;
            out[dst..dst + 4].copy_from_slice(&p.rgba8[src..src + 4]);
        }
    }
    Preview {
        width: out_w as u32,
        height: out_h as u32,
        rgba8: out,
    }
}

/// Physically rotate the CFA data per `raw.orientation`, updating dims,
/// active area and CFA pattern, then reset orientation to Normal.
pub(crate) fn apply_orientation(raw: &mut RawImage) {
    use rayon::prelude::*;
    let (w, h) = (raw.width as usize, raw.height as usize);
    let rot = raw.orientation;
    if rot == Orientation::Normal {
        return;
    }
    let src = &raw.cfa_data;
    let mut dst = vec![0u16; w * h];
    // Destination-space sampler: for each output pixel, its source coords.
    let (out_w, out_h): (usize, usize) = match rot {
        Orientation::Rotate90 | Orientation::Rotate270 => (h, w),
        _ => (w, h),
    };
    let src_of = |x: usize, y: usize| -> (usize, usize) {
        match rot {
            // 90° CW display rotation: out(x,y) = in(y, H-1-x)... derived so
            // that the image CONTENT rotates clockwise on screen.
            Orientation::Rotate90 => (y, h - 1 - x),
            Orientation::Rotate180 => (w - 1 - x, h - 1 - y),
            Orientation::Rotate270 => (w - 1 - y, x),
            Orientation::Normal => (x, y),
        }
    };
    dst.par_chunks_mut(out_w).enumerate().for_each(|(y, row)| {
        for (x, value) in row.iter_mut().enumerate() {
            let (sx, sy) = src_of(x, y);
            *value = src[sy * w + sx];
        }
    });

    // New CFA pattern: sample the colors the first output quad now holds.
    let color_of = |x: usize, y: usize| -> u8 {
        let (sx, sy) = src_of(x, y);
        raw.cfa.color_at(sx as u32, sy as u32)
    };
    let new_cfa = match (color_of(0, 0), color_of(1, 0), color_of(0, 1)) {
        (0, 1, 1) => CfaPattern::Rggb,
        (2, 1, 1) => CfaPattern::Bggr,
        (1, 0, 2) => CfaPattern::Grbg,
        (1, 2, 0) => CfaPattern::Gbrg,
        other => unreachable!("rotation of a Bayer quad stays Bayer: {other:?}"),
    };

    // Active area transforms with the same mapping (rect corners).
    let a = raw.active_area;
    let (ax, ay, aw, ah) = (
        a.x as usize,
        a.y as usize,
        a.width as usize,
        a.height as usize,
    );
    let new_area = match rot {
        Orientation::Rotate90 => sp_core::raw::ActiveArea {
            x: (h - ay - ah) as u32,
            y: ax as u32,
            width: ah as u32,
            height: aw as u32,
        },
        Orientation::Rotate180 => sp_core::raw::ActiveArea {
            x: (w - ax - aw) as u32,
            y: (h - ay - ah) as u32,
            width: aw as u32,
            height: ah as u32,
        },
        Orientation::Rotate270 => sp_core::raw::ActiveArea {
            x: ay as u32,
            y: (w - ax - aw) as u32,
            width: ah as u32,
            height: aw as u32,
        },
        Orientation::Normal => a,
    };

    raw.cfa_data = dst;
    raw.width = out_w as u32;
    raw.height = out_h as u32;
    raw.cfa = new_cfa;
    raw.active_area = new_area;
    raw.orientation = Orientation::Normal;
}

/// Normalize a 1/3/4-length per-channel level vector to 4 entries
/// in RGBE order: [R, G, B, E/G2].
fn spread4(levels: &[f32]) -> [f32; 4] {
    match levels {
        [] => [0.0; 4],
        [one] => [*one; 4],
        [a, b] => [*a, *b, *a, *b],
        [r, g, b] => [*r, *g, *b, *g],
        [r, g, b, e, ..] => [*r, *g, *b, *e],
    }
}

fn parse_cfa(name: &str) -> Option<CfaPattern> {
    match name.to_ascii_uppercase().as_str() {
        "RGGB" => Some(CfaPattern::Rggb),
        "BGGR" => Some(CfaPattern::Bggr),
        "GRBG" => Some(CfaPattern::Grbg),
        "GBRG" => Some(CfaPattern::Gbrg),
        _ => None,
    }
}

fn convert_orientation(o: RawlerOrientation) -> Orientation {
    match o {
        RawlerOrientation::Normal | RawlerOrientation::Unknown => Orientation::Normal,
        RawlerOrientation::Rotate90 => Orientation::Rotate90,
        RawlerOrientation::Rotate180 => Orientation::Rotate180,
        RawlerOrientation::Rotate270 => Orientation::Rotate270,
        // Mirrored orientations are vanishingly rare from cameras; treat as
        // their unmirrored rotation until Phase 3 geometry adds flips.
        RawlerOrientation::HorizontalFlip | RawlerOrientation::VerticalFlip => Orientation::Normal,
        RawlerOrientation::Transpose => Orientation::Rotate90,
        RawlerOrientation::Transverse => Orientation::Rotate270,
    }
}

/// `"Hasselblad"` + `"XCD 35-100E@60"` → `"Hasselblad XCD 35-100E@60"`,
/// skipping the maker when it is absent or already the model's prefix.
fn join_lens(make: &str, model: &str) -> String {
    let (make, model) = (make.trim(), model.trim());
    if make.is_empty() || model.starts_with(make) {
        model.to_owned()
    } else {
        format!("{make} {model}")
    }
}

fn convert_meta(m: &RawMetadata) -> ImageMeta {
    let exif = &m.exif;
    ImageMeta {
        camera_make: m.make.clone(),
        camera_model: m.model.clone(),
        // Prefer rawler's resolved LensDescription, but fall back to the
        // raw EXIF strings when its database has no entry — which is the
        // normal case for Hasselblad XCD glass. Hasselblad encodes the zoom
        // position in the model itself ("XCD 35-100E@60"), so the EXIF
        // string is often more specific than a database name would be.
        lens_model: m
            .lens
            .as_ref()
            .map(|l| join_lens(&l.lens_make, &l.lens_model))
            .or_else(|| {
                let model = exif.lens_model.as_deref()?.trim();
                (!model.is_empty())
                    .then(|| join_lens(exif.lens_make.as_deref().unwrap_or_default(), model))
            }),
        iso: exif.iso_speed.or(exif.iso_speed_ratings.map(u32::from)),
        shutter_s: exif.exposure_time.map(|r| r.as_f32()),
        aperture_f: exif.fnumber.map(|r| r.as_f32()),
        focal_length_mm: exif.focal_length.map(|r| r.as_f32()),
        captured_at: exif
            .date_time_original
            .clone()
            .or_else(|| exif.create_date.clone()),
        gps: exif.gps.as_ref().and_then(|gps| {
            let to_degrees = |parts: &[rawler::formats::tiff::Rational; 3]| {
                f64::from(parts[0].as_f32())
                    + f64::from(parts[1].as_f32()) / 60.0
                    + f64::from(parts[2].as_f32()) / 3600.0
            };
            let mut lat = to_degrees(gps.gps_latitude.as_ref()?);
            let mut lon = to_degrees(gps.gps_longitude.as_ref()?);
            if gps
                .gps_latitude_ref
                .as_deref()
                .is_some_and(|reference| reference.eq_ignore_ascii_case("S"))
            {
                lat = -lat;
            }
            if gps
                .gps_longitude_ref
                .as_deref()
                .is_some_and(|reference| reference.eq_ignore_ascii_case("W"))
            {
                lon = -lon;
            }
            (lat.is_finite()
                && lon.is_finite()
                && (-90.0..=90.0).contains(&lat)
                && (-180.0..=180.0).contains(&lon))
            .then_some([lat, lon])
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_unsupported_layouts_have_specific_messages() {
        for (path, expected) in [
            (
                "photo.DNG",
                "Linear DNG (e.g. Apple ProRAW) is not supported yet",
            ),
            (
                "photo.ARW",
                "Reduced-size three-channel Sony ARW is not supported yet",
            ),
        ] {
            let mut image = rawler_image();
            image.cpp = 3;
            assert!(convert_raw(image, path)
                .unwrap_err()
                .to_string()
                .contains(expected));
        }
        let mut image = rawler_image();
        image.camera.cfa = rawler::CFA::new(concat!(
            "GRGGBG", "BGBRGR", "GRGGBG", "GBGGRG", "RGRBGB", "GBGGRG"
        ));
        assert!(convert_raw(image, "photo.RAF")
            .unwrap_err()
            .to_string()
            .contains("Fujifilm X-Trans sensors are not supported yet"));
    }

    fn rawler_image() -> RawlerImage {
        use rawler::decoders::{CFAConfig, Camera, RawPhotometricInterpretation, WhiteLevel};
        let camera = Camera {
            clean_make: "Test make".into(),
            clean_model: "Test model".into(),
            cfa: rawler::CFA::new("RGGB"),
            xyz_to_cam: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [0.0; 3]],
            ..Default::default()
        };
        let photometric = RawPhotometricInterpretation::Cfa(CFAConfig::new_from_camera(&camera));
        RawlerImage::new_with_data(
            camera,
            RawImageData::Integer((0..8).collect()),
            4,
            2,
            1,
            [1.0; 4],
            photometric,
            None,
            Some(WhiteLevel::default()),
            false,
        )
    }

    /// Inject failures at the actual decoder boundary while retaining valid
    /// sensor data, avoiding a dependence on a proprietary camera fixture.
    struct OptionalPanicDecoder {
        metadata: bool,
        preview: bool,
        thumbnail: bool,
        full_image: bool,
    }

    impl Decoder for OptionalPanicDecoder {
        fn raw_image(
            &self,
            _source: &RawSource,
            _params: &RawDecodeParams,
            _dummy: bool,
        ) -> rawler::Result<RawlerImage> {
            Ok(rawler_image())
        }

        fn raw_metadata(
            &self,
            _source: &RawSource,
            _params: &RawDecodeParams,
        ) -> rawler::Result<RawMetadata> {
            assert!(!self.metadata, "malformed optional metadata");
            let mut metadata = RawMetadata::default();
            metadata.exif.orientation = Some(6);
            Ok(metadata)
        }

        fn preview_image(
            &self,
            _source: &RawSource,
            _params: &RawDecodeParams,
        ) -> rawler::Result<Option<image::DynamicImage>> {
            assert!(!self.preview, "malformed optional preview");
            Ok(None)
        }

        fn thumbnail_image(
            &self,
            _source: &RawSource,
            _params: &RawDecodeParams,
        ) -> rawler::Result<Option<image::DynamicImage>> {
            assert!(!self.thumbnail, "malformed optional thumbnail");
            Ok(Some(image::DynamicImage::ImageRgb8(
                image::RgbImage::from_pixel(2, 1, image::Rgb([40, 80, 120])),
            )))
        }

        fn full_image(
            &self,
            _source: &RawSource,
            _params: &RawDecodeParams,
        ) -> rawler::Result<Option<image::DynamicImage>> {
            assert!(!self.full_image, "malformed optional full image");
            Ok(Some(image::DynamicImage::ImageRgb8(
                image::RgbImage::from_pixel(2, 1, image::Rgb([120, 80, 40])),
            )))
        }

        fn format_dump(&self) -> rawler::analyze::FormatDump {
            unreachable!("decoding does not request diagnostic format dumps")
        }

        fn format_hint(&self) -> rawler::decoders::FormatHint {
            rawler::decoders::FormatHint::Unknown
        }
    }

    #[test]
    fn optional_decoder_panics_preserve_sensor_pixels_and_preview_fallbacks() {
        let source = RawSource::new_from_slice(&[]);
        let mut decoder = OptionalPanicDecoder {
            metadata: true,
            preview: true,
            thumbnail: false,
            full_image: false,
        };
        let output = decode_with_decoder("test.dng", &source, &decoder).unwrap();
        assert_eq!(output.raw.cfa_data, (0..8).collect::<Vec<_>>());
        assert_eq!(output.meta.camera_make, "Test make");
        assert_eq!(output.meta.camera_model, "Test model");
        let preview = output.preview.unwrap();
        assert_eq!((preview.width, preview.height), (2, 1));
        assert_eq!(&preview.rgba8[..4], &[40, 80, 120, 255]);

        // A malformed preview and thumbnail still permit the full-image JPEG
        // fallback, and readable EXIF continues to orient both outputs.
        decoder.metadata = false;
        decoder.thumbnail = true;
        let output = decode_with_decoder("test.dng", &source, &decoder).unwrap();
        assert_eq!((output.raw.width, output.raw.height), (2, 4));
        let preview = output.preview.unwrap();
        assert_eq!((preview.width, preview.height), (1, 2));
        assert_eq!(&preview.rgba8[..4], &[120, 80, 40, 255]);

        decoder.metadata = true;
        decoder.full_image = true;
        let output = decode_with_decoder("test.dng", &source, &decoder).unwrap();
        assert!(output.preview.is_none());
        assert_eq!(output.raw.cfa_data, (0..8).collect::<Vec<_>>());
    }

    #[test]
    fn conversion_rejects_invalid_sensor_and_active_area_before_rotation() {
        use rawler::imgop::{Dim2, Point, Rect};
        for (width, height) in [(0, 2), (4, 0), (1, 8), (8, 1), (4, 3)] {
            let mut image = rawler_image();
            image.width = width;
            image.height = height;
            assert!(convert_raw(image, "test.dng").is_err());
        }
        let mut truncated = rawler_image();
        let RawImageData::Integer(values) = &mut truncated.data else {
            unreachable!("integer test fixture")
        };
        values.pop();
        assert!(convert_raw(truncated, "test.dng").is_err());

        for area in [
            Rect::new(Point::new(1, 0), Dim2::new(4, 2)),
            Rect::new(Point::new(0, 1), Dim2::new(4, 2)),
            Rect::new(Point::new(0, 0), Dim2::new(0, 2)),
            Rect::new(Point::new(0, 0), Dim2::new(4, 1)),
            Rect::new(Point::new(usize::MAX, 0), Dim2::new(4, 2)),
        ] {
            let mut image = rawler_image();
            image.active_area = Some(area);
            image.orientation = RawlerOrientation::Rotate90;
            assert!(convert_raw(image, "test.dng").is_err());
        }

        let mut image = rawler_image();
        image.active_area = Some(Rect::new(Point::new(1, 0), Dim2::new(2, 2)));
        image.orientation = RawlerOrientation::Rotate90;
        let mut converted = convert_raw(image, "test.dng").unwrap();
        apply_orientation(&mut converted);
        assert_eq!(
            converted.active_area,
            ActiveArea {
                x: 0,
                y: 1,
                width: 2,
                height: 2
            }
        );
    }

    #[test]
    fn conversion_rejects_dimensions_and_crop_coordinates_that_would_truncate() {
        // These fields are usize in rawler but u32 downstream. On 64-bit
        // platforms the old casts could turn invalid values into valid ones.
        if let Some(oversized) = (u32::MAX as usize).checked_add(5) {
            let mut image = rawler_image();
            image.width = oversized;
            assert!(convert_raw(image, "test.dng").is_err());
            let mut image = rawler_image();
            image.active_area = Some(rawler::imgop::Rect::new(
                rawler::imgop::Point::new(oversized - 4, 0),
                rawler::imgop::Dim2::new(2, 2),
            ));
            assert!(convert_raw(image, "test.dng").is_err());
        }
    }

    #[test]
    fn conversion_rejects_nonfinite_camera_color_matrix() {
        for value in [f32::NAN, f32::INFINITY] {
            let mut image = rawler_image();
            image.xyz_to_cam[0][0] = value;
            assert!(convert_raw(image, "test.dng").is_err());
        }
    }

    #[test]
    fn spread4_handles_all_lengths() {
        assert_eq!(spread4(&[]), [0.0; 4]);
        assert_eq!(spread4(&[7.0]), [7.0; 4]);
        assert_eq!(spread4(&[1.0, 2.0, 3.0]), [1.0, 2.0, 3.0, 2.0]);
        assert_eq!(spread4(&[1.0, 2.0, 3.0, 4.0]), [1.0, 2.0, 3.0, 4.0]);
    }

    #[test]
    fn convert_meta_converts_exif_gps_and_hemispheres() {
        use rawler::exif::ExifGPS;
        use rawler::formats::tiff::Rational;

        let mut source = RawMetadata::default();
        source.exif.gps = Some(ExifGPS {
            gps_latitude: Some([
                Rational::new(52, 1),
                Rational::new(22, 1),
                Rational::new(30, 1),
            ]),
            gps_latitude_ref: Some("N".into()),
            gps_longitude: Some([
                Rational::new(4, 1),
                Rational::new(54, 1),
                Rational::new(0, 1),
            ]),
            gps_longitude_ref: Some("W".into()),
            ..ExifGPS::default()
        });

        let [lat, lon] = convert_meta(&source).gps.expect("valid GPS");
        assert!((lat - 52.375).abs() < 1.0e-6);
        assert!((lon + 4.9).abs() < 1.0e-6);
    }

    #[test]
    fn rotation_maps_pixels_and_pattern() {
        use sp_core::raw::ActiveArea;
        // 4×2 RGGB frame with distinct values.
        let base = RawImage {
            cfa_data: vec![1, 2, 3, 4, 5, 6, 7, 8],
            width: 4,
            height: 2,
            cfa: CfaPattern::Rggb,
            black_levels: [0.0; 4],
            white_levels: [65535.0; 4],
            wb_coeffs: [1.0; 4],
            cam_to_xyz: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            active_area: ActiveArea {
                x: 0,
                y: 0,
                width: 4,
                height: 2,
            },
            orientation: Orientation::Rotate90,
        };
        let mut r = base.clone();
        apply_orientation(&mut r);
        assert_eq!((r.width, r.height), (2, 4));
        // out(x,y) = in(y, H-1-x) with H=2: out(0,0)=in(0,1)=5.
        assert_eq!(r.cfa_data, vec![5, 1, 6, 2, 7, 3, 8, 4]);
        assert_eq!(r.orientation, Orientation::Normal);
        // Pattern consistency: colors at out coords match source colors.
        assert_eq!(
            r.cfa.color_at(0, 0),
            CfaPattern::Rggb.color_at(0, 1),
            "rotated pattern"
        );

        // 180°: reversal.
        let mut r = base.clone();
        r.orientation = Orientation::Rotate180;
        apply_orientation(&mut r);
        assert_eq!(r.cfa_data, vec![8, 7, 6, 5, 4, 3, 2, 1]);
        assert_eq!((r.width, r.height), (4, 2));

        // 270°: inverse of 90 — applying both yields original.
        let mut r = base.clone();
        r.orientation = Orientation::Rotate90;
        apply_orientation(&mut r);
        r.orientation = Orientation::Rotate270;
        apply_orientation(&mut r);
        assert_eq!(r.cfa_data, base.cfa_data);
        assert_eq!(r.cfa, base.cfa);
    }

    #[test]
    fn cfa_names() {
        assert_eq!(parse_cfa("RGGB"), Some(CfaPattern::Rggb));
        assert_eq!(parse_cfa("gbrg"), Some(CfaPattern::Gbrg));
        assert_eq!(parse_cfa("XTRANS"), None);
    }
}

#[cfg(test)]
mod preview_preflight_tests {
    #[test]
    fn preview_rejects_oversized_tiff_fields_before_rawler() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("malformed.3fr");
        let mut bytes = b"II\x2a\x00\x08\x00\x00\x00\x01\x00".to_vec();
        bytes.extend_from_slice(&256u16.to_le_bytes());
        bytes.extend_from_slice(&4u16.to_le_bytes());
        bytes.extend_from_slice(&u32::MAX.to_le_bytes());
        bytes.extend_from_slice(&26u32.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        std::fs::write(&path, bytes).unwrap();
        let error = super::decode_preview_impl(&path).unwrap_err().to_string();
        let full_error = super::decode_impl(&path).unwrap_err().to_string();
        assert_eq!(error, full_error);
        assert!(error.contains("declares 17179869180 bytes"), "{error}");
    }
}
