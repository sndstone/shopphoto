//! # sp-raw
//!
//! RAW file decoding (Phase 1). Wraps `rawler` 0.7 (pure Rust; supported-
//! camera database and decoders cover Canon, Nikon, Sony, Fujifilm, Pentax,
//! Panasonic/Leica, Olympus/OM, Phase One, Hasselblad and other common RAW
//! families) behind a stable API.
//!
//! The public API is final: `decode(path) -> DecodeOutput`. Blocking; call
//! from the decode pool, never the UI thread (BUILD.md §3.3). Must never
//! panic on malformed input — everything maps into `SpError`.

mod convert;
pub mod dcp;
pub mod lens_calibration;
pub mod lens_store;
mod preflight;
pub mod profile_store;
mod raster;

use sp_core::{ImageMeta, RawImage, SpError};
use std::path::Path;

/// An embedded camera-generated preview, decoded to RGBA8 for direct upload
/// as a UI texture (no image crates needed downstream).
#[derive(Debug, Clone)]
pub struct Preview {
    pub width: u32,
    pub height: u32,
    /// `width * height * 4` bytes, sRGB.
    pub rgba8: Vec<u8>,
}

/// Lightweight thumbnail decode result. Unlike [`DecodeOutput`], this does
/// not materialize the sensor mosaic and is suitable for network libraries.
#[derive(Debug)]
pub struct PreviewOutput {
    pub preview: Option<Preview>,
    pub meta: ImageMeta,
}

/// Result of decoding one RAW file.
#[derive(Debug)]
pub struct DecodeOutput {
    /// Shared because UI and export renderers may consume the same 100 MP
    /// CFA concurrently without copying its ~200 MB sensor buffer.
    pub raw: std::sync::Arc<RawImage>,
    pub meta: ImageMeta,
    pub preview: Option<Preview>,
}

/// A RAW or color-managed rendered photograph ready for non-destructive editing.
#[derive(Debug)]
pub struct SourceOutput {
    pub source: sp_core::source::ImageSource,
    pub meta: ImageMeta,
    pub preview: Option<Preview>,
}

pub fn is_supported_image(path: &Path) -> bool {
    is_supported(path)
        || path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
            sp_core::source::SUPPORTED_RASTER_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str())
        })
}

pub fn decode_source(path: &Path) -> Result<SourceOutput, SpError> {
    if is_supported(path) {
        let output = decode(path)?;
        Ok(SourceOutput {
            source: sp_core::source::ImageSource::Raw(output.raw),
            meta: output.meta,
            preview: output.preview,
        })
    } else if is_supported_image(path) {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| raster::decode(path))).map_err(
            |_| {
                SpError::decode(
                    path.display().to_string(),
                    "image decoder panicked on malformed input",
                )
            },
        )?
    } else {
        Err(SpError::decode(
            path.display().to_string(),
            "unsupported image extension",
        ))
    }
}

pub fn decode_source_preview(path: &Path) -> Result<PreviewOutput, SpError> {
    if is_supported(path) {
        decode_preview(path)
    } else {
        #[cfg(target_os = "macos")]
        if raster::has_system_preview(path) {
            return raster::decode_preview(path);
        }
        let output = decode_source(path)?;
        Ok(PreviewOutput {
            preview: output.preview,
            meta: output.meta,
        })
    }
}

/// File extensions this crate will attempt to decode, lowercase, no dot.
pub use sp_core::raw::SUPPORTED_RAW_EXTENSIONS as SUPPORTED_EXTENSIONS;

/// True if `path`'s extension is one we attempt to decode.
#[must_use]
pub fn is_supported(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| SUPPORTED_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

/// Decode a RAW file: sensor data, calibration, metadata, and (if present)
/// the embedded preview.
///
/// # Errors
/// [`SpError::Decode`] for corrupt/unsupported files, [`SpError::Io`] for
/// filesystem problems.
pub fn decode(path: &Path) -> Result<DecodeOutput, SpError> {
    if !is_supported(path) {
        return Err(SpError::decode(
            path.display().to_string(),
            "unsupported file extension",
        ));
    }
    convert::decode_impl(path)
}

/// Read camera metadata and an embedded preview without decoding the sensor
/// image. The source is mapped lazily so SMB/NFS thumbnails fetch only the
/// header, metadata and JPEG ranges they actually touch.
pub fn decode_preview(path: &Path) -> Result<PreviewOutput, SpError> {
    if !is_supported(path) {
        return Err(SpError::decode(
            path.display().to_string(),
            "unsupported file extension",
        ));
    }
    convert::decode_preview_impl(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn macos_document_types_cover_every_supported_extension() {
        let plist = include_str!("../../../packaging/macos/Info.plist.in");
        for extension in SUPPORTED_EXTENSIONS
            .iter()
            .chain(sp_core::source::SUPPORTED_RASTER_EXTENSIONS)
        {
            assert!(
                plist.contains(&format!("<string>{extension}</string>")),
                "Missing Finder extension {extension}"
            );
            assert!(is_supported_image(Path::new(&format!(
                "image.{}",
                extension.to_ascii_uppercase()
            ))));
        }
    }

    #[test]
    fn extension_gate() {
        assert!(is_supported(Path::new("a/b/IMG_0001.3FR")));
        assert!(is_supported(Path::new("x.fff")));
        assert!(is_supported(Path::new("x.DNG")));
        assert!(is_supported(Path::new("canon.CR3")));
        assert!(is_supported(Path::new("nikon.nef")));
        assert!(is_supported(Path::new("sony.ARW")));
        assert!(is_supported(Path::new("fuji.raf")));
        assert!(!is_supported(Path::new("x.jpg")));
        assert!(!is_supported(Path::new("noext")));
    }

    #[test]
    fn missing_file_is_io_error_not_panic() {
        let err = decode(Path::new("/nonexistent/file.3fr")).expect_err("must fail");
        assert!(matches!(err, SpError::Io { .. } | SpError::Decode { .. }));
    }
}
