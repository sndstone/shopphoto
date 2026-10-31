//! Corrective photographic operations through the actual GPU pipeline.
use sp_core::{
    color::{mat3_apply, mat3_inverse, REC2020_LUMA, REC2020_TO_REC709},
    correction::{FilmNegative, LinearSourceRgb, LocalCorrection},
    edit::{LocalAdjustment, LocalMask},
    source::{ImageSource, RasterImage},
    EditState,
};
use sp_gpu::GpuContext;
use sp_pipeline::GpuRenderer;
use std::sync::{atomic::AtomicBool, Arc};

fn source(pixels: Vec<[f32; 4]>, width: u32, height: u32) -> ImageSource {
    ImageSource::Raster(Arc::new(RasterImage {
        width,
        height,
        linear_rec2020: pixels,
        bits_per_channel: 16,
        embedded_icc: None,
    }))
}
fn render(renderer: &mut GpuRenderer, edit: &EditState) -> Vec<[f32; 4]> {
    renderer
        .render_scene_linear(edit, &AtomicBool::new(false), |_| {})
        .unwrap()
        .linear_rec2020
}
fn left_mask() -> LocalMask {
    LocalMask::Radial {
        center: [0.25, 0.5],
        radius: [0.2, 0.4],
        feather: 0.0,
    }
}

#[test]
fn film_density_gpu_matches_reference_and_border_pipette_uses_uninverted_source() {
    let Ok(ctx) = GpuContext::headless() else {
        return;
    };
    let mut renderer = GpuRenderer::new(ctx).unwrap();
    let film = FilmNegative {
        enabled: true,
        base: LinearSourceRgb([0.8, 0.5, 0.25]),
        ..Default::default()
    };
    let negative = [0.08, 0.05, 0.025, 1.0];
    renderer
        .open_source_with_edit(
            &source(vec![negative; 64 * 64], 64, 64),
            &EditState::default(),
        )
        .unwrap();
    let edit = EditState {
        negative: film,
        ..Default::default()
    };
    let output = render(&mut renderer, &edit);
    let expected = film.apply(LinearSourceRgb([negative[0], negative[1], negative[2]]));
    for (actual, expected) in output[32 * 64 + 32].iter().zip(expected.0) {
        assert!((actual - expected).abs() < 0.003);
    }
    renderer.render(&edit);
    let base = renderer.sample_film_base([0.5, 0.5]).unwrap();
    for (actual, expected) in base.0.iter().zip(negative) {
        assert!(
            (actual - expected).abs() < 0.0002,
            "sample must precede inversion"
        );
    }
    let displayed = renderer.sample_display_rgb(0.5, 0.5).unwrap();
    let exported = renderer
        .render_export(
            &edit,
            false,
            true,
            sp_core::color::OutputColorSpace::Srgb,
            &AtomicBool::new(false),
            |_| {},
        )
        .unwrap();
    for (c, value) in displayed.iter().enumerate() {
        assert!((i16::from(*value) - i16::from(exported.rgba8[(32 * 64 + 32) * 4 + c])).abs() <= 1);
    }
}

#[test]
fn red_eye_removes_only_selected_red_reflection_and_keeps_catchlights() {
    let Ok(ctx) = GpuContext::headless() else {
        return;
    };
    let mut renderer = GpuRenderer::new(ctx).unwrap();
    let red = mat3_apply(
        &mat3_inverse(&REC2020_TO_REC709).unwrap(),
        [0.7, 0.025, 0.03],
    );
    let mut pixels = vec![[red[0], red[1], red[2], 1.0]; 64 * 64];
    pixels[32 * 64 + 16] = [0.8, 0.8, 0.8, 1.0];
    renderer
        .open_source_with_edit(&source(pixels, 64, 64), &EditState::default())
        .unwrap();
    let neutral = render(&mut renderer, &EditState::default());
    let edit = EditState {
        local_adjustments: vec![LocalAdjustment {
            mask: left_mask(),
            corrective: LocalCorrection {
                red_eye: 100.0,
                ..Default::default()
            },
            ..Default::default()
        }],
        ..Default::default()
    };
    let corrected = render(&mut renderer, &edit);
    let pupil = corrected[30 * 64 + 16];
    let pupil709 = mat3_apply(&REC2020_TO_REC709, [pupil[0], pupil[1], pupil[2]]);
    assert!(pupil709[0] < 0.04, "{pupil709:?}");
    for c in 0..3 {
        assert!((corrected[32 * 64 + 16][c] - neutral[32 * 64 + 16][c]).abs() < 0.002);
    }
    assert_eq!(corrected[32 * 64 + 48], neutral[32 * 64 + 48]);
}

#[test]
fn moire_reduces_false_chroma_while_preserving_luminance_detail_and_mask_boundary() {
    let Ok(ctx) = GpuContext::headless() else {
        return;
    };
    let mut renderer = GpuRenderer::new(ctx).unwrap();
    let pixels = (0..64 * 64)
        .map(|i| {
            let delta = if (i % 64 + i / 64) % 2 == 0 {
                0.12
            } else {
                -0.12
            };
            let y = if i / 64 < 32 { 0.2 } else { 0.4 };
            [
                y + delta,
                y - delta * REC2020_LUMA[0] / REC2020_LUMA[1],
                y,
                1.0,
            ]
        })
        .collect();
    renderer
        .open_source_with_edit(&source(pixels, 64, 64), &EditState::default())
        .unwrap();
    let neutral = render(&mut renderer, &EditState::default());
    let edit = EditState {
        local_adjustments: vec![LocalAdjustment {
            mask: left_mask(),
            corrective: LocalCorrection {
                moire: 100.0,
                moire_radius_px: 4.0,
                ..Default::default()
            },
            ..Default::default()
        }],
        ..Default::default()
    };
    let output = render(&mut renderer, &edit);
    let luma = |p: [f32; 4]| (0..3).map(|c| p[c] * REC2020_LUMA[c]).sum::<f32>();
    for y in [24, 31, 32, 40] {
        let i = y * 64 + 16;
        assert!((luma(output[i]) - luma(neutral[i])).abs() < 0.001);
        assert!((output[i][0] - output[i][1]).abs() < (neutral[i][0] - neutral[i][1]).abs() * 0.6);
        assert_eq!(output[y * 64 + 48], neutral[y * 64 + 48]);
    }
}

#[test]
fn photographic_color_controls_match_cpu_in_global_and_masked_gpu_paths() {
    use sp_core::{
        color::LinearRec2020,
        correction::{PhotographicColor, SkinColor},
    };
    let Ok(ctx) = GpuContext::headless() else {
        return;
    };
    let mut renderer = GpuRenderer::new(ctx).unwrap();
    let input = [0.42, 0.23, 0.14];
    renderer
        .open_source_with_edit(
            &source(vec![[input[0], input[1], input[2], 1.0]; 64 * 64], 64, 64),
            &EditState::default(),
        )
        .unwrap();
    let mut selective = PhotographicColor::default();
    selective.selective[0] = [30.0, -10.0, 15.0, -5.0];
    let settings = [
        PhotographicColor {
            mixer: [[0.8, 0.2, 0.0], [0.0, 1.0, 0.0], [0.25, 0.0, 0.75]],
            ..Default::default()
        },
        selective,
        PhotographicColor {
            skin: SkinColor {
                hue_uniformity: 70.0,
                chroma_uniformity: 45.0,
                range_deg: 70.0,
                ..Default::default()
            },
            ..Default::default()
        },
    ];
    let neutral = render(&mut renderer, &EditState::default());
    for params in settings {
        let mut global = EditState::default();
        global.color.photographic = params;
        let global_output = render(&mut renderer, &global);
        let expected = params.apply(LinearRec2020(input));
        for (actual, expected) in global_output[32 * 64 + 16].iter().zip(expected.0) {
            assert!(
                (actual - expected).abs() < 0.003,
                "actual={actual} expected={expected}"
            );
        }
        let mut local = EditState::default();
        local.local_adjustments.push(LocalAdjustment {
            mask: left_mask(),
            color: global.color,
            ..Default::default()
        });
        let local_output = render(&mut renderer, &local);
        for (actual, expected) in local_output[32 * 64 + 16]
            .iter()
            .zip(global_output[32 * 64 + 16])
        {
            assert!((actual - expected).abs() < 0.003);
        }
        assert_eq!(local_output[32 * 64 + 48], neutral[32 * 64 + 48]);
        renderer.render(&global);
        let preview = renderer.sample_display_rgb(0.5, 0.5).unwrap();
        let export = renderer
            .render_export(
                &global,
                false,
                true,
                sp_core::color::OutputColorSpace::Srgb,
                &AtomicBool::new(false),
                |_| {},
            )
            .unwrap();
        for (actual, expected) in preview
            .into_iter()
            .zip(export.rgba8[(32 * 64 + 32) * 4..].iter())
        {
            assert!((i16::from(actual) - i16::from(*expected)).abs() <= 1);
        }
    }
}

#[test]
fn scene_analysis_is_bounded_retains_hdr_and_does_not_change_export() {
    let Ok(ctx) = GpuContext::headless() else {
        return;
    };
    let mut renderer = GpuRenderer::new(ctx).unwrap();
    renderer
        .open_source_with_edit(
            &source(vec![[0.18, 2.0, 4.0, 1.0]; 1536 * 768], 1536, 768),
            &EditState::default(),
        )
        .unwrap();
    let mut edit = EditState::default();
    edit.tone.exposure_ev = -1.0;
    renderer.render(&edit);
    assert!(renderer.read_analysis_frame().unwrap().is_none());
    let baseline = renderer.sample_display_rgb(0.5, 0.5).unwrap();
    renderer.set_analysis_enabled(true);
    renderer.render(&edit);
    let frame = renderer.read_analysis_frame().unwrap().unwrap();
    assert_eq!((frame.width, frame.height), (512, 256));
    let rgb = sp_core::analysis::sample(&frame, [0.5, 0.5]).unwrap().rgb.0;
    for (value, expected) in rgb.into_iter().zip([0.09, 1.0, 2.0]) {
        assert!((value - expected).abs() < 0.006, "{rgb:?}");
    }
    assert_eq!(baseline, renderer.sample_display_rgb(0.5, 0.5).unwrap());
    renderer.set_analysis_enabled(false);
    renderer.render(&edit);
    assert!(renderer.read_analysis_frame().unwrap().is_none());
}

#[test]
fn texture_healing_preserves_target_illumination_and_transfers_source_detail() {
    let Ok(ctx) = GpuContext::headless() else {
        return;
    };
    let mut renderer = GpuRenderer::new(ctx).unwrap();
    let pixels = (0..96 * 64)
        .map(|i| {
            let x = i % 96;
            let y = i / 96;
            let texture = if x < 48 {
                if (x + y) % 2 == 0 {
                    0.035
                } else {
                    -0.035
                }
            } else {
                0.0
            };
            let base = 0.25 + x as f32 * 0.003;
            let value = base + texture;
            [value, value, value, 1.0]
        })
        .collect();
    renderer
        .open_source_with_edit(&source(pixels, 96, 64), &EditState::default())
        .unwrap();
    let edit = EditState {
        spots: vec![sp_core::edit::SpotCorrection {
            target: [0.75, 0.5],
            source: [0.25, 0.5],
            radius: 0.18,
            feather: 0.0,
            heal_texture: 100.0,
            ..Default::default()
        }],
        ..Default::default()
    };
    let output = render(&mut renderer, &edit);
    let a = output[32 * 96 + 72][0];
    let b = output[32 * 96 + 73][0];
    assert!(
        (a - b).abs() > 0.035,
        "source texture should survive, {a} {b}"
    );
    assert!(
        ((a + b) * 0.5 - (0.25 + 72.5 * 0.003)).abs() < 0.018,
        "target brightness must stay local"
    );
    renderer.render(&edit);
    let before = renderer.sample_display_rgb(0.75, 0.5).unwrap();
    let frame = renderer.retouch_analysis_frame(&edit).unwrap();
    assert!(frame.width <= 1024 && frame.height <= 1024);
    assert!(
        (frame.linear_rec2020[32 * 96 + 72][0] - (0.25 + 72.0 * 0.003)).abs() < 0.002,
        "analysis must exclude spot corrections"
    );
    assert_eq!(
        before,
        renderer.sample_display_rgb(0.75, 0.5).unwrap(),
        "analysis restores displayed edit"
    );
}

#[test]
fn many_full_resolution_alpha_masks_use_bounded_gpu_payloads_and_exact_coverage() {
    use sp_core::raw::{ActiveArea, CfaPattern, Orientation};
    let Ok(ctx) = GpuContext::headless() else {
        return;
    };
    // Opt-in runs the same executable assertions on a real 100 MP allocation.
    let edge = if std::env::var_os("SP_LARGE_MASK_STRESS").is_some() {
        10000
    } else {
        1024
    };
    let raw = sp_core::RawImage {
        width: edge,
        height: edge,
        cfa_data: vec![13107; (edge * edge) as usize],
        cfa: CfaPattern::Rggb,
        black_levels: [0.0; 4],
        white_levels: [65535.0; 4],
        wb_coeffs: [1.0; 4],
        cam_to_xyz: sp_core::color::REC2020_TO_XYZ,
        active_area: ActiveArea {
            x: 0,
            y: 0,
            width: edge,
            height: edge,
        },
        orientation: Orientation::Normal,
    };
    let mut renderer = GpuRenderer::new(ctx).unwrap();
    renderer.set_proxy_long_edge(512);
    renderer.open_image(&raw).unwrap();
    drop(raw);
    let mask = sp_core::mask::AlphaMask {
        width: edge,
        height: edge,
        alpha: vec![32768; (edge * edge) as usize].into(),
        source: "stress".into(),
        inverted: false,
        repairs: vec![],
    };
    let mut adjustment = LocalAdjustment {
        mask: LocalMask::Alpha(mask),
        ..Default::default()
    };
    adjustment.tone.exposure_ev = 0.01;
    let edit = EditState {
        local_adjustments: vec![adjustment; 40],
        ..Default::default()
    };
    assert_eq!(
        sp_core::mask::alpha_storage_bytes(&edit),
        u64::from(edge) * u64::from(edge) * 2,
        "duplicate/history references share source alpha storage"
    );
    renderer.set_analysis_enabled(true);
    renderer.render(&EditState::default());
    let base = renderer
        .read_analysis_frame()
        .unwrap()
        .unwrap()
        .linear_rec2020[256 * 512 + 256][0];
    renderer.set_mask_preview(Some(39));
    renderer.render(&edit);
    let coverage = renderer.read_mask_preview().unwrap().unwrap();
    assert!(coverage.alpha.iter().all(|v| v.abs_diff(32768) <= 32));
    let frame = renderer.read_analysis_frame().unwrap().unwrap();
    let expected = base * 2.0f32.powf(40.0 * 0.01 * 32768.0 / 65535.0);
    for y in [0, 100, 101, 102, 203, 204, 511] {
        let pixel = frame.linear_rec2020[y * 512 + 256];
        assert!(
            (pixel[0] - expected).abs() < 0.001,
            "stripe seam or lost mask at {y}: {pixel:?}, expected {expected}"
        );
    }
    let proxy = renderer.sample_display_rgb(0.5, 0.5).unwrap();
    renderer
        .render_tile(&edit, edge / 2 - 64, edge / 2 - 64, 128, 128)
        .unwrap();
    assert!(
        renderer.estimated_texture_bytes()
            < u64::from(edge) * u64::from(edge) * 4 + 30 * 1024 * 1024,
        "tracked resident textures must stay bounded"
    );
    assert_eq!(proxy, renderer.sample_display_rgb(0.5, 0.5).unwrap());
}

#[test]
fn guided_mask_bake_uses_exact_coverage_and_preserves_display() {
    let Ok(ctx) = GpuContext::headless() else {
        return;
    };
    let mut renderer = GpuRenderer::new(ctx).unwrap();
    renderer
        .open_source_with_edit(
            &source(vec![[0.2, 0.3, 0.1, 1.0]; 64 * 64], 64, 64),
            &EditState::default(),
        )
        .unwrap();
    let mut local = LocalAdjustment {
        mask: left_mask(),
        mask_inverted: true,
        opacity: 0.6,
        ..Default::default()
    };
    local.tone.exposure_ev = 0.5;
    let edit = EditState {
        local_adjustments: vec![local],
        ..Default::default()
    };
    renderer.render(&edit);
    let before = renderer.sample_display_rgb(0.5, 0.5).unwrap();
    let (mask, guide) = renderer.mask_refinement_input(&edit, 0).unwrap();
    assert!(mask.sample([0.25, 0.5]) < 0.01);
    assert!((mask.sample([0.8, 0.5]) - 0.6).abs() < 0.002);
    assert!(
        (guide.linear_rec2020[32 * 64 + 50][0] - 0.2).abs() < 0.001,
        "guidance precedes this adjustment"
    );
    assert_eq!(before, renderer.sample_display_rgb(0.5, 0.5).unwrap());
    let refined = sp_core::mask::refine_guided(&mask, &guide, 4.0, 0.1).unwrap();
    let mut baked = edit.clone();
    baked.local_adjustments[0].mask = LocalMask::Alpha(refined);
    baked.local_adjustments[0].mask_inverted = false;
    baked.local_adjustments[0].opacity = 1.0;
    let output = render(&mut renderer, &baked);
    assert!((output[32 * 64 + 50][0] - 0.2 * 2.0f32.powf(0.3)).abs() < 0.002);
}

#[test]
fn small_blur_restoration_matches_cpu_preserves_hdr_and_proxy_export() {
    let ctx = match GpuContext::headless() {
        Ok(context) => context,
        Err(sp_core::SpError::NoGpu(error)) => {
            eprintln!("skipping GPU test: {error}");
            return;
        }
        Err(error) => panic!("GPU initialization failed: {error}"),
    };
    let mut renderer = GpuRenderer::new(ctx).unwrap();
    let value = |x: i32, y: i32| 1.4 + 0.3 * (x as f32 * 0.8).sin() + 0.1 * (y as f32 * 0.5).cos();
    let pixels = (0..64 * 64)
        .map(|i| {
            let v = value(i % 64, i / 64);
            [v, v, v, 1.0]
        })
        .collect();
    renderer
        .open_source_with_edit(&source(pixels, 64, 64), &EditState::default())
        .unwrap();
    let neutral = render(&mut renderer, &EditState::default());
    let mut edit = EditState::default();
    edit.sharpen.restore_blur = 80.0;
    edit.sharpen.restore_regularization = 20.0;
    let output = render(&mut renderer, &edit);
    for y in 5..59 {
        for x in 5..59 {
            let expected = sp_core::restoration::restore_luma(x, y, &edit.sharpen, |sx, sy| {
                neutral[(sy * 64 + sx) as usize][0]
            });
            assert!(
                (output[(y * 64 + x) as usize][0] - expected).abs() < 0.003,
                "{x},{y}: {} vs {expected}",
                output[(y * 64 + x) as usize][0]
            );
        }
    }
    assert!(output.iter().any(|p| p[0] > 1.0));
    renderer.render(&edit);
    let preview = renderer.sample_display_rgb(0.5, 0.5).unwrap();
    let export = renderer
        .render_export(
            &edit,
            false,
            true,
            sp_core::color::OutputColorSpace::Srgb,
            &AtomicBool::new(false),
            |_| {},
        )
        .unwrap();
    for (c, value) in preview.iter().enumerate() {
        assert!((i16::from(*value) - i16::from(export.rgba8[(32 * 64 + 32) * 4 + c])).abs() <= 1);
    }
}
