//! End-to-end RGB input contracts on a real GPU: no RAW tone curve or CFA
//! processing, same proxy and full export, and retained sub-8-bit precision.

use sp_core::{source::ImageSource, EditState};
use sp_gpu::GpuContext;
use sp_pipeline::GpuRenderer;
use std::sync::atomic::AtomicBool;

#[test]
fn neutral_raw_merge_preparation_is_linear_without_camera_base_curve() {
    let raw = sp_core::RawImage {
        cfa_data: vec![16384; 32 * 32],
        width: 32,
        height: 32,
        cfa: sp_core::CfaPattern::Rggb,
        black_levels: [0.0; 4],
        white_levels: [65535.0; 4],
        wb_coeffs: [1.0; 4],
        cam_to_xyz: sp_core::color::REC2020_TO_XYZ,
        active_area: sp_core::raw::ActiveArea {
            x: 0,
            y: 0,
            width: 32,
            height: 32,
        },
        orientation: sp_core::raw::Orientation::Normal,
    };
    let ctx = match GpuContext::headless() {
        Ok(context) => context,
        Err(sp_core::SpError::NoGpu(error)) => {
            eprintln!("skipping GPU test: {error}");
            return;
        }
        Err(error) => panic!("GPU initialization failed: {error}"),
    };
    let mut renderer = GpuRenderer::new(ctx).unwrap();
    renderer.set_profile(Some(sp_core::profile::CameraProfile::standard(
        "test",
        raw.cam_to_xyz,
    )));
    let mut edit = EditState::default();
    edit.highlight_recovery.enabled = false;
    edit.nr.hot_pixels = false;
    edit.lens.enable = false;
    renderer
        .open_source_with_edit(&ImageSource::Raw(std::sync::Arc::new(raw)), &edit)
        .unwrap();
    let linear = renderer
        .render_scene_linear(&edit, &AtomicBool::new(false), |_| {})
        .unwrap();
    for channel in &linear.linear_rec2020[16 * 32 + 16][..3] {
        assert!(
            (*channel - 0.25).abs() < 0.003,
            "RAW baseline curve was applied: {channel}"
        );
    }
}

#[test]
fn scene_readback_and_float_tiff_keep_hdr_headroom_and_signed_values() {
    let source = sp_core::source::RasterImage {
        width: 8,
        height: 8,
        linear_rec2020: vec![[0.25, 1.5, 4.0, 1.0]; 64],
        bits_per_channel: 32,
        embedded_icc: None,
    };
    let ctx = match GpuContext::headless() {
        Ok(context) => context,
        Err(sp_core::SpError::NoGpu(error)) => {
            eprintln!("skipping GPU test: {error}");
            return;
        }
        Err(error) => panic!("GPU initialization failed: {error}"),
    };
    let mut renderer = GpuRenderer::new(ctx).unwrap();
    let image = ImageSource::Raster(std::sync::Arc::new(source));
    renderer
        .open_source_with_edit(&image, &EditState::default())
        .unwrap();
    let rendered = renderer
        .render_scene_linear(&EditState::default(), &AtomicBool::new(false), |_| {})
        .unwrap();
    for pixel in &rendered.linear_rec2020 {
        for (actual, expected) in pixel[..3].iter().zip([0.25, 1.5, 4.0]) {
            assert!(
                (*actual - expected).abs() < 0.005,
                "headroom changed: {pixel:?}"
            );
        }
    }
    let mut master = rendered;
    master.linear_rec2020[0] = [-0.01, 0.1234567, 4.5, 1.0];
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("master.tif");
    sp_export::write_linear_tiff(&master, &path, "merge recipe", &AtomicBool::new(false)).unwrap();
    let source = sp_raw::decode_source(&path).unwrap();
    let ImageSource::Raster(reopened) = source.source else {
        panic!("raster")
    };
    assert_eq!(reopened.bits_per_channel, 32);
    assert!(reopened.embedded_icc.is_some());
    for c in 0..3 {
        assert!(
            (reopened.linear_rec2020[0][c] - master.linear_rec2020[0][c]).abs() < 0.0005,
            "float master changed: {:?}",
            reopened.linear_rec2020[0]
        );
    }
    assert!(
        sp_export::write_linear_tiff(&master, &path, "replacement", &AtomicBool::new(false))
            .is_err(),
        "must not overwrite an existing source"
    );
}

#[test]
fn raster_proxy_and_sixteen_bit_export_preserve_source_and_apply_edits() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("raster16.tiff");
    let width = 16;
    let height = 8;
    let mut original = Vec::new();
    for _y in 0..height {
        for x in 0..width {
            let value = 20000 + x as u16 * 64;
            original.extend([value, value + 6000, value + 12000]);
        }
    }
    image::ImageBuffer::<image::Rgb<u16>, _>::from_raw(width, height, original.clone())
        .unwrap()
        .save(&path)
        .unwrap();
    let source = sp_raw::decode_source(&path).unwrap();
    assert!(matches!(source.source, ImageSource::Raster(_)));
    let ctx = match GpuContext::headless() {
        Ok(context) => context,
        Err(sp_core::SpError::NoGpu(error)) => {
            eprintln!("skipping GPU test: {error}");
            return;
        }
        Err(error) => panic!("GPU initialization failed: {error}"),
    };
    let mut renderer = GpuRenderer::new(ctx).unwrap();
    let edit = EditState::default();
    renderer
        .open_source_with_edit(&source.source, &edit)
        .unwrap();
    assert_eq!(renderer.active_size(), Some((width, height)));
    renderer.render_without_histogram(&edit);
    let (preview, pw, ph) = renderer.read_display_srgb8().unwrap();
    assert_eq!((pw, ph), (width, height));
    let render = |renderer: &mut GpuRenderer, edit: &EditState| {
        renderer
            .render_export(
                edit,
                false,
                true,
                sp_core::color::OutputColorSpace::Srgb,
                &AtomicBool::new(false),
                |_| {},
            )
            .unwrap()
    };
    let exported = render(&mut renderer, &edit);
    let rgb16 = exported.rgba16.as_ref().unwrap();
    for (pixel, source) in rgb16.chunks_exact(4).zip(original.chunks_exact(3)) {
        for c in 0..3 {
            assert!(
                pixel[c].abs_diff(source[c]) <= 48,
                "neutral raster changed: {} -> {}",
                source[c],
                pixel[c]
            );
        }
    }
    assert!(rgb16[4] > rgb16[0], "sub-8-bit steps must survive export");
    for (a, b) in preview.iter().zip(exported.rgba8.iter()) {
        assert!(a.abs_diff(*b) <= 1, "proxy differs from full RGB export");
    }
    let mut brighter = edit.clone();
    brighter.tone.exposure_ev = 1.0;
    let brighter = render(&mut renderer, &brighter);
    assert!(brighter.rgba8[0] > exported.rgba8[0] + 20);
    // RAW-specific toggles copied from a RAW sidecar cannot change pixels.
    let mut raw_settings = edit.clone();
    raw_settings.high_detail_demosaic = true;
    raw_settings.highlight_recovery.enabled = true;
    raw_settings.nr.camera_denoise_mode = sp_core::edit::CameraDenoiseMode::Cleanest;
    let unchanged = render(&mut renderer, &raw_settings);
    assert_eq!(unchanged.rgba16, exported.rgba16);
    raw_settings.resolution.mode = sp_core::edit::ResolutionMode::Super2x2;
    let smaller = render(&mut renderer, &raw_settings);
    assert_eq!((smaller.width, smaller.height), (width / 2, height / 2));
    assert!(
        smaller.rgba8[0] > 60,
        "raster reduction must not sample the unused CFA binding"
    );
    // Edits and export never mutate the source file.
    assert_eq!(
        image::open(&path).unwrap().into_rgb16().into_raw(),
        original
    );
}
