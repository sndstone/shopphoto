//! Editable HDR/stack masters: atomic, strip-written RGB32F linear TIFF.
use sp_core::source::RasterImage;
use std::{
    io::BufWriter,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

pub fn linear_rec2020_icc() -> Result<Vec<u8>, String> {
    let mut profile = moxcms::ColorProfile::new_bt2020();
    let linear = moxcms::ToneReprCurve::Lut(Vec::new());
    profile.red_trc = Some(linear.clone());
    profile.green_trc = Some(linear.clone());
    profile.blue_trc = Some(linear);
    // The TRCs describe linear light, not BT.2020's usual transfer function.
    profile.cicp = None;
    profile.encode().map_err(|e| e.to_string())
}

/// Preserves unclipped highlights and signed working RGB without quantization
/// to an integer output space. Refuses replacement of an existing file.
pub fn write_linear_tiff(
    image: &RasterImage,
    destination: &Path,
    description: &str,
    cancel: &AtomicBool,
) -> Result<(), String> {
    if image.width == 0
        || image.height == 0
        || image.linear_rec2020.len() != image.width as usize * image.height as usize
    {
        return Err("Invalid linear TIFF pixels".into());
    }
    let directory = destination.parent().ok_or("Missing output directory")?;
    let mut temporary = tempfile::NamedTempFile::new_in(directory).map_err(|e| e.to_string())?;
    {
        let mut writer = BufWriter::new(temporary.as_file_mut());
        let mut encoder =
            tiff::encoder::TiffEncoder::new(&mut writer).map_err(|e| e.to_string())?;
        let mut output = encoder
            .new_image::<tiff::encoder::colortype::RGB32Float>(image.width, image.height)
            .map_err(|e| e.to_string())?;
        output.rows_per_strip(32).map_err(|e| e.to_string())?;
        output
            .encoder()
            .write_tag(
                tiff::tags::Tag::IccProfile,
                linear_rec2020_icc()?.as_slice(),
            )
            .map_err(|e| e.to_string())?;
        output
            .encoder()
            .write_tag(tiff::tags::Tag::Software, "ShopPhoto")
            .map_err(|e| e.to_string())?;
        output
            .encoder()
            .write_tag(tiff::tags::Tag::ImageDescription, description)
            .map_err(|e| e.to_string())?;
        let mut offset = 0usize;
        while output.next_strip_sample_count() > 0 {
            if cancel.load(Ordering::Relaxed) {
                return Err("Merge cancelled".into());
            }
            let count = output.next_strip_sample_count() as usize / 3;
            let mut strip = Vec::with_capacity(count * 3);
            for pixel in &image.linear_rec2020[offset..offset + count] {
                if pixel[..3].iter().any(|v| !v.is_finite()) || pixel[3] != 1.0 {
                    return Err("Linear TIFF requires finite opaque RGB pixels".into());
                }
                strip.extend_from_slice(&pixel[..3]);
            }
            output.write_strip(&strip).map_err(|e| e.to_string())?;
            offset += count;
        }
        output.finish().map_err(|e| e.to_string())?;
        use std::io::Write;
        writer.flush().map_err(|e| e.to_string())?;
    }
    temporary.as_file().sync_all().map_err(|e| e.to_string())?;
    if cancel.load(Ordering::Relaxed) {
        return Err("Merge cancelled".into());
    }
    temporary
        .persist_noclobber(destination)
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn float_tiff_contains_an_independently_readable_icc_profile() {
        let image = RasterImage {
            width: 8,
            height: 8,
            linear_rec2020: vec![[0.25, 1.5, 4.0, 1.0]; 64],
            bits_per_channel: 32,
            embedded_icc: None,
        };
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("float.tif");
        write_linear_tiff(&image, &path, "test", &AtomicBool::new(false)).unwrap();
        let mut decoder = tiff::decoder::Decoder::new(std::fs::File::open(&path).unwrap()).unwrap();
        let icc = decoder
            .get_tag_u8_vec(tiff::tags::Tag::IccProfile)
            .expect("ICC TIFF field");
        assert_eq!(icc, linear_rec2020_icc().unwrap());
        use image::ImageDecoder;
        let mut direct = image::codecs::tiff::TiffDecoder::new(std::io::BufReader::new(
            std::fs::File::open(&path).unwrap(),
        ))
        .unwrap();
        assert!(direct.icc_profile().unwrap().is_some(), "direct decoder");
    }
}
