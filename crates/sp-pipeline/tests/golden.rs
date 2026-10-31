//! Golden pipeline test (BUILD.md Phase 2 step 6): render a synthetic CFA
//! through the REAL GPU pipeline (superpixel → downsample → wb_color →
//! output transform) and compare the readback against a pure-CPU reference.
//! Skips cleanly on machines with no GPU adapter.

use sp_core::color::{mat3_apply, srgb_oetf, REC2020_TO_REC709, REC2020_TO_XYZ};
use sp_core::profile::{CameraProfile, HueSatLut, Illuminant, IlluminantMatrix};
use sp_core::raw::{ActiveArea, CfaPattern, Orientation};
use sp_core::{EditState, RawImage};
use sp_gpu::GpuContext;
use sp_pipeline::GpuRenderer;

/// Build a synthetic RGGB frame where every 2×2 quad encodes the same
/// camera-RGB color, normalized against black=0 / white=65535.
fn synthetic_raw(w: u32, h: u32, rgb: [f32; 3]) -> RawImage {
    let mut cfa = vec![0u16; (w * h) as usize];
    for y in 0..h {
        for x in 0..w {
            let c = CfaPattern::Rggb.color_at(x, y) as usize;
            cfa[(y * w + x) as usize] = (rgb[c].clamp(0.0, 1.0) * 65535.0) as u16;
        }
    }
    RawImage {
        cfa_data: cfa,
        width: w,
        height: h,
        cfa: CfaPattern::Rggb,
        black_levels: [0.0; 4],
        white_levels: [65535.0; 4],
        wb_coeffs: [1.0, 1.0, 1.0, 1.0],
        // Camera space IS Rec.2020: cam→XYZ = REC2020_TO_XYZ, so the
        // pipeline's cam→Rec2020 matrix is identity.
        cam_to_xyz: REC2020_TO_XYZ,
        active_area: ActiveArea {
            x: 0,
            y: 0,
            width: w,
            height: h,
        },
        orientation: Orientation::Normal,
    }
}

/// CPU reference for the Phase 2 pipeline with a neutral edit.
fn expected_rgb8(cam_rgb: [f32; 3]) -> [u8; 3] {
    expected_rgb8_with(cam_rgb, 0.0)
}

/// CPU mirror of the neutral pipeline: identity cam→Rec2020, WB [1,1,1],
/// the Standard base curve via the same tone-LUT builder the GPU uses,
/// then Rec.709 + sRGB encoding.
fn expected_rgb8_with(cam_rgb: [f32; 3], exposure_ev: f32) -> [u8; 3] {
    use sp_core::edit::{CurveParams, ToneParams};
    use sp_pipeline::tone::{apply_tone_lut, build_tone_lut, standard_base_curve};
    let params = ToneParams {
        exposure_ev,
        ..Default::default()
    };
    let base = standard_base_curve();
    let lut = build_tone_lut(&params, &CurveParams::default(), Some(&base));
    let toned = cam_rgb.map(|v| apply_tone_lut(&lut, v));
    let rec709 = mat3_apply(&REC2020_TO_REC709, toned);
    let mut out = [0u8; 3];
    for (o, v) in out.iter_mut().zip(rec709) {
        *o = (srgb_oetf(v.clamp(0.0, 1.0)) * 255.0).round() as u8;
    }
    out
}

fn readback_center_of(ctx: &GpuContext, tex: &wgpu::Texture, w: u32, h: u32) -> [u8; 4] {
    readback_pixel_of(ctx, tex, w, h, w / 2, h / 2)
}

fn readback_pixel_of(
    ctx: &GpuContext,
    tex: &wgpu::Texture,
    w: u32,
    h: u32,
    x: u32,
    y: u32,
) -> [u8; 4] {
    let bytes_per_row = (w * 4).div_ceil(256) * 256;
    let buf = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: (bytes_per_row * h) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = ctx.device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: tex,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buf,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: Some(h),
            },
        },
        wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
    );
    ctx.queue.submit([encoder.finish()]);

    let slice = buf.slice(..);
    slice.map_async(wgpu::MapMode::Read, |r| r.expect("map"));
    ctx.device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll");
    let data = slice.get_mapped_range();
    let off = (y * bytes_per_row + x * 4) as usize;
    [data[off], data[off + 1], data[off + 2], data[off + 3]]
}

fn readback_rgba(ctx: &GpuContext, tex: &wgpu::Texture, w: u32, h: u32) -> Vec<u8> {
    let bytes_per_row = (w * 4).div_ceil(256) * 256;
    let buf = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback image"),
        size: (bytes_per_row * h) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = ctx.device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: tex,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buf,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: Some(h),
            },
        },
        wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
    );
    ctx.queue.submit([encoder.finish()]);

    let slice = buf.slice(..);
    slice.map_async(wgpu::MapMode::Read, |r| r.expect("map"));
    ctx.device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll");
    let data = slice.get_mapped_range();
    let mut rgba = vec![0; (w * h * 4) as usize];
    for y in 0..h as usize {
        let source = &data[y * bytes_per_row as usize..y * bytes_per_row as usize + w as usize * 4];
        rgba[y * w as usize * 4..(y + 1) * w as usize * 4].copy_from_slice(source);
    }
    rgba
}

fn synthetic_gradient_raw(w: u32, h: u32) -> RawImage {
    let mut raw = synthetic_raw(w, h, [0.0; 3]);
    for y in 0..h {
        for x in 0..w {
            let channel = CfaPattern::Rggb.color_at(x, y) as usize;
            let ramp = 0.08 + 0.72 * x as f32 / (w - 1) as f32;
            let color_scale = [0.9, 1.0, 0.8][channel];
            raw.cfa_data[(y * w + x) as usize] = (ramp * color_scale * 65535.0) as u16;
        }
    }
    raw
}

fn synthetic_split_color_raw(w: u32, h: u32) -> RawImage {
    let mut raw = synthetic_raw(w, h, [0.0; 3]);
    for y in 0..h {
        for x in 0..w {
            let rgb = if x < w / 2 {
                [0.36, 0.11, 0.08]
            } else {
                [0.08, 0.34, 0.12]
            };
            let channel = CfaPattern::Rggb.color_at(x, y) as usize;
            raw.cfa_data[(y * w + x) as usize] = (rgb[channel] * 65_535.0) as u16;
        }
    }
    raw
}

fn display_oklab_ab(rgb: [u8; 3]) -> [f32; 2] {
    let [r, g, b] = rgb.map(|value| sp_core::color::srgb_eotf(f32::from(value) / 255.0));
    let l = (0.412_221_46 * r + 0.536_332_55 * g + 0.051_445_995 * b).cbrt();
    let m = (0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b).cbrt();
    let s = (0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b).cbrt();
    [
        1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
        0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
    ]
}

/// Neutral fine-line target. Since the ground truth is achromatic, any RGB
/// channel separation after demosaic is a false-color artifact. Four-pixel
/// bars are intentionally close to the Bayer sampling limit, while the sparse
/// horizontal crossings prevent this from being a one-dimensional special
/// case.
fn synthetic_fine_grid_raw(w: u32, h: u32) -> RawImage {
    let mut raw = synthetic_raw(w, h, [0.0; 3]);
    for y in 0..h {
        for x in 0..w {
            let vertical = (x / 4) % 2 == 0;
            let horizontal = y % 16 < 2;
            let value = if vertical ^ horizontal { 0.72 } else { 0.12 };
            raw.cfa_data[(y * w + x) as usize] = (value * 65_535.0) as u16;
        }
    }
    raw
}

fn mean_false_chroma(rgba: &[u8], width: u32, info: sp_pipeline::renderer::TileInfo) -> f32 {
    let mut total = 0_u64;
    let mut count = 0_u64;
    // Ignore two pixels around the visible boundary: neither algorithm can
    // reconstruct samples outside the requested comparison region.
    for y in info.margin_y + 2..info.margin_y + info.h - 2 {
        for x in info.margin_x + 2..info.margin_x + info.w - 2 {
            let offset = ((y * width + x) * 4) as usize;
            let r = i32::from(rgba[offset]);
            let g = i32::from(rgba[offset + 1]);
            let b = i32::from(rgba[offset + 2]);
            total += (r - g).unsigned_abs() as u64 + (b - g).unsigned_abs() as u64;
            count += 2;
        }
    }
    total as f32 / count as f32
}

/// A neutral scene under unequal as-shot WB gains with one two-channel-clipped
/// highlight at the centre. Treating equal *camera* channels as neutral turns
/// this pixel magenta after WB; true neutralization must happen in the linear
/// working space instead.
fn partially_clipped_neutral_highlight_raw(w: u32, h: u32) -> RawImage {
    let mut raw = synthetic_raw(w, h, [0.40, 0.80, 0.533_333_36]);
    raw.wb_coeffs = [2.0, 1.0, 1.5, 1.0];
    let qx = (w / 2) & !1;
    let qy = (h / 2) & !1;
    for y in qy..qy + 2 {
        for x in qx..qx + 2 {
            let channel = CfaPattern::Rggb.color_at(x, y) as usize;
            let value = [1.0, 0.80, 1.0][channel];
            raw.cfa_data[(y * w + x) as usize] = (value * 65_535.0) as u16;
        }
    }
    raw
}

fn readback_center_pixel(ctx: &GpuContext, renderer: &GpuRenderer) -> [u8; 4] {
    let (tex, w, h) = renderer.output().expect("image open");
    readback_center_of(ctx, tex, w, h)
}

#[test]
fn malvar_tile_matches_reference_on_flat_color() {
    // On a spatially constant scene every demosaic must reproduce the
    // constant exactly — this validates the whole 1:1 tile path
    // (raw_prepare → Malvar → wb/tone/output) including CFA parity shifts
    // from odd tile origins.
    let Ok(ctx) = GpuContext::headless() else {
        return;
    };
    let mut renderer = GpuRenderer::new(ctx.clone()).expect("renderer");
    let cam_rgb = [0.3, 0.42, 0.24];
    let raw = synthetic_raw(96, 96, cam_rgb);
    renderer.open_image(&raw).expect("open");
    let want = expected_rgb8(cam_rgb);

    // Odd origin exercises the shifted-CFA logic.
    for (x, y) in [(8, 8), (17, 9), (3, 21)] {
        let info = renderer
            .render_tile(&EditState::default(), x, y, 48, 40)
            .expect("tile");
        assert_eq!((info.x, info.y, info.w, info.h), (x, y, 48, 40));
        // Center pixel is well inside the visible region regardless of margins.
        let tile_tex = tile_out_texture(&renderer);
        let got = readback_center_of(&ctx, tile_tex, info.tex_w, info.tex_h);
        for ch in 0..3 {
            let diff = (i32::from(got[ch]) - i32::from(want[ch])).abs();
            assert!(
                diff <= 2,
                "tile at ({x},{y}) channel {ch}: got {} want {}",
                got[ch],
                want[ch]
            );
        }
    }
}

#[test]
fn ahd_reduces_false_color_on_a_fine_grid() {
    let Ok(ctx) = GpuContext::headless() else {
        return;
    };
    let mut renderer = GpuRenderer::new(ctx.clone()).expect("renderer");
    renderer
        .open_image(&synthetic_fine_grid_raw(160, 128))
        .expect("open");

    let mut edit = EditState::default();
    let malvar_info = renderer.render_tile(&edit, 16, 16, 128, 96).unwrap();
    let malvar = readback_rgba(
        &ctx,
        tile_out_texture(&renderer),
        malvar_info.tex_w,
        malvar_info.tex_h,
    );
    let malvar_chroma = mean_false_chroma(&malvar, malvar_info.tex_w, malvar_info);

    edit.high_detail_demosaic = true;
    let ahd_info = renderer.render_tile(&edit, 16, 16, 128, 96).unwrap();
    let ahd = readback_rgba(
        &ctx,
        tile_out_texture(&renderer),
        ahd_info.tex_w,
        ahd_info.tex_h,
    );
    let ahd_chroma = mean_false_chroma(&ahd, ahd_info.tex_w, ahd_info);

    assert!(
        ahd_chroma < malvar_chroma * 0.85,
        "AHD false chroma {ahd_chroma:.3} was not at least 15% below Malvar {malvar_chroma:.3}"
    );
}

/// Test-only access to the tile texture (renderer exposes only the view).
fn tile_out_texture(renderer: &GpuRenderer) -> &wgpu::Texture {
    renderer.tile_texture().expect("tile rendered")
}

#[test]
fn synthetic_colors_survive_the_pipeline() {
    let Ok(ctx) = GpuContext::headless() else {
        eprintln!("no GPU adapter — skipping golden pipeline test");
        return;
    };
    let mut renderer = GpuRenderer::new(ctx.clone()).expect("renderer");

    // Distinct chroma + a neutral gray; values inside Rec.709 gamut so the
    // 2020→709 conversion stays un-clamped and exactly comparable.
    for cam_rgb in [[0.4, 0.4, 0.4], [0.35, 0.45, 0.30], [0.10, 0.20, 0.40]] {
        let raw = synthetic_raw(64, 64, cam_rgb);
        renderer.open_image(&raw).expect("open");
        renderer.render(&EditState::default());

        let got = readback_center_pixel(&ctx, &renderer);
        let want = expected_rgb8(cam_rgb);
        for ch in 0..3 {
            let diff = (i32::from(got[ch]) - i32::from(want[ch])).abs();
            assert!(
                diff <= 2,
                "cam {cam_rgb:?} channel {ch}: got {} want {} (±2)",
                got[ch],
                want[ch]
            );
        }
        assert_eq!(got[3], 255);
    }
}

#[test]
fn partially_clipped_neutral_highlight_stays_white_after_wb() {
    let Ok(ctx) = GpuContext::headless() else {
        return;
    };
    let mut renderer = GpuRenderer::new(ctx.clone()).expect("renderer");
    renderer
        .open_image(&partially_clipped_neutral_highlight_raw(128, 128))
        .expect("open");
    renderer.set_view_options(sp_core::edit::ProofSpace::Rec2020, false, 0);
    let mut edit = EditState::default();
    // Pull the reconstructed highlight below display clipping so any false
    // colour remains measurable in the 8-bit output.
    edit.tone.exposure_ev = -2.0;
    renderer.render(&edit);
    ctx.device
        .poll(wgpu::PollType::wait_indefinitely())
        .unwrap();
    let assert_white = |path: &str, pixel: [u8; 4]| {
        let (low, high) = pixel[..3]
            .iter()
            .fold((u8::MAX, u8::MIN), |(low, high), value| {
                (low.min(*value), high.max(*value))
            });
        assert!(
            high - low <= 3,
            "{path} clipped neutral highlight developed a colour rim: {pixel:?}"
        );
    };
    assert_white("proxy", readback_center_pixel(&ctx, &renderer));

    let rgba_center = |rgba: &[u8], width: u32, height: u32| -> [u8; 4] {
        let offset = ((height / 2 * width + width / 2) * 4) as usize;
        rgba[offset..offset + 4].try_into().unwrap()
    };
    renderer.set_hdr_display(true);
    let mut hdr_edit = edit.clone();
    hdr_edit.hdr.enabled = true;
    hdr_edit.hdr.headroom_stops = 4.0;
    renderer.render(&hdr_edit);
    ctx.device
        .poll(wgpu::PollType::wait_indefinitely())
        .unwrap();
    let (hdr_proxy, hdr_w, hdr_h) = renderer.read_scrgb_display_srgb8(false).unwrap();
    assert_white("EDR proxy", rgba_center(&hdr_proxy, hdr_w, hdr_h));

    let tile = renderer.render_tile(&hdr_edit, 0, 0, 128, 128).unwrap();
    ctx.device
        .poll(wgpu::PollType::wait_indefinitely())
        .unwrap();
    let (hdr_tile, tile_w, tile_h) = renderer.read_scrgb_display_srgb8(true).unwrap();
    assert_eq!((tile_w, tile_h), (tile.tex_w, tile.tex_h));
    assert_white("EDR 1:1 tile", rgba_center(&hdr_tile, tile_w, tile_h));

    let mut export_edit = hdr_edit;
    export_edit.resolution.mode = sp_core::edit::ResolutionMode::Super2x2;
    for (label, hdr_pq) in [("SDR export", false), ("HDR PQ export", true)] {
        let exported = renderer
            .render_export(
                &export_edit,
                hdr_pq,
                false,
                sp_core::color::OutputColorSpace::Rec2020,
                &std::sync::atomic::AtomicBool::new(false),
                |_| {},
            )
            .unwrap();
        assert_white(
            label,
            rgba_center(&exported.rgba8, exported.width, exported.height),
        );
    }
}

#[test]
fn adobe_rgb_export_uses_the_adobe_gamma_and_keeps_grey_neutral() {
    let Ok(ctx) = GpuContext::headless() else {
        return;
    };
    let mut renderer = GpuRenderer::new(ctx.clone()).unwrap();
    renderer
        .open_image(&synthetic_raw(64, 64, [0.18; 3]))
        .unwrap();
    let edit = EditState::default();
    let center = |image: &sp_pipeline::renderer::RenderedImage| {
        let index = ((image.height / 2) * image.width + image.width / 2) as usize * 4;
        [
            image.rgba8[index],
            image.rgba8[index + 1],
            image.rgba8[index + 2],
        ]
    };
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let srgb = renderer
        .render_export(
            &edit,
            false,
            false,
            sp_core::color::OutputColorSpace::Srgb,
            &cancel,
            |_| {},
        )
        .unwrap();
    let adobe = renderer
        .render_export(
            &edit,
            false,
            false,
            sp_core::color::OutputColorSpace::AdobeRgb,
            &cancel,
            |_| {},
        )
        .unwrap();
    let srgb_px = center(&srgb);
    let adobe_px = center(&adobe);
    // Grey stays neutral: both spaces share D65, so the gamut matrix maps
    // equal channels to equal channels.
    assert!(
        adobe_px.iter().all(|c| c.abs_diff(adobe_px[0]) <= 1),
        "Adobe grey is not neutral: {adobe_px:?}"
    );
    // The two encodings describe the same linear light through their own
    // transfer curves. Mid grey is where sRGB and gamma 2.2 differ most in
    // code value, so the bytes must differ while the linear values agree.
    assert_ne!(srgb_px[1], adobe_px[1], "Adobe export used the sRGB curve");
    let srgb_linear = sp_core::color::srgb_eotf(f32::from(srgb_px[1]) / 255.0);
    let adobe_linear = sp_core::color::adobe_rgb_eotf(f32::from(adobe_px[1]) / 255.0);
    assert!(
        (srgb_linear - adobe_linear).abs() < 0.01,
        "sRGB {srgb_px:?} → {srgb_linear:.4}, Adobe {adobe_px:?} → {adobe_linear:.4}"
    );
}

#[test]
fn radial_local_adjustment_only_changes_its_masked_region() {
    use sp_core::edit::{ColorParams, LocalAdjustment, LocalMask, ToneParams};

    let Ok(ctx) = GpuContext::headless() else {
        return;
    };
    let mut renderer = GpuRenderer::new(ctx.clone()).expect("renderer");
    renderer
        .open_image(&synthetic_raw(128, 128, [0.22, 0.24, 0.20]))
        .expect("open");
    let mut edit = EditState::default();
    edit.local_adjustments.push(LocalAdjustment {
        name: "Radial exposure".into(),
        enabled: true,
        mask: LocalMask::Radial {
            center: [0.5, 0.5],
            radius: [0.22, 0.22],
            feather: 0.25,
        },
        tone: ToneParams {
            exposure_ev: 1.0,
            ..Default::default()
        },
        color: ColorParams::default(),
        ..sp_core::edit::LocalAdjustment::default()
    });
    renderer.render(&edit);
    ctx.device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll");
    let (texture, width, height) = renderer.output().expect("output");
    let center = readback_pixel_of(&ctx, texture, width, height, width / 2, height / 2);
    let corner = readback_pixel_of(&ctx, texture, width, height, 4, 4);
    assert!(
        center[1] > corner[1] + 20,
        "radial exposure did not stay local: center={center:?} corner={corner:?}"
    );
}

#[test]
fn full_resolution_alpha_preserves_soft_coverage_in_export() {
    use sp_core::edit::{LocalAdjustment, LocalMask, ToneParams};
    let Ok(ctx) = GpuContext::headless() else {
        return;
    };
    let mut renderer = GpuRenderer::new(ctx).unwrap();
    renderer
        .open_image(&synthetic_raw(128, 128, [0.2, 0.2, 0.2]))
        .unwrap();
    let mut edit = EditState::default();
    edit.local_adjustments.push(LocalAdjustment {
        name: "Alpha".into(),
        enabled: true,
        mask: LocalMask::Alpha(sp_core::mask::AlphaMask {
            width: 128,
            height: 128,
            alpha: (0..128 * 128)
                .map(|i| {
                    if i % 128 < 42 {
                        0
                    } else if i % 128 < 85 {
                        32768
                    } else {
                        65535
                    }
                })
                .collect::<Vec<_>>()
                .into(),
            source: "test".into(),
            inverted: false,
            repairs: vec![],
        }),
        tone: ToneParams {
            exposure_ev: 1.0,
            ..Default::default()
        },
        color: Default::default(),
        ..sp_core::edit::LocalAdjustment::default()
    });
    let image = renderer
        .render_export(
            &edit,
            false,
            true,
            sp_export::ExportColorSpace::Srgb,
            &std::sync::atomic::AtomicBool::new(false),
            |_| {},
        )
        .unwrap();
    let pixels = image.rgba16.unwrap();
    let green = |x| pixels[(64 * image.width as usize + x) * 4 + 1];
    assert!(green(20) + 1000 < green(64));
    assert!(green(64) + 1000 < green(105));
}

#[test]
fn color_range_adjustment_selects_sampled_hue() {
    use sp_core::edit::{ColorParams, LocalAdjustment, LocalMask, ToneParams};

    let Ok(ctx) = GpuContext::headless() else {
        return;
    };
    let mut renderer = GpuRenderer::new(ctx.clone()).expect("renderer");
    renderer
        .open_image(&synthetic_split_color_raw(128, 96))
        .expect("open");
    renderer.render(&EditState::default());
    ctx.device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll");
    let (texture, width, height) = renderer.output().expect("output");
    let before_red = readback_pixel_of(&ctx, texture, width, height, width / 4, height / 2);
    let before_green = readback_pixel_of(&ctx, texture, width, height, width * 3 / 4, height / 2);
    let target_ab = display_oklab_ab([before_red[0], before_red[1], before_red[2]]);

    let mut edit = EditState::default();
    edit.local_adjustments.push(LocalAdjustment {
        name: "Sampled red".into(),
        enabled: true,
        mask: LocalMask::ColorRange {
            target_ab,
            radius: 0.035,
            feather: 0.015,
        },
        tone: ToneParams {
            exposure_ev: 1.0,
            ..Default::default()
        },
        color: ColorParams::default(),
        ..sp_core::edit::LocalAdjustment::default()
    });
    renderer.render(&edit);
    ctx.device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll");
    let (texture, width, height) = renderer.output().expect("output");
    let after_red = readback_pixel_of(&ctx, texture, width, height, width / 4, height / 2);
    let after_green = readback_pixel_of(&ctx, texture, width, height, width * 3 / 4, height / 2);

    assert!(
        after_red[0] > before_red[0] + 25,
        "sampled hue did not receive the adjustment: {before_red:?} -> {after_red:?}"
    );
    assert!(
        after_green[1].abs_diff(before_green[1]) <= 3,
        "unselected hue changed: {before_green:?} -> {after_green:?}"
    );
}

#[test]
fn clone_spot_copies_the_selected_source_region() {
    use sp_core::edit::SpotCorrection;

    let Ok(ctx) = GpuContext::headless() else {
        return;
    };
    let mut renderer = GpuRenderer::new(ctx.clone()).expect("renderer");
    renderer
        .open_image(&synthetic_gradient_raw(160, 96))
        .expect("open");

    renderer.render(&EditState::default());
    ctx.device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll");
    let (texture, width, height) = renderer.output().expect("output");
    let source_before = readback_pixel_of(&ctx, texture, width, height, width / 4, height / 2);
    let target_before = readback_pixel_of(&ctx, texture, width, height, width * 3 / 4, height / 2);

    let mut edit = EditState::default();
    edit.spots.push(SpotCorrection {
        target: [0.75, 0.5],
        source: [0.25, 0.5],
        radius: 0.12,
        feather: 0.2,
        clone: true,
        stroke: Vec::new(),
        ..Default::default()
    });
    renderer.render(&edit);
    ctx.device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll");
    let (texture, width, height) = renderer.output().expect("output");
    let target_after = readback_pixel_of(&ctx, texture, width, height, width * 3 / 4, height / 2);
    let distance = |left: [u8; 4], right: [u8; 4]| {
        left[..3]
            .iter()
            .zip(&right[..3])
            .map(|(a, b)| (i32::from(*a) - i32::from(*b)).abs())
            .sum::<i32>()
    };
    assert!(
        distance(target_after, source_before) < distance(target_before, source_before),
        "clone did not move target toward source: source={source_before:?} before={target_before:?} after={target_after:?}"
    );
}

#[test]
fn red_point_curve_changes_only_red_on_gray() {
    let Ok(ctx) = GpuContext::headless() else {
        return;
    };
    let mut renderer = GpuRenderer::new(ctx.clone()).expect("renderer");
    renderer
        .open_image(&synthetic_raw(64, 64, [0.22; 3]))
        .expect("open");
    renderer.set_view_options(sp_core::edit::ProofSpace::Rec2020, false, 0);
    renderer.render(&EditState::default());
    ctx.device
        .poll(wgpu::PollType::wait_indefinitely())
        .unwrap();
    let neutral = readback_center_pixel(&ctx, &renderer);

    let mut edit = EditState::default();
    edit.curve
        .red
        .points
        .insert(1, sp_core::edit::CurvePoint { x: 0.5, y: 0.3 });
    renderer.render(&edit);
    ctx.device
        .poll(wgpu::PollType::wait_indefinitely())
        .unwrap();
    let changed = readback_center_pixel(&ctx, &renderer);
    assert!(changed[0] + 10 < neutral[0]);
    assert!((i16::from(changed[1]) - i16::from(neutral[1])).abs() <= 1);
    assert!((i16::from(changed[2]) - i16::from(neutral[2])).abs() <= 1);
}

#[test]
fn monochrome_mixer_removes_chroma() {
    let Ok(ctx) = GpuContext::headless() else {
        return;
    };
    let mut renderer = GpuRenderer::new(ctx.clone()).expect("renderer");
    renderer
        .open_image(&synthetic_raw(64, 64, [0.42, 0.24, 0.12]))
        .expect("open");
    renderer.set_view_options(sp_core::edit::ProofSpace::Rec2020, false, 0);
    let mut edit = EditState::default();
    edit.color.monochrome = true;
    edit.color.monochrome_mix[0] = 25.0;
    renderer.render(&edit);
    ctx.device
        .poll(wgpu::PollType::wait_indefinitely())
        .unwrap();
    let pixel = readback_center_pixel(&ctx, &renderer);
    let (low, high) = pixel[..3]
        .iter()
        .fold((u8::MAX, u8::MIN), |(low, high), value| {
            (low.min(*value), high.max(*value))
        });
    assert!(high - low <= 2, "{pixel:?}");
}

#[test]
fn midtone_color_grading_adds_the_selected_hue() {
    let Ok(ctx) = GpuContext::headless() else {
        return;
    };
    let mut renderer = GpuRenderer::new(ctx.clone()).expect("renderer");
    renderer
        .open_image(&synthetic_raw(64, 64, [0.25; 3]))
        .expect("open");
    renderer.set_view_options(sp_core::edit::ProofSpace::Rec2020, false, 0);
    renderer.render(&EditState::default());
    ctx.device
        .poll(wgpu::PollType::wait_indefinitely())
        .unwrap();
    let neutral = readback_center_pixel(&ctx, &renderer);

    let mut edit = EditState::default();
    edit.color.grading.midtones = sp_core::edit::ColorGradeWheel {
        hue_degrees: 35.0,
        saturation: 80.0,
    };
    renderer.render(&edit);
    ctx.device
        .poll(wgpu::PollType::wait_indefinitely())
        .unwrap();
    let warm = readback_center_pixel(&ctx, &renderer);
    let neutral_warmth = i16::from(neutral[0]) - i16::from(neutral[2]);
    let graded_warmth = i16::from(warm[0]) - i16::from(warm[2]);
    assert!(
        graded_warmth > neutral_warmth + 4,
        "{warm:?} vs {neutral:?}"
    );
}

#[test]
fn gamut_warning_marks_srgb_but_not_rec2020_green() {
    let Ok(ctx) = GpuContext::headless() else {
        return;
    };
    let mut renderer = GpuRenderer::new(ctx.clone()).expect("renderer");
    renderer
        .open_image(&synthetic_raw(64, 64, [0.0, 0.8, 0.0]))
        .expect("open");
    renderer.set_view_options(sp_core::edit::ProofSpace::Srgb, true, 0);
    renderer.render(&EditState::default());
    ctx.device
        .poll(wgpu::PollType::wait_indefinitely())
        .unwrap();
    let srgb = readback_center_pixel(&ctx, &renderer);
    assert!(srgb[0] > 250 && srgb[1] < 5 && srgb[2] > 250, "{srgb:?}");

    renderer.set_view_options(sp_core::edit::ProofSpace::Rec2020, true, 0);
    renderer.render(&EditState::default());
    ctx.device
        .poll(wgpu::PollType::wait_indefinitely())
        .unwrap();
    let wide = readback_center_pixel(&ctx, &renderer);
    assert!(!(wide[0] > 250 && wide[1] < 5 && wide[2] > 250), "{wide:?}");
}

#[test]
fn local_contrast_is_invariant_to_tile_origin() {
    let Ok(ctx) = GpuContext::headless() else {
        return;
    };
    let mut renderer = GpuRenderer::new(ctx.clone()).expect("renderer");
    renderer
        .open_image(&synthetic_gradient_raw(192, 128))
        .expect("open");
    let mut edit = EditState::default();
    edit.local.clarity = 50.0;
    edit.local.texture = 35.0;
    let global = (88_u32, 64_u32);

    let first = renderer.render_tile(&edit, 32, 24, 96, 80).unwrap();
    ctx.device
        .poll(wgpu::PollType::wait_indefinitely())
        .unwrap();
    let first_origin = (first.x - first.margin_x, first.y - first.margin_y);
    let a = readback_pixel_of(
        &ctx,
        renderer.tile_texture().unwrap(),
        first.tex_w,
        first.tex_h,
        global.0 - first_origin.0,
        global.1 - first_origin.1,
    );

    let second = renderer.render_tile(&edit, 48, 16, 96, 88).unwrap();
    ctx.device
        .poll(wgpu::PollType::wait_indefinitely())
        .unwrap();
    let second_origin = (second.x - second.margin_x, second.y - second.margin_y);
    let b = readback_pixel_of(
        &ctx,
        renderer.tile_texture().unwrap(),
        second.tex_w,
        second.tex_h,
        global.0 - second_origin.0,
        global.1 - second_origin.1,
    );
    for channel in 0..3 {
        assert!(
            (i16::from(a[channel]) - i16::from(b[channel])).abs() <= 1,
            "{a:?} vs {b:?}"
        );
    }
}

#[test]
fn display_p3_proof_matches_display_p3_export() {
    let Ok(ctx) = GpuContext::headless() else {
        return;
    };
    let mut renderer = GpuRenderer::new(ctx.clone()).expect("renderer");
    renderer
        .open_image(&synthetic_raw(64, 64, [0.55, 0.25, 0.08]))
        .expect("open");
    renderer.set_view_options(sp_core::edit::ProofSpace::DisplayP3, false, 0);
    renderer.render(&EditState::default());
    ctx.device
        .poll(wgpu::PollType::wait_indefinitely())
        .unwrap();
    let proof = readback_center_pixel(&ctx, &renderer);

    let exported = renderer
        .render_export(
            &EditState::default(),
            false,
            false,
            sp_core::color::OutputColorSpace::DisplayP3,
            &std::sync::atomic::AtomicBool::new(false),
            |_| {},
        )
        .unwrap();
    let offset = ((exported.height / 2 * exported.width + exported.width / 2) * 4) as usize;
    for (channel, proof_value) in proof.iter().take(3).enumerate() {
        assert!((i16::from(*proof_value) - i16::from(exported.rgba8[offset + channel])).abs() <= 2);
    }
}

#[test]
fn exposure_ev_doubles_linear_light() {
    let Ok(ctx) = GpuContext::headless() else {
        return;
    };
    let mut renderer = GpuRenderer::new(ctx.clone()).expect("renderer");
    let raw = synthetic_raw(64, 64, [0.2, 0.2, 0.2]);
    renderer.open_image(&raw).expect("open");

    let mut edit = EditState::default();
    edit.tone.exposure_ev = 1.0;
    renderer.render(&edit);

    let got = readback_center_pixel(&ctx, &renderer);
    // +1 EV on 0.2 gray through the same LUT the GPU applies.
    let want = expected_rgb8_with([0.2, 0.2, 0.2], 1.0)[1];
    let diff = (i32::from(got[1]) - i32::from(want)).abs();
    assert!(diff <= 2, "got {} want {want}", got[1]);
}

#[test]
fn synthetic_dcp_huesat_map_changes_gpu_render() {
    let Ok(ctx) = GpuContext::headless() else {
        return;
    };
    let mut renderer = GpuRenderer::new(ctx.clone()).expect("renderer");
    renderer
        .open_image(&synthetic_raw(64, 64, [0.42, 0.30, 0.18]))
        .expect("open");
    let mut edit = EditState::default();
    edit.wb.temp_kelvin = Some(6_504.0);
    renderer.render(&edit);
    let standard = readback_center_pixel(&ctx, &renderer);

    let map = HueSatLut {
        hue_divisions: 6,
        sat_divisions: 2,
        val_divisions: 1,
        entries: (0..12).map(|_| (28.0, 1.15, 1.0)).collect(),
    };
    let profile = CameraProfile {
        name: "Synthetic DCP".into(),
        camera_model: "test".into(),
        matrices: vec![
            IlluminantMatrix {
                illuminant: Illuminant::StdA,
                cam_to_xyz: REC2020_TO_XYZ,
            },
            IlluminantMatrix {
                illuminant: Illuminant::D65,
                cam_to_xyz: REC2020_TO_XYZ,
            },
        ],
        forward_matrices: Vec::new(),
        huesat1: Some(map.clone()),
        huesat2: Some(map),
        look: None,
        rgb_lut: None,
        chroma_warp1: None,
        chroma_warp2: None,
        base_curve: None,
    };
    renderer.set_profile(Some(profile));
    renderer.render(&edit);
    let profiled = readback_center_pixel(&ctx, &renderer);
    let distance: i32 = (0..3)
        .map(|c| (i32::from(profiled[c]) - i32::from(standard[c])).abs())
        .sum();
    assert!(
        distance > 12,
        "standard={standard:?}, profiled={profiled:?}"
    );
}

#[test]
fn compound_local_masks_preserve_soft_boolean_coverage_and_modifiers() {
    use sp_core::edit::{LocalAdjustment, LocalMask, MaskOperation, ToneParams};
    let Ok(ctx) = GpuContext::headless() else {
        return;
    };
    let mut renderer = GpuRenderer::new(ctx).unwrap();
    renderer
        .open_image(&synthetic_raw(128, 96, [0.18; 3]))
        .unwrap();
    let render = |renderer: &mut GpuRenderer, edit: &EditState| {
        renderer
            .render_export(
                edit,
                false,
                true,
                sp_core::color::OutputColorSpace::Srgb,
                &std::sync::atomic::AtomicBool::new(false),
                |_| {},
            )
            .unwrap()
            .rgba16
            .unwrap()
    };
    let neutral = render(&mut renderer, &EditState::default());
    let sample = |pixels: &[u16], x: usize| pixels[(48 * 128 + x) * 4 + 1];
    let leaf = |center| LocalMask::Radial {
        center: [center, 0.5],
        radius: [0.25, 0.3],
        feather: 0.0,
    };
    let mut edit = EditState::default();
    edit.local_adjustments.push(LocalAdjustment {
        tone: ToneParams {
            exposure_ev: 1.0,
            ..Default::default()
        },
        ..Default::default()
    });
    for (operation, selected) in [
        (MaskOperation::Add, [true, true, true]),
        (MaskOperation::Subtract, [true, false, false]),
        (MaskOperation::Intersect, [false, true, false]),
    ] {
        edit.local_adjustments[0].mask = LocalMask::Composite {
            operation,
            left: Box::new(leaf(0.35)),
            right: Box::new(leaf(0.65)),
        };
        let result = render(&mut renderer, &edit);
        for (x, changed) in [26, 64, 102].into_iter().zip(selected) {
            let delta = i32::from(sample(&result, x)) - i32::from(sample(&neutral, x));
            assert!(
                if changed {
                    delta > 4000
                } else {
                    delta.abs() <= 2
                },
                "{operation:?} x={x}: delta={delta}"
            );
        }
    }
    // Inversion applies to the completed expression, then amount scales it.
    edit.local_adjustments[0].mask_inverted = true;
    edit.local_adjustments[0].opacity = 0.5;
    let half = render(&mut renderer, &edit);
    assert!(sample(&half, 26) > sample(&neutral, 26) + 1000);
    assert!(sample(&half, 64).abs_diff(sample(&neutral, 64)) <= 2);
    edit.local_adjustments[0].opacity = 1.0;
    let full = render(&mut renderer, &edit);
    assert!(sample(&full, 26) > sample(&half, 26) + 1000);
    edit.local_adjustments[0].opacity = 0.0;
    assert_eq!(render(&mut renderer, &edit), neutral);
}

#[test]
fn compound_masks_preserve_alpha_softness_and_source_modifiers() {
    use sp_core::edit::{LocalAdjustment, LocalMask, MaskOperation, ToneParams};
    let Ok(ctx) = GpuContext::headless() else {
        return;
    };
    let mut renderer = GpuRenderer::new(ctx).unwrap();
    renderer
        .open_image(&synthetic_raw(64, 64, [0.15; 3]))
        .unwrap();
    let alpha = |coverage| {
        LocalMask::Alpha(sp_core::mask::AlphaMask {
            width: 1,
            height: 1,
            alpha: vec![coverage].into(),
            source: "soft".into(),
            inverted: false,
            repairs: vec![],
        })
    };
    let mut edit = EditState::default();
    edit.local_adjustments.push(LocalAdjustment {
        mask: LocalMask::Composite {
            operation: MaskOperation::Intersect,
            left: Box::new(alpha(32768)),
            right: Box::new(LocalMask::Modified {
                mask: Box::new(alpha(0)),
                inverted: true,
                opacity: 0.5,
            }),
        },
        tone: ToneParams {
            exposure_ev: 2.0,
            ..Default::default()
        },
        ..Default::default()
    });
    let render = |renderer: &mut GpuRenderer, edit: &EditState| {
        renderer
            .render_export(
                edit,
                false,
                true,
                sp_core::color::OutputColorSpace::Srgb,
                &std::sync::atomic::AtomicBool::new(false),
                |_| {},
            )
            .unwrap()
            .rgba16
            .unwrap()
    };
    let compound = render(&mut renderer, &edit);
    edit.local_adjustments[0].mask = alpha(65535);
    edit.local_adjustments[0].tone.exposure_ev = 0.5;
    let equivalent = render(&mut renderer, &edit);
    assert!(compound
        .iter()
        .zip(&equivalent)
        .all(|(a, b)| a.abs_diff(*b) <= 3));
}

#[test]
fn extended_local_controls_change_only_selected_pixels() {
    use sp_core::edit::{CurvePoint, LocalAdjustment, LocalMask};
    let Ok(ctx) = GpuContext::headless() else {
        return;
    };
    let mut renderer = GpuRenderer::new(ctx).unwrap();
    // Deterministic, textured color image gives spatial tools real signal.
    let mut raw = synthetic_raw(128, 96, [0.24, 0.15, 0.10]);
    for y in 0..raw.height {
        for x in 0..raw.width {
            let noise = (((x / 2 * 37 + y / 2 * 53) % 17) as i32 - 8) * 150;
            let index = (y * raw.width + x) as usize;
            raw.cfa_data[index] = (i32::from(raw.cfa_data[index]) + noise) as u16;
        }
    }
    renderer.open_image(&raw).unwrap();
    let render = |renderer: &mut GpuRenderer, edit: &EditState| {
        renderer
            .render_export(
                edit,
                false,
                true,
                sp_core::color::OutputColorSpace::Srgb,
                &std::sync::atomic::AtomicBool::new(false),
                |_| {},
            )
            .unwrap()
            .rgba16
            .unwrap()
    };
    let baseline = render(&mut renderer, &EditState::default());
    for feature in [
        "temperature",
        "tint",
        "curve",
        "channel",
        "hsl",
        "grading",
        "clarity",
        "texture",
        "dehaze",
        "sharpness",
        "noise",
    ] {
        let mut adjustment = LocalAdjustment {
            mask: LocalMask::Alpha(sp_core::mask::AlphaMask {
                width: 2,
                height: 1,
                alpha: vec![65535, 0].into(),
                source: "left".into(),
                inverted: false,
                repairs: vec![],
            }),
            ..Default::default()
        };
        match feature {
            "temperature" => adjustment.temperature = 70.0,
            "tint" => adjustment.tint = 70.0,
            "curve" => adjustment
                .curve
                .luma
                .points
                .insert(1, CurvePoint { x: 0.5, y: 0.7 }),
            "channel" => adjustment
                .curve
                .red
                .points
                .insert(1, CurvePoint { x: 0.5, y: 0.7 }),
            "hsl" => {
                for band in &mut adjustment.color.hsl {
                    band.hue_shift = 70.0;
                }
            }
            "grading" => {
                adjustment.color.grading.midtones.hue_degrees = 240.0;
                adjustment.color.grading.midtones.saturation = 80.0;
            }
            "clarity" => adjustment.local.clarity = 90.0,
            "texture" => adjustment.local.texture = 90.0,
            "dehaze" => adjustment.local.dehaze = 70.0,
            "sharpness" => adjustment.sharpness = 90.0,
            "noise" => adjustment.noise_reduction = 90.0,
            _ => unreachable!(),
        }
        let mut edit = EditState::default();
        edit.local_adjustments.push(adjustment);
        let result = render(&mut renderer, &edit);
        let mut inside = 0u64;
        for y in 12..84 {
            for x in 4..24 {
                for c in 0..3 {
                    let i = (y * 128 + x) * 4 + c;
                    inside += u64::from(result[i].abs_diff(baseline[i]));
                }
            }
        }
        assert!(
            inside > 1000,
            "{feature} had no meaningful effect in selected region: {inside}"
        );
        for y in 12..84 {
            for x in 100..124 {
                for c in 0..3 {
                    let i = (y * 128 + x) * 4 + c;
                    assert!(
                        result[i].abs_diff(baseline[i]) <= 2,
                        "{feature} leaked outside the mask"
                    );
                }
            }
        }
    }
}

#[test]
fn local_preview_uses_exact_composite_color_luma_and_modifier_coverage() {
    use sp_core::edit::{LocalAdjustment, LocalMask, MaskOperation, ToneParams};
    let Ok(ctx) = GpuContext::headless() else {
        return;
    };
    let mut renderer = GpuRenderer::new(ctx.clone()).unwrap();
    renderer
        .open_image(&synthetic_raw(128, 96, [0.18; 3]))
        .unwrap();
    let selection = LocalMask::Composite {
        operation: MaskOperation::Intersect,
        left: Box::new(LocalMask::ColorRange {
            target_ab: [0.0, 0.0],
            radius: 0.1,
            feather: 0.1,
        }),
        right: Box::new(LocalMask::LuminanceRange {
            minimum: 0.3,
            maximum: 10.0,
            feather: 0.0,
        }),
    };
    let mut edit = EditState {
        local_adjustments: vec![
            LocalAdjustment {
                tone: ToneParams {
                    exposure_ev: -5.0,
                    ..Default::default()
                },
                mask: LocalMask::LuminanceRange {
                    minimum: 0.0,
                    maximum: 10.0,
                    feather: 0.0,
                },
                ..Default::default()
            },
            LocalAdjustment {
                mask: selection,
                opacity: 0.4,
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    renderer.set_mask_preview(Some(1));
    renderer.render(&edit);
    let dark = renderer.read_mask_preview().unwrap().unwrap();
    assert!(dark.sample([0.5, 0.5]) < 0.01);
    edit.local_adjustments[0].tone.exposure_ev = 2.0;
    renderer.render(&edit);
    let bright = renderer.read_mask_preview().unwrap().unwrap();
    assert!((bright.sample([0.5, 0.5]) - 0.4).abs() < 0.002);
    let with_overlay = readback_center_pixel(&ctx, &renderer);
    renderer.set_mask_preview(None);
    renderer.render(&edit);
    assert_eq!(
        with_overlay,
        readback_center_pixel(&ctx, &renderer),
        "preview must not modify the developed photo"
    );
    assert!(renderer.read_mask_preview().unwrap().is_none());
    edit.local_adjustments[1].mask_inverted = true;
    renderer.set_mask_preview(Some(1));
    renderer.render(&edit);
    assert!(
        renderer
            .read_mask_preview()
            .unwrap()
            .unwrap()
            .sample([0.5, 0.5])
            < 0.01
    );
}

#[test]
fn paint_flow_pressure_and_erase_are_continuous_and_circular_in_pixels() {
    use sp_core::edit::{LocalAdjustment, LocalMask};
    use sp_core::mask::{BrushSettings, PaintStroke};
    let Ok(ctx) = GpuContext::headless() else {
        return;
    };
    let mut renderer = GpuRenderer::new(ctx).unwrap();
    renderer
        .open_image(&synthetic_raw(256, 128, [0.18; 3]))
        .unwrap();
    renderer.set_mask_preview(Some(0));
    let settings = BrushSettings {
        size: 0.2,
        feather: 0.0,
        flow: 0.25,
        pressure: true,
        ..Default::default()
    };
    let stroke = PaintStroke::new([0.5, 0.5], 0.5, false);
    let mut edit = EditState::default();
    edit.local_adjustments.push(LocalAdjustment {
        mask: LocalMask::Paint {
            strokes: vec![stroke.clone()],
            settings,
        },
        ..Default::default()
    });
    renderer.render(&edit);
    let coverage = renderer.read_mask_preview().unwrap().unwrap();
    assert!((coverage.sample([0.5, 0.5]) - 0.25).abs() < 0.002);
    assert!(
        coverage.sample([0.56, 0.5]) < 0.001,
        "pressure halves radius"
    );
    assert!(
        coverage.sample([0.5, 0.58]) > 0.24,
        "circular radius follows image short edge"
    );
    if let LocalMask::Paint { strokes, .. } = &mut edit.local_adjustments[0].mask {
        strokes.push(stroke);
    }
    renderer.render(&edit);
    assert!(
        (renderer
            .read_mask_preview()
            .unwrap()
            .unwrap()
            .sample([0.5, 0.5])
            - 0.4375)
            .abs()
            < 0.002
    );
    if let LocalMask::Paint { strokes, .. } = &mut edit.local_adjustments[0].mask {
        strokes.push(PaintStroke::new([0.5, 0.5], 1.0, true));
    }
    renderer.render(&edit);
    assert!(
        (renderer
            .read_mask_preview()
            .unwrap()
            .unwrap()
            .sample([0.5, 0.5])
            - 0.328125)
            .abs()
            < 0.002
    );
}

#[test]
fn canonical_mask_input_bypasses_lens_geometry_without_changing_develop() {
    let Ok(ctx) = GpuContext::headless() else {
        return;
    };
    let mut raw = synthetic_raw(128, 96, [0.18; 3]);
    for y in 0..raw.height {
        for x in 0..raw.width {
            if (x / 16 + y / 16) % 2 == 0 {
                raw.cfa_data[(y * raw.width + x) as usize] = 48000;
            }
        }
    }
    let mut renderer = GpuRenderer::new(ctx).unwrap();
    renderer.open_image(&raw).unwrap();
    let render = |renderer: &mut GpuRenderer, edit: &EditState| {
        renderer
            .render_export(
                edit,
                false,
                true,
                sp_core::color::OutputColorSpace::Srgb,
                &std::sync::atomic::AtomicBool::new(false),
                |_| {},
            )
            .unwrap()
            .rgba16
            .unwrap()
    };
    let mut edit = EditState::default();
    edit.tone.exposure_ev = 0.2;
    let original = render(&mut renderer, &edit);
    edit.lens.enable = true;
    edit.lens.crop_to_valid = true;
    edit.lens.distortion = [0.12, 0.02, 0.0];
    edit.lens.lateral_ca = [0.02, -0.01];
    let corrected = render(&mut renderer, &edit);
    assert_ne!(original, corrected);
    renderer.set_canonical_mask_input(true);
    assert_eq!(
        original,
        render(&mut renderer, &edit),
        "AI mattes must use canonical pre-warp coordinates while retaining develop controls"
    );
}
