//! macOS system codecs. ImageIO owns allocations until the RAII guard drops;
//! color conversion is shared with the other raster decoders.
use super::*;
use std::ffi::{c_char, CStr, CString};
use std::os::unix::ffi::OsStrExt;

#[repr(C)]
struct Frame {
    pixels: *mut u16,
    width: usize,
    height: usize,
    orientation: u32,
    icc: *mut u8,
    icc_len: usize,
    metadata: *mut u8,
    metadata_len: usize,
}

unsafe extern "C" {
    fn sp_imageio_decode(
        path: *const c_char,
        thumbnail: i32,
        out: *mut Frame,
        error: *mut c_char,
        error_len: usize,
    ) -> i32;
    fn sp_imageio_free(frame: *mut Frame);
}

impl Drop for Frame {
    fn drop(&mut self) {
        // SAFETY: the bridge initializes every field and owns these allocations.
        unsafe { sp_imageio_free(self) };
    }
}

pub(super) fn decode(path: &Path, thumbnail: bool) -> Result<SourceOutput, SpError> {
    let fail = |message: String| SpError::decode(path.display().to_string(), message);
    let filename = CString::new(path.as_os_str().as_bytes()).map_err(|e| fail(e.to_string()))?;
    // SAFETY: all-zero is the bridge's empty frame representation.
    let mut frame: Frame = unsafe { std::mem::zeroed() };
    let mut error = [0 as c_char; 512];
    // SAFETY: pointers are valid for the call; the frame guard frees results.
    if unsafe {
        sp_imageio_decode(
            filename.as_ptr(),
            i32::from(thumbnail),
            &mut frame,
            error.as_mut_ptr(),
            error.len(),
        )
    } == 0
    {
        // SAFETY: the error buffer is zero-initialized and snprintf-terminated.
        return Err(fail(
            unsafe { CStr::from_ptr(error.as_ptr()) }
                .to_string_lossy()
                .into(),
        ));
    }
    // SAFETY: successful bridge calls return width*height*4 native u16s and ICC.
    let (samples, embedded_icc) = unsafe {
        (
            std::slice::from_raw_parts(frame.pixels, frame.width * frame.height * 4),
            std::slice::from_raw_parts(frame.icc, frame.icc_len).to_vec(),
        )
    };
    let rgba = image::ImageBuffer::<image::Rgba<u16>, _>::from_raw(
        frame.width as u32,
        frame.height as u32,
        samples.to_vec(),
    )
    .ok_or_else(|| fail("Invalid ImageIO bitmap dimensions".into()))?;
    let meta = if frame.metadata_len > 0 {
        // SAFETY: metadata is an owned JSON allocation of the given length.
        let bytes = unsafe { std::slice::from_raw_parts(frame.metadata, frame.metadata_len) };
        // Missing optional fields deserialize as None; the two required strings
        // are always supplied by the bridge.
        serde_json::from_slice(bytes).map_err(|e| fail(format!("ImageIO metadata: {e}")))?
    } else {
        ImageMeta::default()
    };
    let orientation = frame.orientation;
    drop(frame);
    let mut decoded = DynamicImage::ImageRgba16(rgba);
    decoded.apply_orientation(
        image::metadata::Orientation::from_exif(orientation as u8)
            .unwrap_or(image::metadata::Orientation::NoTransforms),
    );
    let rgba = decoded.into_rgba16();
    let (width, height) = rgba.dimensions();
    if rgba.pixels().all(|pixel| pixel.0 == [0; 4]) {
        return Err(fail("ImageIO returned no decoded pixels; the macOS system codec may be unavailable or blocked".into()));
    }
    if rgba.pixels().any(|pixel| pixel[3] != u16::MAX) {
        return Err(fail("Transparent photographs are unsupported; flatten against your chosen background before opening".into()));
    }
    let encoded: Vec<f32> = rgba
        .pixels()
        .flat_map(|p| p.0[..3].iter().map(|v| *v as f32 / 65535.0))
        .collect();
    drop(rgba);
    let converted = convert_to_working(&encoded, Some(&embedded_icc)).map_err(fail)?;
    drop(encoded);
    let pixels: Vec<_> = converted
        .chunks_exact(3)
        .map(|p| [p[0], p[1], p[2], 1.0])
        .collect();
    Ok(SourceOutput {
        preview: Some(preview(width, height, &pixels)),
        meta,
        source: ImageSource::Raster(Arc::new(RasterImage {
            width,
            height,
            linear_rec2020: pixels,
            bits_per_channel: 16,
            embedded_icc: Some(embedded_icc),
        })),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    unsafe extern "C" {
        fn sp_imageio_test_image(path: *const c_char, orientation: u32, heic: i32, p3: i32) -> i32;
    }

    #[test]
    fn heic_full_resolution_orientation_and_color() {
        check_fixture(true, false);
    }

    #[test]
    fn imageio_bitmap_orientation_and_color_without_system_hevc_codec() {
        check_fixture(false, false);
        check_fixture(false, true);
    }

    fn check_fixture(heic: bool, p3: bool) {
        let directory = tempfile::tempdir().unwrap();
        for orientation in 1..=8 {
            let path = directory.path().join(format!(
                "fixture-{orientation}.{}",
                if heic { "heic" } else { "png" }
            ));
            let filename = CString::new(path.as_os_str().as_bytes()).unwrap();
            // SAFETY: the fixture writer only reads this terminated filename.
            if unsafe {
                sp_imageio_test_image(
                    filename.as_ptr(),
                    orientation,
                    i32::from(heic),
                    i32::from(p3),
                )
            } == 0
            {
                assert!(heic, "PNG fixture encoder must be available");
                eprintln!(
                    "Skipping HEIC fixture: system HEVC encoder unavailable (sandbox or OS codec)"
                );
                return;
            }
            let output = if heic {
                crate::decode_source(&path)
            } else {
                decode(&path, false)
            }
            .unwrap();
            let expected = if orientation >= 5 {
                (64, 128)
            } else {
                (128, 64)
            };
            assert_eq!(output.source.dimensions(), expected);
            assert_eq!(output.meta.camera_make, "ShopPhoto Test");
            assert_eq!(output.meta.camera_model, "ImageIO fixture");
            assert_eq!(output.meta.iso, Some(200));
            assert!((output.meta.shutter_s.unwrap() - 1.0 / 125.0).abs() < 0.00001);
            assert!((output.meta.aperture_f.unwrap() - 2.8).abs() < 0.001);
            assert_eq!(output.meta.focal_length_mm, Some(35.0));
            assert_eq!(
                output.meta.captured_at.as_deref(),
                Some("2026-10-22T12:34:56")
            );
            let ImageSource::Raster(image) = output.source else {
                panic!("raster")
            };
            // Neutral green should stay close to the independently transformed
            // sRGB signal; red and blue patches must retain their channel order.
            let icc = if p3 {
                ColorProfile::new_display_p3()
            } else {
                ColorProfile::new_srgb()
            }
            .encode()
            .unwrap();
            let patches: [[f32; 3]; 4] = [
                [200.0, 100.0, 40.0],
                [40.0, 100.0, 200.0],
                [200.0, 160.0, 40.0],
                [40.0, 160.0, 200.0],
            ]
            .map(|rgb| {
                convert_to_working(&rgb.map(|v| v / 255.0), Some(&icc))
                    .unwrap()
                    .try_into()
                    .unwrap()
            });
            let mut reference =
                DynamicImage::ImageRgb32F(image::Rgb32FImage::from_fn(128, 64, |x, y| {
                    image::Rgb(patches[usize::from(x >= 64) + 2 * usize::from(y >= 32)])
                }));
            reference.apply_orientation(
                image::metadata::Orientation::from_exif(orientation as u8).unwrap(),
            );
            let reference = reference.into_rgb32f();
            for (x, y) in [
                (3, 3),
                (3, expected.1 - 4),
                (expected.0 - 4, 3),
                (expected.0 - 4, expected.1 - 4),
            ] {
                let actual = image.linear_rec2020[(y * expected.0 + x) as usize];
                for (a, e) in actual[..3].iter().zip(reference.get_pixel(x, y).0) {
                    assert!(
                        (a - e).abs() < 0.06,
                        "orientation {orientation}: {actual:?} vs {e}"
                    );
                }
            }
            let preview = decode(&path, true).unwrap().preview.unwrap();
            assert_eq!((preview.width, preview.height), expected);
        }
    }

    #[test]
    fn owner_heic_sample_when_available() {
        let Some(directory) = std::env::var_os("SP_TEST_RAWS") else {
            return;
        };
        let path = Path::new(&directory).join("IMG_0486.HEIC");
        if !path.is_file() {
            return;
        }
        let output = crate::decode_source(&path).unwrap();
        let ImageSource::Raster(image) = output.source else {
            panic!("raster")
        };
        assert!(image.width > 512 && image.height > 512);
        assert!(image
            .linear_rec2020
            .iter()
            .all(|p| p.iter().all(|v| v.is_finite())));
        assert!(image
            .linear_rec2020
            .iter()
            .any(|p| p[0].max(p[1]).max(p[2]) > 0.1));
        let preview = crate::decode_source_preview(&path)
            .unwrap()
            .preview
            .unwrap();
        assert!(preview.width.max(preview.height) <= 512);
    }
}
