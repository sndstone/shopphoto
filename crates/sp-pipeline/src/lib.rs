//! # sp-pipeline
//!
//! The develop pipeline: turns (RawImage, CameraProfile, EditState) into
//! rendered pixels by scheduling `sp-gpu` passes. Owns proxy
//! logic, tiling, and render caching (BUILD.md §3.5).
//! The render-request vocabulary and pass planner remain pure and testable;
//! [`GpuRenderer`] executes the complete proxy, 1:1 and export paths.

pub mod auto_ca;
pub mod auto_defringe;
pub mod auto_level;
pub mod auto_tone;
pub mod match_fit;
pub mod match_io;
pub mod proof;
pub mod renderer;
pub mod tone;

pub use renderer::GpuRenderer;

use sp_core::edit::ResolutionMode;
use sp_core::EditState;
use sp_gpu::PassId;

/// What a render is for; determines resolution strategy (BUILD.md §3.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderTarget {
    /// Interactive viewport at ≤ 2× viewport resolution (proxy).
    Proxy { viewport_w: u32, viewport_h: u32 },
    /// 1:1 inspection: render only the visible tile + margin at full working
    /// resolution.
    OneToOne { tile_x: u32, tile_y: u32 },
    /// Full working-resolution render, tiled, for export.
    Export,
}

/// Compute the ordered list of passes for an edit. Pure function: culls
/// passes whose parameters are neutral, picks the demosaic variant from the
/// resolution mode and target. This is the single source of truth for pass
/// selection — the GPU executor must not second-guess it.
#[must_use]
pub fn plan_passes(edit: &EditState, target: RenderTarget) -> Vec<PassId> {
    let mut passes = vec![PassId::RawPrepare];

    // Pre-demosaic NR (`nr_raw.wgsl`) only when luminance NR is engaged:
    // it targets per-photosite shot noise, which is what the demosaic would
    // otherwise smear into correlated color error. Chroma-only NR is served
    // entirely by the post-demosaic Oklab bilateral.
    if edit.nr.luminance > 0.0
        || edit.nr.camera_denoise_mode != sp_core::edit::CameraDenoiseMode::Off
    {
        passes.push(PassId::RawDenoise);
    }

    // Demosaic choice: Super2x2 uses the superpixel path outright (it IS the
    // downscale). Fast proxy first-frames also use superpixel; that decision
    // is made by the caller passing a Proxy target while the full render is
    // pending — here we only encode the resolution rule.
    match edit.resolution.mode {
        ResolutionMode::Super2x2 => passes.push(PassId::DemosaicSuperpixel),
        ResolutionMode::Full => passes.push(PassId::DemosaicMalvar),
        ResolutionMode::Super3x3 | ResolutionMode::Super4x4 => {
            passes.push(PassId::DemosaicMalvar);
            passes.push(PassId::Supersample);
        }
    }

    passes.push(PassId::WbColor);
    if matches!(edit.profile, sp_core::edit::ProfileChoice::Named(_)) {
        passes.push(PassId::ProfileColorPre);
    }
    passes.push(PassId::ToneColor);
    if matches!(edit.profile, sp_core::edit::ProfileChoice::Named(_)) || edit.look.is_some() {
        passes.push(PassId::ProfileColorPost);
    }

    if edit.lens.enable
        && edit.lens.defringe
        && (edit.lens.defringe_purple > 0.0 || edit.lens.defringe_green > 0.0)
    {
        passes.push(PassId::Defringe);
    }

    if edit.nr.luminance > 0.0
        || edit.nr.chroma > 0.0
        || edit.nr.camera_denoise_mode != sp_core::edit::CameraDenoiseMode::Off
    {
        passes.push(PassId::NoiseReduction);
    }
    if !edit.local.is_neutral() {
        passes.push(PassId::LocalContrast);
    }
    if !edit.spots.is_empty()
        || edit
            .local_adjustments
            .iter()
            .any(|adjustment| adjustment.enabled)
    {
        passes.push(PassId::Retouch);
    }
    if edit.sharpen.amount > 0.0 {
        passes.push(PassId::Sharpen);
    }

    passes.push(PassId::OutputTransform);
    if matches!(
        target,
        RenderTarget::Proxy { .. } | RenderTarget::OneToOne { .. }
    ) {
        passes.push(PassId::Composite);
    }
    passes
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROXY: RenderTarget = RenderTarget::Proxy {
        viewport_w: 1920,
        viewport_h: 1080,
    };

    #[test]
    fn neutral_edit_is_minimal_pipeline() {
        let passes = plan_passes(&EditState::default(), PROXY);
        assert_eq!(
            passes,
            vec![
                PassId::RawPrepare,
                PassId::DemosaicMalvar,
                PassId::WbColor,
                PassId::ToneColor,
                PassId::OutputTransform,
                PassId::Composite,
            ]
        );
    }

    #[test]
    fn named_profile_wraps_tone_with_color_passes() {
        let mut edit = EditState::default();
        edit.profile = sp_core::edit::ProfileChoice::Named("Synthetic DCP".into());
        let passes = plan_passes(&edit, PROXY);
        let pre = passes
            .iter()
            .position(|p| *p == PassId::ProfileColorPre)
            .expect("pre-profile pass");
        let tone = passes
            .iter()
            .position(|p| *p == PassId::ToneColor)
            .expect("tone pass");
        let post = passes
            .iter()
            .position(|p| *p == PassId::ProfileColorPost)
            .expect("post-profile pass");
        assert!(pre < tone && tone < post);
    }

    #[test]
    fn super2x2_replaces_demosaic_no_extra_pass() {
        let mut e = EditState::default();
        e.resolution.mode = ResolutionMode::Super2x2;
        let passes = plan_passes(&e, PROXY);
        assert!(passes.contains(&PassId::DemosaicSuperpixel));
        assert!(!passes.contains(&PassId::DemosaicMalvar));
        assert!(!passes.contains(&PassId::Supersample));
    }

    #[test]
    fn super3x3_is_demosaic_plus_box() {
        let mut e = EditState::default();
        e.resolution.mode = ResolutionMode::Super3x3;
        let passes = plan_passes(&e, PROXY);
        assert!(passes.contains(&PassId::DemosaicMalvar));
        assert!(passes.contains(&PassId::Supersample));
    }

    #[test]
    fn nr_and_sharpen_cull_when_neutral() {
        let mut e = EditState::default();
        assert!(!plan_passes(&e, PROXY).contains(&PassId::NoiseReduction));
        e.nr.chroma = 25.0;
        assert!(plan_passes(&e, PROXY).contains(&PassId::NoiseReduction));
        e.sharpen.amount = 40.0;
        assert!(plan_passes(&e, PROXY).contains(&PassId::Sharpen));
    }

    #[test]
    fn raw_denoise_follows_luminance_nr_only() {
        let mut e = EditState::default();
        assert!(!plan_passes(&e, PROXY).contains(&PassId::RawDenoise));
        e.nr.chroma = 60.0;
        assert!(
            !plan_passes(&e, PROXY).contains(&PassId::RawDenoise),
            "chroma-only NR must not schedule the mosaic-domain pass"
        );
        e.nr.luminance = 10.0;
        let passes = plan_passes(&e, PROXY);
        let denoise = passes
            .iter()
            .position(|p| *p == PassId::RawDenoise)
            .expect("raw denoise pass");
        let demosaic = passes
            .iter()
            .position(|p| *p == PassId::DemosaicMalvar)
            .expect("demosaic pass");
        assert!(denoise < demosaic, "raw NR must precede the demosaic");
    }

    #[test]
    fn camera_denoise_mode_schedules_raw_and_rgb_passes() {
        let mut edit = EditState::default();
        edit.nr.camera_denoise_mode = sp_core::edit::CameraDenoiseMode::PreserveDetail;
        let passes = plan_passes(&edit, PROXY);
        assert!(passes.contains(&PassId::RawDenoise));
        assert!(passes.contains(&PassId::NoiseReduction));
        let raw = passes
            .iter()
            .position(|pass| *pass == PassId::RawDenoise)
            .expect("raw denoise");
        let demosaic = passes
            .iter()
            .position(|pass| *pass == PassId::DemosaicMalvar)
            .expect("demosaic");
        assert!(raw < demosaic);
    }

    #[test]
    fn local_contrast_culls_when_neutral() {
        let mut edit = EditState::default();
        assert!(!plan_passes(&edit, PROXY).contains(&PassId::LocalContrast));
        edit.local.clarity = 1.0;
        let passes = plan_passes(&edit, PROXY);
        let local = passes
            .iter()
            .position(|pass| *pass == PassId::LocalContrast)
            .unwrap();
        let sharpen = {
            edit.sharpen.amount = 1.0;
            plan_passes(&edit, PROXY)
                .iter()
                .position(|pass| *pass == PassId::Sharpen)
                .unwrap()
        };
        assert!(local < sharpen);
    }

    #[test]
    fn defringe_is_selective_and_precedes_local_detail() {
        let mut edit = EditState::default();
        edit.lens.defringe = true;
        edit.lens.defringe_purple = 50.0;
        edit.local.clarity = 10.0;
        edit.sharpen.amount = 20.0;
        let passes = plan_passes(&edit, PROXY);
        let fringe = passes
            .iter()
            .position(|pass| *pass == PassId::Defringe)
            .unwrap();
        let local = passes
            .iter()
            .position(|pass| *pass == PassId::LocalContrast)
            .unwrap();
        let sharpen = passes
            .iter()
            .position(|pass| *pass == PassId::Sharpen)
            .unwrap();
        assert!(fringe < local && local < sharpen);

        edit.lens.enable = false;
        assert!(!plan_passes(&edit, PROXY).contains(&PassId::Defringe));
        edit.lens.enable = true;
        edit.lens.defringe_purple = 0.0;
        assert!(!plan_passes(&edit, PROXY).contains(&PassId::Defringe));
    }

    #[test]
    fn export_has_no_composite() {
        let passes = plan_passes(&EditState::default(), RenderTarget::Export);
        assert!(!passes.contains(&PassId::Composite));
        assert_eq!(*passes.last().expect("non-empty"), PassId::OutputTransform);
    }
}
