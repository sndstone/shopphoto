//! Independent numeric checks and cached/fresh/preview/export equivalence.
use sp_core::{
    color::OutputColorSpace,
    edit::{LocalAdjustment, LocalMask, ToneParams},
    source::{ImageSource, RasterImage},
    EditState,
};
use sp_gpu::GpuContext;
use sp_pipeline::GpuRenderer;
use std::sync::{atomic::AtomicBool, Arc};

fn source(gray: bool) -> ImageSource {
    let (width, height) = (128, 96);
    let pixels = (0..height)
        .flat_map(|y| {
            (0..width).map(move |x| {
                let value = 0.01 + ((x * 13 + y * 7) % 113) as f32 / 80.0;
                if gray {
                    [value, value, value, 1.0]
                } else {
                    [value, value * 0.7, value * 0.4, 1.0]
                }
            })
        })
        .collect();
    ImageSource::Raster(Arc::new(RasterImage {
        width,
        height,
        linear_rec2020: pixels,
        bits_per_channel: 16,
        embedded_icc: None,
    }))
}

#[test]
fn preview_prefix_reuses_early_passes_and_invalidates_pixel_dependencies() {
    let ctx = match GpuContext::headless() {
        Ok(context) => context,
        Err(sp_core::SpError::NoGpu(error)) => {
            eprintln!("skipping GPU test: {error}");
            return;
        }
        Err(error) => panic!("GPU initialization failed: {error}"),
    };
    let input = source(false);
    let mut cached = GpuRenderer::new(ctx.clone()).unwrap();
    cached
        .open_source_with_edit(&input, &EditState::default())
        .unwrap();
    let pipes = cached.shared_pipelines();
    let mut edit = EditState::default();
    edit.nr.luminance = 25.0;
    edit.nr.chroma = 15.0;
    for step in 0..9 {
        match step {
            1 => edit.tone.exposure_ev = 0.7,
            2 => edit.local.texture = 20.0,
            3 => edit.sharpen.amount = 60.0,
            4 => edit.wb.temp_kelvin = Some(7200.0),
            5 => edit.nr.luminance = 40.0,
            6 => edit.negative.enabled = true,
            7 => edit.highlight_recovery.enabled = !edit.highlight_recovery.enabled,
            8 => edit.look = Some("Portrait".into()),
            _ => {}
        }
        let before = cached.preview_cache_stats();
        cached.render_without_histogram(&edit);
        let after = cached.preview_cache_stats();
        if (1..=3).contains(&step) {
            assert_eq!(after.0, before.0 + 1, "step {step} must reuse prefix");
        } else {
            assert_eq!(after.1, before.1 + 1, "step {step} must invalidate prefix");
        }
        let mut fresh = GpuRenderer::with_pipelines(ctx.clone(), pipes.clone()).unwrap();
        fresh.set_preview_cache_enabled(false);
        fresh.open_source_with_edit(&input, &edit).unwrap();
        fresh.render_without_histogram(&edit);
        assert_eq!(
            cached.read_display_srgb8().unwrap(),
            fresh.read_display_srgb8().unwrap(),
            "step {step}"
        );
    }
    let before = cached.preview_cache_stats();
    cached.open_source_with_edit(&source(true), &edit).unwrap();
    cached.render_without_histogram(&edit);
    assert_eq!(
        cached.preview_cache_stats().1,
        before.1 + 1,
        "new source invalidates prefix"
    );
}

#[test]
fn multiple_export_tiles_match_large_full_frame_with_spatial_edits() {
    let ctx = match GpuContext::headless() {
        Ok(context) => context,
        Err(sp_core::SpError::NoGpu(error)) => {
            eprintln!("skipping GPU test: {error}");
            return;
        }
        Err(error) => panic!("GPU initialization failed: {error}"),
    };
    let (width, height) = (1371, 1087);
    let input = ImageSource::Raster(Arc::new(RasterImage {
        width,
        height,
        bits_per_channel: 16,
        embedded_icc: None,
        linear_rec2020: (0..width * height)
            .map(|i| {
                let value = 0.02 + ((i * 13 + i / width * 7) % 113) as f32 / 120.0;
                [value, value * 0.7, value * 0.4, 1.0]
            })
            .collect(),
    }));
    let mut renderer = GpuRenderer::new(ctx.clone()).unwrap();
    let mut edit = EditState::default();
    edit.nr.luminance = 20.0;
    edit.nr.chroma = 15.0;
    edit.local.texture = 15.0;
    edit.sharpen.amount = 45.0;
    renderer.open_source_with_edit(&input, &edit).unwrap();
    renderer.render_tile(&edit, 0, 0, width, height).unwrap();
    let full = read_texture(&ctx, renderer.tile_texture().unwrap(), width, height);
    let tiled = renderer
        .render_export(
            &edit,
            false,
            false,
            OutputColorSpace::Srgb,
            &AtomicBool::new(false),
            |_| {},
        )
        .unwrap();
    assert_eq!(tiled.rgba8.len(), full.len());
    assert!(tiled
        .rgba8
        .iter()
        .zip(&full)
        .all(|(a, b)| a.abs_diff(*b) <= 1));
    let sixteen = renderer
        .render_export(
            &edit,
            true,
            true,
            OutputColorSpace::Rec2020,
            &AtomicBool::new(false),
            |_| {},
        )
        .unwrap();
    assert_eq!(sixteen.rgba16.unwrap().len(), (width * height * 4) as usize);
    assert!(renderer
        .render_export(
            &edit,
            false,
            false,
            OutputColorSpace::Srgb,
            &AtomicBool::new(true),
            |_| {}
        )
        .is_err());
}

fn cases() -> Vec<(&'static str, EditState)> {
    let mut cases = vec![("neutral", EditState::default())];
    for (name, values) in [
        ("exposure", vec![("light.exposure", 0.7)]),
        (
            "tone",
            vec![
                ("light.contrast", 35.0),
                ("light.highlights", -45.0),
                ("light.shadows", 28.0),
            ],
        ),
        (
            "endpoints",
            vec![
                ("light.whites", 18.0),
                ("light.blacks", -12.0),
                ("light.brightness", 15.0),
            ],
        ),
        (
            "color",
            vec![("color.vibrance", 32.0), ("color.saturation", -18.0)],
        ),
        (
            "local contrast",
            vec![
                ("effects.texture", 20.0),
                ("effects.clarity", 18.0),
                ("effects.dehaze", 12.0),
            ],
        ),
        (
            "white balance",
            vec![("wb.temperature", 7200.0), ("wb.tint", 15.0)],
        ),
        ("sharpen", vec![("detail.sharpening", 65.0)]),
    ] {
        cases.push((
            name,
            sp_core::commands::EditCommand::Set(sp_core::commands::SetValues {
                values: values.into_iter().map(|(id, v)| (id.into(), v)).collect(),
            })
            .apply(&EditState::default())
            .unwrap(),
        ));
    }
    let mut monochrome = EditState::default();
    monochrome.color.monochrome = true;
    cases.push(("monochrome", monochrome));
    let mut grading = EditState::default();
    grading.color.grading.midtones.hue_degrees = 210.0;
    grading.color.grading.midtones.saturation = 30.0;
    cases.push(("grading", grading));
    let mut mixer = EditState::default();
    mixer.color.hsl[1].hue_shift = 24.0;
    mixer.color.hsl[1].saturation = 18.0;
    cases.push(("HSL", mixer));
    let mut masked = EditState::default();
    masked.local_adjustments.push(LocalAdjustment {
        mask: LocalMask::Radial {
            center: [0.42, 0.57],
            radius: [0.2, 0.3],
            feather: 0.6,
        },
        tone: ToneParams {
            exposure_ev: 0.7,
            ..Default::default()
        },
        ..Default::default()
    });
    cases.push(("radial mask", masked));
    let mut curve = EditState::default();
    curve
        .curve
        .luma
        .points
        .insert(1, sp_core::edit::CurvePoint { x: 0.45, y: 0.57 });
    cases.push(("curve", curve));
    let mut grain = EditState::default();
    grain.grain.amount = 20.0;
    cases.push(("grain", grain));
    let mut vignette = EditState::default();
    vignette.vignette.amount = -25.0;
    cases.push(("vignette", vignette));
    cases
}

fn read_texture(ctx: &GpuContext, texture: &wgpu::Texture, width: u32, height: u32) -> Vec<u8> {
    let stride = (width * 4).div_ceil(256) * 256;
    let buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("window equivalence"),
        size: u64::from(stride) * u64::from(height),
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = ctx.device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(stride),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    ctx.queue.submit([encoder.finish()]);
    ctx.map_read_buffer(&buffer).unwrap();
    let mapped = buffer.slice(..).get_mapped_range();
    let pixels = (0..height)
        .flat_map(|row| {
            mapped[(row * stride) as usize..(row * stride + width * 4) as usize]
                .iter()
                .copied()
        })
        .collect();
    drop(mapped);
    buffer.unmap();
    pixels
}

#[test]
fn zoom_windows_match_the_same_region_of_a_full_export() {
    let Ok(ctx) = GpuContext::headless() else {
        return;
    };
    let mut renderer = GpuRenderer::new(ctx.clone()).unwrap();
    renderer
        .open_source_with_edit(&source(false), &EditState::default())
        .unwrap();
    for (name, edit) in cases() {
        let full = renderer
            .render_export(
                &edit,
                false,
                false,
                OutputColorSpace::Srgb,
                &AtomicBool::new(false),
                |_| {},
            )
            .unwrap();
        let (x, y, w, h) = (21, 17, 47, 39);
        let info = renderer.render_tile(&edit, x, y, w, h).unwrap();
        let tile = read_texture(
            &ctx,
            renderer.tile_texture().unwrap(),
            info.tex_w,
            info.tex_h,
        );
        for row in 0..h {
            for column in 0..w {
                for c in 0..3 {
                    let actual = tile[(((row + info.margin_y) * info.tex_w
                        + column
                        + info.margin_x)
                        * 4
                        + c) as usize];
                    let expected =
                        full.rgba8[(((row + y) * full.width + column + x) * 4 + c) as usize];
                    assert!(
                        actual.abs_diff(expected) <= 2,
                        "{name}: window pixel ({column},{row}) {actual} != {expected}"
                    );
                }
            }
        }
    }
}

#[test]
fn cached_renders_equal_fresh_renders_for_develop_combinations() {
    let Ok(ctx) = GpuContext::headless() else {
        return;
    };
    let source = source(false);
    let mut cached = GpuRenderer::new(ctx.clone()).unwrap();
    let pipes = cached.shared_pipelines();
    cached.set_analysis_enabled(true);
    cached
        .open_source_with_edit(&source, &EditState::default())
        .unwrap();
    for (name, edit) in cases() {
        cached.render_without_histogram(&edit);
        let pixels = cached.read_display_srgb8().unwrap().0;
        let scene = cached.read_analysis_frame().unwrap().unwrap();
        let mut fresh = GpuRenderer::with_pipelines(ctx.clone(), pipes.clone()).unwrap();
        fresh.set_analysis_enabled(true);
        fresh.open_source_with_edit(&source, &edit).unwrap();
        fresh.render_without_histogram(&edit);
        assert_eq!(
            pixels,
            fresh.read_display_srgb8().unwrap().0,
            "cached/fresh SDR: {name}"
        );
        assert_eq!(
            scene.linear_rec2020,
            fresh.read_analysis_frame().unwrap().unwrap().linear_rec2020,
            "cached/fresh scene-linear: {name}"
        );
        // Revisiting neutral must invalidate every stage affected by this case.
        cached.render_without_histogram(&EditState::default());
    }
}

#[test]
fn preview_and_export_agree_for_nonspatial_edits() {
    let Ok(ctx) = GpuContext::headless() else {
        return;
    };
    let mut renderer = GpuRenderer::new(ctx).unwrap();
    let source = source(false);
    renderer
        .open_source_with_edit(&source, &EditState::default())
        .unwrap();
    for (name, edit) in cases()
        .into_iter()
        .filter(|(name, _)| !matches!(*name, "local contrast" | "sharpen"))
    {
        renderer.render_without_histogram(&edit);
        let preview = renderer.read_display_srgb8().unwrap().0;
        let exported = renderer
            .render_export(
                &edit,
                false,
                true,
                OutputColorSpace::Srgb,
                &AtomicBool::new(false),
                |_| {},
            )
            .unwrap();
        let differences: Vec<_> = preview
            .chunks_exact(4)
            .zip(exported.rgba8.chunks_exact(4))
            .flat_map(|(a, b)| (0..3).map(move |c| a[c].abs_diff(b[c]) as f64))
            .collect();
        let max = differences.iter().copied().fold(0.0, f64::max);
        let mean = differences.iter().sum::<f64>() / differences.len() as f64;
        assert!(max <= 3.0 && mean < 0.5, "{name}: max={max}, mean={mean}");
    }
}

#[test]
fn scene_linear_tone_matches_cpu_oracle_and_retains_hdr_headroom() {
    use sp_pipeline::tone::{apply_tone_lut, build_tone_lut};
    let Ok(ctx) = GpuContext::headless() else {
        return;
    };
    let mut renderer = GpuRenderer::new(ctx).unwrap();
    let source = source(true);
    renderer
        .open_source_with_edit(&source, &EditState::default())
        .unwrap();
    let ImageSource::Raster(input) = source else {
        unreachable!()
    };
    let mut hdr_seen = false;
    for (name, edit) in cases()
        .into_iter()
        .filter(|(name, _)| matches!(*name, "neutral" | "exposure" | "tone" | "endpoints"))
    {
        let reference = build_tone_lut(&edit.tone, &edit.curve, None);
        let rendered = renderer
            .render_scene_linear(&edit, &AtomicBool::new(false), |_| {})
            .unwrap();
        for (original, pixel) in input.linear_rec2020.iter().zip(&rendered.linear_rec2020) {
            // The GPU's source texture quantizes once to f16 before the tone pass.
            let quantized = half::f16::from_f32(original[0]).to_f32();
            let expected = apply_tone_lut(&reference, quantized);
            for actual in &pixel[..3] {
                assert!(
                    actual.is_finite() && (*actual - expected).abs() <= 0.004 * expected.max(1.0),
                    "{name}: expected={expected}, actual={actual}"
                );
                hdr_seen |= *actual > 1.0;
            }
        }
    }
    assert!(hdr_seen, "Scene-linear reference was clipped to SDR");
}
