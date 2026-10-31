//! # sp-gpu
//!
//! GPU device management, texture pooling, and the compute-pass graph. Owns
//! all `wgpu` interaction below the app's surface/present layer.
//!
//! Architectural commitments (BUILD.md §3, §10, Phase 6):
//! - All intermediate textures are `rgba16float` (raw stages `r16uint` /
//!   `r32float`); working space is scene-referred Linear Rec.2020.
//! - The texture pool recycles allocations by (size, format); steady-state
//!   editing performs zero texture allocations per frame.
//! - HDR-capable export remains independent of the presentation surface.
//! - Native EDR presentation is feature-gated by wgpu surface support; the
//!   current wgpu 29 application surface presents the documented SDR view.
//!
//! Device context, compiled compute pipelines, and texture helpers live here;
//! the executor lives in `sp-pipeline`.

pub mod context;
pub mod pipelines;
pub mod readback;
pub mod upload;

pub use context::{DeviceLossMonitor, GpuContext};
pub use readback::ReadbackRing;
pub use upload::StagingRing;

use serde::{Deserialize, Serialize};

/// Identifies one compute pass in the develop pipeline, in canonical
/// execution order (BUILD.md §3.2). The pipeline DAG culls passes whose
/// parameters are neutral.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum PassId {
    /// Black-level subtract, normalize, hot-pixel suppression. `r16uint → r32float`.
    RawPrepare,
    /// Optional pre-demosaic noise reduction (Phase 4).
    RawDenoise,
    /// Full-quality Malvar–He–Cutler 5×5 demosaic.
    DemosaicMalvar,
    /// 2×2 Bayer quad → 1 RGB pixel. Fast preview AND the Super2x2
    /// supersampling path (Phase 5).
    DemosaicSuperpixel,
    /// White balance gains + camera matrix → Linear Rec.2020.
    WbColor,
    /// WB-resolved DNG HueSat map, before tone.
    ProfileColorPre,
    /// Exposure/contrast/highlights/shadows/whites/blacks as a 1-D LUT,
    /// then tone curve and HSL.
    ToneColor,
    /// DNG look table + fitted RGB LUT, after tone.
    ProfileColorPost,
    /// Selective purple/green axial chromatic-aberration removal.
    Defringe,
    /// Chroma then luminance noise reduction in Oklab (Phase 4).
    NoiseReduction,
    /// Oklab-L texture, clarity, and dehaze adjustment.
    LocalContrast,
    /// Masked local adjustments plus non-destructive heal/clone spots.
    Retouch,
    /// Luminance unsharp mask.
    Sharpen,
    /// Box-average 3×3/4×4 supersampling after demosaic (Phase 5;
    /// 2×2 uses `DemosaicSuperpixel` instead).
    Supersample,
    /// Working space → display or export encoding + dither.
    OutputTransform,
    /// UI-over-image composite to the swapchain.
    Composite,
}

impl PassId {
    /// WGSL source file for this pass, relative to `shaders/`.
    #[must_use]
    pub fn shader_file(self) -> &'static str {
        match self {
            PassId::RawPrepare => "raw_prepare.wgsl",
            PassId::RawDenoise => "nr_raw.wgsl",
            PassId::DemosaicMalvar => "demosaic_malvar.wgsl",
            PassId::DemosaicSuperpixel => "demosaic_superpixel.wgsl",
            PassId::WbColor => "wb_color.wgsl",
            PassId::ProfileColorPre | PassId::ProfileColorPost => "huesat_lut.wgsl",
            PassId::ToneColor => "tone_color.wgsl",
            PassId::Defringe => "defringe.wgsl",
            PassId::NoiseReduction => "nr_main.wgsl",
            PassId::LocalContrast => "local_contrast.wgsl",
            PassId::Retouch => "retouch.wgsl",
            PassId::Sharpen => "sharpen.wgsl",
            PassId::Supersample => "supersample.wgsl",
            PassId::OutputTransform => "output_transform.wgsl",
            PassId::Composite => "composite.wgsl",
        }
    }
}

/// Compute workgroup size used by all image passes. 16×16 is the sweet spot
/// for Apple TBDR GPUs (BUILD.md §10.3) and fine elsewhere.
pub const WORKGROUP_DIM: u32 = 16;

/// Dispatch grid for an image of `width`×`height` pixels.
#[must_use]
pub fn dispatch_size(width: u32, height: u32) -> (u32, u32) {
    (
        width.div_ceil(WORKGROUP_DIM),
        height.div_ceil(WORKGROUP_DIM),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dispatch_covers_image() {
        assert_eq!(dispatch_size(16, 16), (1, 1));
        assert_eq!(dispatch_size(17, 16), (2, 1));
        assert_eq!(dispatch_size(11656, 8742), (729, 547)); // X2D full frame
    }
}
