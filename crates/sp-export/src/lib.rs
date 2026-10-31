//! # sp-export
//!
//! Export engine (Phase 5): encoders, resampling, export jobs and presets.
//!
//! This crate deliberately owns no GPU state. [`ExportImage`] is the handoff
//! from the tiled develop renderer; geometry, linear-light resizing, color
//! encoding, container writing and presets are deterministic CPU work that
//! can run on the two-thread export pool.

use serde::{Deserialize, Serialize};
use sp_core::ImageMeta;
use std::path::{Path, PathBuf};

mod avif_metadata;
mod encode;
pub mod gainmap;
pub mod gallery;
mod linear;
mod metadata;
mod output_profile;
pub mod print;
pub mod queue;
mod raster;
mod text_watermark;
mod ultrahdr;
pub mod workflow;

pub use encode::{
    encode_file, encode_file_hdr, encode_file_with_metadata, encode_ultra_hdr, icc_profile,
};
pub use linear::{linear_rec2020_icc, write_linear_tiff};
pub use metadata::PhotoMetadata;
pub use output_profile::{OutputIntent, OutputProfile};
pub use raster::{
    prepare_image, prepare_image_already_converted, prepare_image_already_converted_owned,
    prepare_pq_image, prepare_pq_image_owned, ExportImage,
};
pub use sp_core::color::OutputColorSpace as ExportColorSpace;

/// Current serialized export-preset schema.
pub const EXPORT_SETTINGS_VERSION: u32 = 2;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum WatermarkPosition {
    TopLeft,
    TopRight,
    BottomLeft,
    #[default]
    BottomRight,
    Center,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Watermark {
    pub png: PathBuf,
    pub position: WatermarkPosition,
    pub opacity: f32,
    pub scale_percent: f32,
}

/// Delivery mark only; this does not create an editable text layer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextWatermark {
    pub text: String,
    pub position: WatermarkPosition,
    pub opacity: f32,
    pub size_percent: f32,
    pub rgb: [u8; 3],
}
impl Default for TextWatermark {
    fn default() -> Self {
        Self {
            text: "© Photographer".into(),
            position: WatermarkPosition::BottomRight,
            opacity: 0.7,
            size_percent: 3.0,
            rgb: [255; 3],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileSizeLimit {
    pub max_kib: u32,
    pub minimum_quality: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Border {
    pub width_percent: f32,
    pub rgb: [u8; 3],
}

/// Output file format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExportFormat {
    Jpeg {
        quality: u8,
        chroma_444: bool,
    },
    Tiff {
        bit_depth: TiffDepth,
        compressed: bool,
    },
    Png {
        sixteen_bit: bool,
    },
    Avif {
        quality: u8,
    },
    /// Ultra HDR: a baseline SDR JPEG carrying a gain map, so HDR-aware
    /// viewers (Android, iOS 18+, Chrome, Safari) show the highlights while
    /// every other decoder shows the SDR rendition. Requires rendering the
    /// image twice, so it is slower than a plain JPEG.
    UltraHdr {
        quality: u8,
    },
    /// Lossless WebP uses the Rust encoder; lossy uses bundled libwebp.
    WebP {
        lossless: bool,
    },
    /// Lossy or lossless 16-bit JPEG XL, with ICC or HDR PQ signaling.
    JpegXl {
        quality: u8,
        #[serde(default)]
        lossless: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TiffDepth {
    Eight,
    Sixteen,
}

impl ExportFormat {
    /// Lowercase file extension, no dot.
    #[must_use]
    pub fn extension(self) -> &'static str {
        match self {
            ExportFormat::Jpeg { .. } => "jpg",
            ExportFormat::Tiff { .. } => "tif",
            ExportFormat::Png { .. } => "png",
            ExportFormat::Avif { .. } => "avif",
            // Deliberately a .jpg: the file IS a baseline JPEG, and giving
            // it another extension would stop ordinary viewers opening it.
            ExportFormat::UltraHdr { .. } => "jpg",
            ExportFormat::JpegXl { .. } => "jxl",
            ExportFormat::WebP { .. } => "webp",
        }
    }

    /// Whether this format accepts 16-bit integer RGB in the v1 exporter.
    #[must_use]
    pub fn supports_sixteen_bit(self) -> bool {
        matches!(
            self,
            ExportFormat::Tiff {
                bit_depth: TiffDepth::Sixteen,
                ..
            } | ExportFormat::Png { sixteen_bit: true }
                | ExportFormat::JpegXl { .. }
        )
    }
}

/// How the exported image is sized, applied after the working-resolution
/// (supersampling) mode.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub enum SizeMode {
    /// Working resolution as-is.
    #[default]
    Original,
    /// Scale down (never up) so the long edge is at most this many pixels.
    LongEdgePx(u32),
    /// Scale down (never up) so the short edge is at most this many pixels.
    ShortEdgePx(u32),
    /// Scale by a percentage, capped at 100% to avoid accidental upscaling.
    Percent(f32),
    /// Exact pixel size; aspect handled by prior crop, mismatches letterbox-
    /// free scale-to-fit (Phase 5 defines exact policy).
    Exact { width: u32, height: u32 },
    /// Scale down to approximately this many megapixels.
    Megapixels(f32),
}

impl SizeMode {
    /// Final output dimensions for a working-resolution image `w`×`h`.
    /// Never upscales.
    #[must_use]
    pub fn output_dims(self, w: u32, h: u32) -> (u32, u32) {
        let scale = |s: f64| -> (u32, u32) {
            let s = s.min(1.0);
            (
                ((w as f64 * s).round() as u32).max(1),
                ((h as f64 * s).round() as u32).max(1),
            )
        };
        match self {
            SizeMode::Original => (w, h),
            SizeMode::LongEdgePx(edge) => {
                let long = w.max(h) as f64;
                scale(edge as f64 / long)
            }
            SizeMode::ShortEdgePx(edge) => {
                let short = w.min(h) as f64;
                scale(edge as f64 / short)
            }
            SizeMode::Percent(percent) => scale(f64::from(percent.clamp(1.0, 100.0)) / 100.0),
            SizeMode::Exact { width, height } => {
                scale((width as f64 / w as f64).min(height as f64 / h as f64))
            }
            SizeMode::Megapixels(mp) => {
                let target = (mp as f64 * 1.0e6).max(1.0);
                scale((target / (w as f64 * h as f64)).sqrt())
            }
        }
    }
}

/// Output sharpening applied after resizing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum OutputSharpening {
    #[default]
    None,
    Low,
    Standard,
    High,
}

/// A complete export configuration (a saveable preset).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExportSettings {
    /// Optional local non-diffusion RGB super-resolution checkpoint.
    #[serde(default)]
    pub upscale_model: Option<PathBuf>,
    pub version: u32,
    pub format: ExportFormat,
    pub color_space: ExportColorSpace,
    /// Optional final RGB ICC conversion, applied after resizing/sharpening.
    #[serde(default)]
    pub output_profile: Option<OutputProfile>,
    #[serde(default)]
    pub output_intent: OutputIntent,
    #[serde(default = "default_true")]
    pub output_black_point_compensation: bool,
    #[serde(default = "default_quality")]
    pub webp_quality: u8,
    #[serde(default)]
    pub file_size_limit: Option<FileSizeLimit>,
    pub size: SizeMode,
    /// Final-size, luminance-preserving output sharpening.
    #[serde(default)]
    pub output_sharpening: OutputSharpening,
    /// Strip GPS coordinates from copied EXIF.
    pub strip_gps: bool,
    /// Copy camera/exposure metadata into formats that support EXIF.
    #[serde(default = "default_true")]
    pub copy_metadata: bool,
    /// Include captions, keywords and creator/rights information.
    #[serde(default = "default_true")]
    pub copy_descriptive_metadata: bool,
    /// Per-photo metadata supplied by the caller, never stored in a preset.
    #[serde(skip)]
    pub photo_metadata: PhotoMetadata,
    /// Destination directory. An empty path means the source directory.
    #[serde(default)]
    pub destination: PathBuf,
    /// Tokens: `{name}`, `{date}`, `{seq}`.
    #[serde(default = "default_pattern")]
    pub filename_pattern: String,
    /// Optional final-size PNG watermark, composited after output sharpening.
    #[serde(default)]
    pub watermark: Option<Watermark>,
    #[serde(default)]
    pub text_watermark: Option<TextWatermark>,
    /// Optional solid inset border, rendered at final output size.
    #[serde(default)]
    pub border: Option<Border>,
}

const fn default_true() -> bool {
    true
}
const fn default_quality() -> u8 {
    90
}

fn default_pattern() -> String {
    "{name}-edited".to_owned()
}

impl Default for ExportSettings {
    fn default() -> Self {
        Self {
            version: EXPORT_SETTINGS_VERSION,
            upscale_model: None,
            format: ExportFormat::Jpeg {
                quality: 90,
                chroma_444: true,
            },
            color_space: ExportColorSpace::Srgb,
            output_profile: None,
            output_intent: OutputIntent::default(),
            output_black_point_compensation: true,
            webp_quality: 90,
            file_size_limit: None,
            size: SizeMode::Original,
            output_sharpening: OutputSharpening::None,
            strip_gps: false,
            copy_metadata: true,
            copy_descriptive_metadata: true,
            photo_metadata: PhotoMetadata::default(),
            destination: PathBuf::new(),
            filename_pattern: default_pattern(),
            watermark: None,
            text_watermark: None,
            border: None,
        }
    }
}

/// A named, saveable export configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExportPreset {
    pub name: String,
    pub settings: ExportSettings,
}

impl ExportPreset {
    /// Practical starting points shown before the user saves custom presets.
    #[must_use]
    pub fn built_ins() -> Vec<Self> {
        vec![
            Self {
                name: "Web · Full HD".into(),
                settings: ExportSettings {
                    format: ExportFormat::Jpeg {
                        quality: 88,
                        chroma_444: false,
                    },
                    size: SizeMode::LongEdgePx(2_048),
                    output_sharpening: OutputSharpening::Standard,
                    filename_pattern: "{name}-web".into(),
                    strip_gps: true,
                    ..ExportSettings::default()
                },
            },
            Self {
                name: "Social · 4 MP".into(),
                settings: ExportSettings {
                    format: ExportFormat::Jpeg {
                        quality: 90,
                        chroma_444: false,
                    },
                    size: SizeMode::Megapixels(4.0),
                    output_sharpening: OutputSharpening::Standard,
                    filename_pattern: "{name}-social".into(),
                    strip_gps: true,
                    ..ExportSettings::default()
                },
            },
            Self {
                name: "Print · P3 TIFF".into(),
                settings: ExportSettings {
                    format: ExportFormat::Tiff {
                        bit_depth: TiffDepth::Sixteen,
                        compressed: true,
                    },
                    color_space: ExportColorSpace::DisplayP3,
                    output_sharpening: OutputSharpening::Low,
                    filename_pattern: "{name}-print".into(),
                    ..ExportSettings::default()
                },
            },
            Self {
                name: "Archive · 16-bit PNG".into(),
                settings: ExportSettings {
                    format: ExportFormat::Png { sixteen_bit: true },
                    color_space: ExportColorSpace::Rec2020,
                    filename_pattern: "{name}-archive".into(),
                    ..ExportSettings::default()
                },
            },
            Self {
                name: "Modern · AVIF".into(),
                settings: ExportSettings {
                    format: ExportFormat::Avif { quality: 82 },
                    size: SizeMode::LongEdgePx(3_840),
                    output_sharpening: OutputSharpening::Low,
                    filename_pattern: "{name}-avif".into(),
                    strip_gps: true,
                    ..ExportSettings::default()
                },
            },
        ]
    }
}

/// Load named presets. Missing files produce an empty list; malformed files
/// are reported so the UI can preserve/quarantine them.
pub fn load_presets(path: &Path) -> Result<Vec<ExportPreset>, ExportError> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|source| ExportError::Preset {
            path: path.to_path_buf(),
            reason: source.to_string(),
        }),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(source) => Err(ExportError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

/// Save named presets atomically.
pub fn save_presets(path: &Path, presets: &[ExportPreset]) -> Result<(), ExportError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| ExportError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    let bytes = serde_json::to_vec_pretty(presets).map_err(|source| ExportError::Preset {
        path: path.to_path_buf(),
        reason: source.to_string(),
    })?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, bytes).map_err(|source| ExportError::Io {
        path: tmp.clone(),
        source,
    })?;
    std::fs::rename(&tmp, path).map_err(|source| ExportError::Io {
        path: path.to_path_buf(),
        source,
    })
}

/// Expand an export filename pattern and append the selected extension.
#[must_use]
pub fn output_path(
    source: &Path,
    settings: &ExportSettings,
    meta: Option<&ImageMeta>,
    sequence: usize,
) -> PathBuf {
    let name = source
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("image");
    let date = meta
        .and_then(|value| value.captured_at.as_deref())
        .and_then(|value| value.get(..10))
        .unwrap_or("undated");
    let stem = settings
        .filename_pattern
        .replace("{name}", name)
        .replace("{date}", date)
        .replace("{seq}", &format!("{:04}", sequence + 1));
    let safe: String = stem
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '/' | '\\' | ':') {
                '_'
            } else {
                c
            }
        })
        .collect();
    let dir = if settings.destination.as_os_str().is_empty() {
        source.parent().unwrap_or_else(|| Path::new("."))
    } else {
        settings.destination.as_path()
    };
    dir.join(format!("{safe}.{}", settings.format.extension()))
}

/// Export failures retain the exact path and preserve the previous file.
#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    #[error("could not access {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("could not encode {path}: {reason}")]
    Encode { path: PathBuf, reason: String },
    #[error("invalid export preset {path}: {reason}")]
    Preset { path: PathBuf, reason: String },
    #[error("export was cancelled")]
    Cancelled,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_edge_never_upscales() {
        assert_eq!(
            SizeMode::LongEdgePx(2048).output_dims(1000, 800),
            (1000, 800)
        );
        assert_eq!(SizeMode::LongEdgePx(500).output_dims(1000, 800), (500, 400));
    }

    #[test]
    fn megapixel_target_close() {
        // X2D Super2x2 working res ≈ 5828×4371 ≈ 25.5 MP → ask for 12.0 MP.
        let (w, h) = SizeMode::Megapixels(12.0).output_dims(5828, 4371);
        let mp = (w as f64 * h as f64) / 1.0e6;
        assert!((mp - 12.0).abs() < 0.1, "got {mp} MP");
        // Aspect preserved.
        assert!((w as f64 / h as f64 - 5828.0 / 4371.0).abs() < 0.01);
    }

    #[test]
    fn short_edge_and_percent_preserve_aspect_without_upscaling() {
        assert_eq!(
            SizeMode::ShortEdgePx(500).output_dims(2_000, 1_000),
            (1_000, 500)
        );
        assert_eq!(
            SizeMode::Percent(50.0).output_dims(2_000, 1_000),
            (1_000, 500)
        );
        assert_eq!(
            SizeMode::Percent(150.0).output_dims(2_000, 1_000),
            (2_000, 1_000)
        );
    }

    #[test]
    fn original_is_identity() {
        assert_eq!(SizeMode::Original.output_dims(123, 456), (123, 456));
    }

    #[test]
    fn exact_size_is_an_aspect_preserving_bounding_box() {
        assert_eq!(
            SizeMode::Exact {
                width: 1_000,
                height: 1_000
            }
            .output_dims(4_000, 3_000),
            (1_000, 750)
        );
        assert_eq!(
            SizeMode::Exact {
                width: 8_000,
                height: 8_000
            }
            .output_dims(4_000, 3_000),
            (4_000, 3_000)
        );
    }

    #[test]
    fn filename_tokens_and_sanitizing() {
        let settings = ExportSettings {
            filename_pattern: "{date}/{name}:{seq}".into(),
            ..Default::default()
        };
        let meta = ImageMeta {
            captured_at: Some("2026-07-20T12:30:00Z".into()),
            ..Default::default()
        };
        assert_eq!(
            output_path(Path::new("/photos/A.3FR"), &settings, Some(&meta), 2),
            PathBuf::from("/photos/2026-07-20_A_0003.jpg")
        );
    }

    #[test]
    fn presets_round_trip() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("export_presets.json");
        let presets = vec![ExportPreset {
            name: "Web".into(),
            settings: ExportSettings::default(),
        }];
        save_presets(&path, &presets).expect("save");
        assert_eq!(load_presets(&path).expect("load"), presets);
    }

    #[test]
    fn built_in_presets_are_named_and_valid() {
        let presets = ExportPreset::built_ins();
        assert!(presets.len() >= 5);
        assert!(presets.iter().all(|preset| !preset.name.is_empty()));
        assert!(presets
            .iter()
            .any(|preset| preset.settings.output_sharpening != OutputSharpening::None));
    }
}
