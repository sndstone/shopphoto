//! Acceptance tests for lens shading correction and the chroma-plane warp.
//!
//! Both stages ride along inside existing passes (shading in raw prepare,
//! the warp in wb_color), so these exercise them through the real tile
//! render path rather than in isolation. Skips without a GPU adapter.

use sp_core::lens::{GridGeometry, LensProfile, ShadingGrid, QUAD_SLOTS};
use sp_core::profile::{CameraProfile, ChromaWarp};
use sp_core::raw::{ActiveArea, CfaPattern, Orientation};
use sp_core::{EditState, RawImage};
use sp_gpu::GpuContext;
use sp_pipeline::GpuRenderer;

const W: u32 = 128;
const H: u32 = 128;
/// Grid spacing chosen so a 5×5 grid spans the 128×128 test sensor exactly.
const SPACING: f32 = 32.0;

fn geometry() -> GridGeometry {
    let g = GridGeometry::for_sensor(W, H, SPACING);
    assert_eq!((g.grid_w, g.grid_h), (5, 5), "test assumes a 5x5 grid");
    g
}

/// Radial falloff at grid node `(gx, gy)`: 1.0 at the centre, ~0.55 in the
/// corners. Deliberately evaluated on grid nodes so the pixel-domain
/// vignette below is exactly piecewise-bilinear, and its inverse is
/// therefore exactly representable by the same grid.
fn node_falloff(gx: u32, gy: u32) -> f32 {
    let c = 2.0_f32;
    let (dx, dy) = ((gx as f32 - c) / c, (gy as f32 - c) / c);
    1.0 - 0.45 * ((dx * dx + dy * dy) / 2.0)
}

/// Bilinearly interpolate the node falloff at absolute sensor pixel (x, y),
/// mirroring `lens_gain` in the shaders.
fn pixel_falloff(x: u32, y: u32, geom: GridGeometry) -> f32 {
    let coord = |v: u32, center: f32, n: u32| {
        let g = (v as f32 - center) / SPACING + (n - 1) as f32 / 2.0;
        let g = g.clamp(0.0, (n - 1) as f32);
        let i0 = g.floor();
        (i0 as u32, (i0 as u32 + 1).min(n - 1), g - i0)
    };
    let (x0, x1, fx) = coord(x, geom.center_x, geom.grid_w);
    let (y0, y1, fy) = coord(y, geom.center_y, geom.grid_h);
    let top = node_falloff(x0, y0) + (node_falloff(x1, y0) - node_falloff(x0, y0)) * fx;
    let bot = node_falloff(x0, y1) + (node_falloff(x1, y1) - node_falloff(x0, y1)) * fx;
    top + (bot - top) * fy
}

/// Flat mid-gray CFA with `node_falloff` baked in — a synthetic vignetted
/// flat field, which is exactly what a calibration shot is.
fn vignetted_flat() -> RawImage {
    let geom = geometry();
    let mut cfa = vec![0u16; (W * H) as usize];
    for y in 0..H {
        for x in 0..W {
            let v = 0.5 * pixel_falloff(x, y, geom);
            cfa[(y * W + x) as usize] = (v * 65_535.0) as u16;
        }
    }
    base_raw(cfa)
}

/// Flat mid-gray CFA, no falloff.
fn flat() -> RawImage {
    flat_at(0.5)
}

/// Flat CFA at `level`, plus a mild colour cast, kept dark enough that no
/// channel clips after the tone curve. Clipped channels cannot move, which
/// silently turns a colour assertion into a tautology.
fn colored_flat() -> RawImage {
    let mut raw = flat_at(0.22);
    raw.wb_coeffs = [1.35, 1.0, 0.80, 1.0];
    raw
}

/// Dark subject, narrow synthetic magenta axial fringe, then a neutral
/// highlight. The color lives only on R/B photosites so the real demosaic
/// must carry it into developed RGB before the defringe stage can repair it.
fn purple_highlight_edge() -> RawImage {
    let mut cfa = vec![0_u16; (W * H) as usize];
    for y in 0..H {
        for x in 0..W {
            let slot = ((y & 1) * 2 + (x & 1)) as usize;
            let level = if x >= 66 {
                0.82
            } else if (61..66).contains(&x) {
                if slot == 1 || slot == 2 {
                    0.09
                } else {
                    0.48
                }
            } else {
                0.09
            };
            cfa[(y * W + x) as usize] = (level * 65_535.0) as u16;
        }
    }
    base_raw(cfa)
}

/// Eclipse-like broad magenta bloom between a dark silhouette and a nearly
/// clipped neutral highlight. At 20 sensor pixels it is far wider than
/// ordinary one-pixel demosaic/LCA color and exercises the explicit wide path.
fn wide_purple_highlight_bloom() -> RawImage {
    let mut cfa = vec![0_u16; (W * H) as usize];
    for y in 0..H {
        for x in 0..W {
            let slot = ((y & 1) * 2 + (x & 1)) as usize;
            let level = if x >= 70 {
                0.94
            } else if (48..70).contains(&x) {
                if slot == 1 || slot == 2 {
                    0.12
                } else {
                    0.68
                }
            } else {
                0.04
            };
            cfa[(y * W + x) as usize] = (level * 65_535.0) as u16;
        }
    }
    base_raw(cfa)
}

fn green_highlight_edge() -> RawImage {
    let mut cfa = vec![0_u16; (W * H) as usize];
    for y in 0..H {
        for x in 0..W {
            let slot = ((y & 1) * 2 + (x & 1)) as usize;
            let level = if x >= 66 {
                0.82
            } else if (61..66).contains(&x) {
                if slot == 1 || slot == 2 {
                    0.48
                } else {
                    0.09
                }
            } else {
                0.09
            };
            cfa[(y * W + x) as usize] = (level * 65_535.0) as u16;
        }
    }
    base_raw(cfa)
}

fn solid_purple_subject() -> RawImage {
    let mut cfa = vec![0_u16; (W * H) as usize];
    for y in 0..H {
        for x in 0..W {
            let slot = ((y & 1) * 2 + (x & 1)) as usize;
            let level = if slot == 1 || slot == 2 { 0.10 } else { 0.38 };
            cfa[(y * W + x) as usize] = (level * 65_535.0) as u16;
        }
    }
    base_raw(cfa)
}

fn flat_at(level: f32) -> RawImage {
    base_raw(vec![(level * 65_535.0) as u16; (W * H) as usize])
}

fn base_raw(cfa_data: Vec<u16>) -> RawImage {
    RawImage {
        cfa_data,
        width: W,
        height: H,
        cfa: CfaPattern::Rggb,
        black_levels: [0.0; 4],
        white_levels: [65_535.0; 4],
        wb_coeffs: [1.0, 1.0, 1.0, 1.0],
        cam_to_xyz: sp_core::color::REC2020_TO_XYZ,
        active_area: ActiveArea {
            x: 0,
            y: 0,
            width: W,
            height: H,
        },
        orientation: Orientation::Normal,
    }
}

/// A profile whose gains invert `node_falloff` — the result of calibrating
/// against `vignetted_flat`.
fn inverse_profile() -> LensProfile {
    let geom = geometry();
    let mut gains = Vec::with_capacity((geom.grid_w * geom.grid_h) as usize);
    for gy in 0..geom.grid_h {
        for gx in 0..geom.grid_w {
            gains.push([1.0 / node_falloff(gx, gy); QUAD_SLOTS]);
        }
    }
    LensProfile {
        apertures: vec![ShadingGrid {
            f_number: 4.0,
            gains,
        }],
        ..LensProfile::identity("Test 90mm", "test90", geom)
    }
}

/// Render `raw` with `edit` and return the visible tile as RGB in 0..1.
fn render_rgb(
    ctx: &GpuContext,
    renderer: &mut GpuRenderer,
    raw: &RawImage,
    edit: &EditState,
) -> Vec<[f32; 3]> {
    // Match the app/export path: a loaded sidecar's lens state is baked into
    // the first proxy rather than repaired by a second full-sensor pass.
    renderer
        .open_image_with_edit(raw, edit)
        .expect("open with edit");
    let info = renderer
        .render_tile(edit, 8, 8, W - 16, H - 16)
        .expect("tile");
    let tex = renderer.tile_texture().expect("tex");
    let (tw, th) = (info.tex_w, info.tex_h);
    let bytes_per_row = (tw * 4).div_ceil(256) * 256;
    let buf = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: u64::from(bytes_per_row * th),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = ctx.device.create_command_encoder(&Default::default());
    enc.copy_texture_to_buffer(
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
                rows_per_image: Some(th),
            },
        },
        wgpu::Extent3d {
            width: tw,
            height: th,
            depth_or_array_layers: 1,
        },
    );
    ctx.queue.submit([enc.finish()]);
    let slice = buf.slice(..);
    slice.map_async(wgpu::MapMode::Read, |r| r.expect("map"));
    ctx.device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll");
    let data = slice.get_mapped_range();

    let mut out = Vec::with_capacity((info.w * info.h) as usize);
    for y in 0..info.h {
        for x in 0..info.w {
            let off = ((y + info.margin_y) * bytes_per_row + (x + info.margin_x) * 4) as usize;
            out.push([
                f32::from(data[off]) / 255.0,
                f32::from(data[off + 1]) / 255.0,
                f32::from(data[off + 2]) / 255.0,
            ]);
        }
    }
    out
}

/// Mean green of the `n`×`n` block at the top-left of the visible tile.
fn corner_green(px: &[[f32; 3]], n: usize) -> f32 {
    let w = (W - 16) as usize;
    let mut acc = 0.0;
    for y in 0..n {
        for x in 0..n {
            acc += px[y * w + x][1];
        }
    }
    acc / (n * n) as f32
}

/// Mean green of the `n`×`n` block at the centre of the visible tile.
fn center_green(px: &[[f32; 3]], n: usize) -> f32 {
    let w = (W - 16) as usize;
    let h = px.len() / w;
    let (cx, cy) = (w / 2 - n / 2, h / 2 - n / 2);
    let mut acc = 0.0;
    for y in 0..n {
        for x in 0..n {
            acc += px[(cy + y) * w + cx + x][1];
        }
    }
    acc / (n * n) as f32
}

fn setup() -> Option<(GpuContext, GpuRenderer)> {
    let ctx = GpuContext::headless().ok()?;
    let renderer = GpuRenderer::new(ctx.clone()).ok()?;
    Some((ctx, renderer))
}

#[test]
fn lens_profile_flattens_a_vignetted_flat_field() {
    let Some((ctx, mut renderer)) = setup() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let raw = vignetted_flat();
    let edit = EditState::default();

    // Uncorrected: the corner is visibly darker than the centre.
    renderer.set_lens_profile(None, None);
    let before = render_rgb(&ctx, &mut renderer, &raw, &edit);
    let (c_before, m_before) = (corner_green(&before, 8), center_green(&before, 8));
    assert!(
        m_before - c_before > 0.05,
        "fixture should vignette: corner {c_before}, centre {m_before}"
    );

    // Corrected: the frame comes back flat.
    let profile = inverse_profile();
    renderer.set_lens_profile(Some(&profile), Some(4.0));
    assert!(renderer.has_lens_profile());
    let after = render_rgb(&ctx, &mut renderer, &raw, &edit);
    let (c_after, m_after) = (corner_green(&after, 8), center_green(&after, 8));
    assert!(
        (m_after - c_after).abs() < 0.02,
        "correction should flatten the field: corner {c_after}, centre {m_after}"
    );
    assert!(
        (m_after - m_before).abs() < 0.02,
        "the optical centre has unity gain and must not move: {m_before} -> {m_after}"
    );
}

#[test]
fn lens_correction_respects_the_toggle_and_strength() {
    let Some((ctx, mut renderer)) = setup() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let raw = vignetted_flat();
    let profile = inverse_profile();
    renderer.set_lens_profile(Some(&profile), Some(4.0));

    let mut off = EditState::default();
    off.lens.enable = false;
    let off_px = render_rgb(&ctx, &mut renderer, &raw, &off);

    let mut zero = EditState::default();
    zero.lens.vignetting = 0.0;
    let zero_px = render_rgb(&ctx, &mut renderer, &raw, &zero);
    assert_eq!(
        off_px, zero_px,
        "zero strength must be identical to the toggle being off"
    );

    let mut half = EditState::default();
    half.lens.vignetting = 50.0;
    let half_px = render_rgb(&ctx, &mut renderer, &raw, &half);
    let full_px = render_rgb(&ctx, &mut renderer, &raw, &EditState::default());

    let (c_off, c_half, c_full) = (
        corner_green(&off_px, 8),
        corner_green(&half_px, 8),
        corner_green(&full_px, 8),
    );
    assert!(
        c_off < c_half && c_half < c_full,
        "strength should interpolate: off {c_off}, half {c_half}, full {c_full}"
    );
}

#[test]
fn selective_defringe_reduces_purple_at_a_highlight_edge_without_a_luma_halo() {
    let Some((ctx, mut renderer)) = setup() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    renderer.set_lens_profile(None, None);
    let raw = purple_highlight_edge();
    let before = render_rgb(&ctx, &mut renderer, &raw, &EditState::default());

    let mut edit = EditState::default();
    edit.lens.defringe = true;
    edit.lens.defringe_purple = 100.0;
    edit.lens.defringe_radius_px = 3.0;
    edit.lens.defringe_threshold = 0.0;
    let after = render_rgb(&ctx, &mut renderer, &raw, &edit);

    let visible_w = (W - 16) as usize;
    let x0 = 61_usize - 8;
    let x1 = 66_usize - 8;
    let mut before_excess = 0.0;
    let mut after_excess = 0.0;
    let mut before_luma = 0.0;
    let mut after_luma = 0.0;
    let mut count = 0.0;
    for y in 16..(H as usize - 32) {
        for x in x0..x1 {
            let b = before[y * visible_w + x];
            let a = after[y * visible_w + x];
            before_excess += (b[0].min(b[2]) - b[1]).max(0.0);
            after_excess += (a[0].min(a[2]) - a[1]).max(0.0);
            before_luma += 0.2627 * b[0] + 0.6780 * b[1] + 0.0593 * b[2];
            after_luma += 0.2627 * a[0] + 0.6780 * a[1] + 0.0593 * a[2];
            count += 1.0;
        }
    }
    before_excess /= count;
    after_excess /= count;
    before_luma /= count;
    after_luma /= count;
    assert!(
        before_excess > 0.02 && after_excess < before_excess * 0.82,
        "purple fringe should fall materially: {before_excess} -> {after_excess}"
    );
    assert!(
        (after_luma - before_luma).abs() < 0.05,
        "linear-luminance-preserving repair must not create a visible halo after output shaping: {before_luma} -> {after_luma}"
    );
}

#[test]
fn wide_bloom_mode_repairs_a_broad_magenta_highlight_halo() {
    let Some((ctx, mut renderer)) = setup() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    renderer.set_lens_profile(None, None);
    let raw = wide_purple_highlight_bloom();
    let before = render_rgb(&ctx, &mut renderer, &raw, &EditState::default());

    let mut edit = EditState::default();
    edit.lens.defringe = true;
    edit.lens.defringe_purple = 100.0;
    edit.lens.defringe_radius_px = 24.0;
    edit.lens.defringe_threshold = 20.0;
    edit.lens.defringe_wide_bloom = true;
    let after = render_rgb(&ctx, &mut renderer, &raw, &edit);

    let visible_w = (W - 16) as usize;
    let mut before_excess = 0.0;
    let mut after_excess = 0.0;
    let mut before_luma = 0.0;
    let mut after_luma = 0.0;
    let mut count = 0.0;
    for y in 20..(H as usize - 28) {
        for sensor_x in 51_usize..68 {
            let x = sensor_x - 8;
            let b = before[y * visible_w + x];
            let a = after[y * visible_w + x];
            before_excess += (b[0].min(b[2]) - b[1]).max(0.0);
            after_excess += (a[0].min(a[2]) - a[1]).max(0.0);
            before_luma += 0.2627 * b[0] + 0.6780 * b[1] + 0.0593 * b[2];
            after_luma += 0.2627 * a[0] + 0.6780 * a[1] + 0.0593 * a[2];
            count += 1.0;
        }
    }
    before_excess /= count;
    after_excess /= count;
    before_luma /= count;
    after_luma /= count;
    assert!(
        after_excess < before_excess * 0.68,
        "wide magenta bloom should be materially neutralized: {before_excess} -> {after_excess}"
    );
    assert!(
        (after_luma - before_luma).abs() < 0.06,
        "wide repair must retain the halo's luminance: {before_luma} -> {after_luma}"
    );
}

#[test]
fn automatic_scan_estimates_a_wide_bloom_radius() {
    let Some((ctx, mut renderer)) = setup() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    renderer
        .open_image(&wide_purple_highlight_bloom())
        .expect("open broad synthetic fringe");
    assert!(renderer
        .render_for_fringe_analysis(&EditState::default())
        .is_none());
    ctx.device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("wait for wide analysis");
    let histogram = renderer.poll_histogram().expect("wide histogram");
    let recommendation = sp_pipeline::auto_defringe::recommend(&histogram)
        .expect("coherent broad fringe should be detected");
    assert!(recommendation.purple > 0.0);
    assert!(recommendation.radius_px > 8.0, "{recommendation:?}");
    assert!(recommendation.wide_bloom);
}

#[test]
fn automatic_scan_finds_a_coherent_purple_highlight_fringe() {
    let Some((ctx, mut renderer)) = setup() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    renderer
        .open_image(&purple_highlight_edge())
        .expect("open synthetic fringe");
    assert!(renderer
        .render_for_fringe_analysis(&EditState::default())
        .is_none());
    ctx.device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("wait for analysis");
    let histogram = renderer.poll_histogram().expect("analysis histogram");
    let recommendation = sp_pipeline::auto_defringe::recommend(&histogram)
        .expect("coherent highlight fringe should be detected");
    assert!(recommendation.purple > 0.0);
    assert_eq!(recommendation.green, 0.0);
}

#[test]
fn automatic_scan_separates_green_fringe_and_protects_flat_subject_color() {
    let Some((ctx, mut renderer)) = setup() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    renderer
        .open_image(&green_highlight_edge())
        .expect("open synthetic green fringe");
    assert!(renderer
        .render_for_fringe_analysis(&EditState::default())
        .is_none());
    ctx.device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("wait for green analysis");
    let histogram = renderer.poll_histogram().expect("green histogram");
    let recommendation = sp_pipeline::auto_defringe::recommend(&histogram)
        .expect("coherent green highlight fringe should be detected");
    assert_eq!(recommendation.purple, 0.0);
    assert!(recommendation.green > 0.0);

    renderer
        .open_image(&solid_purple_subject())
        .expect("open flat purple subject");
    assert!(renderer
        .render_for_fringe_analysis(&EditState::default())
        .is_none());
    ctx.device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("wait for subject-color analysis");
    let histogram = renderer.poll_histogram().expect("subject histogram");
    assert!(
        sp_pipeline::auto_defringe::recommend(&histogram).is_none(),
        "flat real purple color without a touching highlight edge must be protected"
    );
}

#[test]
fn shading_gains_are_indexed_by_quad_slot() {
    let Some((ctx, mut renderer)) = setup() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    // On an RGGB sensor, slot 0 is red. Boosting only slot 0 must raise
    // red and leave green and blue alone — the check that the shader's
    // (y&1)*2 + (x&1) indexing agrees with how a grid is stored.
    let geom = geometry();
    let mut profile = LensProfile::identity("Slot test", "slottest", geom);
    for cell in &mut profile.apertures[0].gains {
        *cell = [1.5, 1.0, 1.0, 1.0];
    }
    let raw = flat();

    renderer.set_lens_profile(None, None);
    let before = render_rgb(&ctx, &mut renderer, &raw, &EditState::default());
    renderer.set_lens_profile(Some(&profile), Some(4.0));
    let after = render_rgb(&ctx, &mut renderer, &raw, &EditState::default());

    let mid = before.len() / 2;
    let (b, a) = (before[mid], after[mid]);
    assert!(
        a[0] > b[0] + 0.05,
        "slot 0 gain should raise red: {} -> {}",
        b[0],
        a[0]
    );
    assert!(
        (a[1] - b[1]).abs() < 0.02 && (a[2] - b[2]).abs() < 0.02,
        "green and blue must be untouched: {b:?} -> {a:?}"
    );
}

#[test]
fn identity_chroma_warp_is_a_no_op() {
    let Some((ctx, mut renderer)) = setup() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    // A colourful fixture, so chroma is well away from neutral and the warp
    // is actually engaged.
    let raw = colored_flat();
    let edit = EditState::default();

    let mut profile = CameraProfile::standard("test", sp_core::color::REC2020_TO_XYZ);
    renderer.set_profile(Some(profile.clone()));
    let without = render_rgb(&ctx, &mut renderer, &raw, &edit);

    profile.chroma_warp1 = Some(ChromaWarp::identity(17));
    renderer.set_profile(Some(profile));
    let with = render_rgb(&ctx, &mut renderer, &raw, &edit);

    // Round-tripping through the lookup and back out of the luma equation
    // must not move the pixel by more than 8-bit quantization.
    for (i, (a, b)) in without.iter().zip(&with).enumerate() {
        for c in 0..3 {
            assert!(
                (a[c] - b[c]).abs() <= 2.0 / 255.0,
                "pixel {i} channel {c}: {} vs {}",
                a[c],
                b[c]
            );
        }
    }
}

#[test]
fn chroma_warp_shifts_colour_without_shifting_luma() {
    let Some((ctx, mut renderer)) = setup() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let raw = colored_flat();
    let edit = EditState::default();

    let mut profile = CameraProfile::standard("test", sp_core::color::REC2020_TO_XYZ);
    renderer.set_profile(Some(profile.clone()));
    let before = render_rgb(&ctx, &mut renderer, &raw, &edit);

    // Push cr (the red axis) outward everywhere: more saturated red.
    let mut warp = ChromaWarp::identity(17);
    for entry in &mut warp.entries {
        entry[1] *= 1.5;
    }
    profile.chroma_warp1 = Some(warp);
    renderer.set_profile(Some(profile));
    let after = render_rgb(&ctx, &mut renderer, &raw, &edit);

    let mid = before.len() / 2;
    let (b, a) = (before[mid], after[mid]);
    // Assert on the red/green ratio rather than absolute red: the warp
    // holds luma fixed, so pushing red out necessarily pulls green in, and
    // the ratio is what "more saturated" actually means.
    let ratio = |p: [f32; 3]| p[0] / p[1].max(1e-4);
    assert!(
        ratio(a) > ratio(b) + 0.02,
        "a positive cr warp should saturate red: {b:?} -> {a:?}"
    );

    // Luma is solved back out of the luma equation, so the stage cannot
    // change brightness. Compare Rec.2020 luma of the encoded output —
    // the tone curve is monotonic and shared, so equal scene luma stays
    // equal here.
    let luma = |p: [f32; 3]| {
        let l = sp_core::color::REC2020_LUMA;
        l[0] * p[0] + l[1] * p[1] + l[2] * p[2]
    };
    assert!(
        (luma(a) - luma(b)).abs() < 0.02,
        "luma must be preserved: {} -> {}",
        luma(b),
        luma(a)
    );
}

/// Build a warp that is the identity everywhere except on one half of one
/// chromaticity axis, where it pushes red up by `push`.
///
/// `on_red` selects which coordinate the half-plane is cut along. The CPU
/// mirror and the shader must agree on that choice, on the index order and
/// on where neutral sits; if any of the three disagreed, the two variants
/// below would swap their effects.
fn half_plane_warp(size: u32, on_red: bool, push: f32) -> ChromaWarp {
    let mut warp = ChromaWarp::identity(size);
    for b in 0..size {
        for r in 0..size {
            let i = (b * size + r) as usize;
            let node = if on_red { r } else { b } as f32 / (size - 1) as f32;
            if node > sp_core::profile::CHROMA_NEUTRAL {
                warp.entries[i][0] = (warp.entries[i][0] + push).clamp(0.0, 1.0);
                warp.entries[i][1] = warp.entries[i][1].min(1.0 - warp.entries[i][0]);
            }
        }
    }
    warp
}

#[test]
fn cpu_chroma_warp_mirror_agrees_with_the_shader_conventions() {
    let Some((ctx, mut renderer)) = setup() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    // The Reference Match fitter optimizes against `apply_chroma_warp` on the
    // CPU. If that drifts from the shader — swapped axes, transposed index
    // order, a different neutral point — every fitted profile would be
    // tuned for a pipeline that does not exist, and nothing else in the
    // suite would notice.
    //
    // `colored_flat` is red-rich and blue-poor, so its chromaticity sits
    // above neutral in red and below it in blue. A warp gated on the red
    // axis must therefore fire, and one gated on blue must not.
    let raw = colored_flat();
    let edit = EditState::default();
    let base = CameraProfile::standard("test", sp_core::color::REC2020_TO_XYZ);

    renderer.set_profile(Some(base.clone()));
    let plain = render_rgb(&ctx, &mut renderer, &raw, &edit);
    let scene = {
        // Sanity-check the fixture really is on the side of neutral this
        // test assumes, using the CPU mirror's own coordinates.
        let p = plain[plain.len() / 2];
        let sum = p[0] + p[1] + p[2];
        (p[0] / sum, p[2] / sum)
    };
    assert!(
        scene.0 > sp_core::profile::CHROMA_NEUTRAL && scene.1 < sp_core::profile::CHROMA_NEUTRAL,
        "fixture chromaticity {scene:?} is not where this test assumes"
    );

    let mut on_red = base.clone();
    on_red.chroma_warp1 = Some(half_plane_warp(17, true, 0.08));
    renderer.set_profile(Some(on_red));
    let red_gated = render_rgb(&ctx, &mut renderer, &raw, &edit);

    let mut on_blue = base;
    on_blue.chroma_warp1 = Some(half_plane_warp(17, false, 0.08));
    renderer.set_profile(Some(on_blue));
    let blue_gated = render_rgb(&ctx, &mut renderer, &raw, &edit);

    let (p, r, b) = (
        plain[plain.len() / 2],
        red_gated[red_gated.len() / 2],
        blue_gated[blue_gated.len() / 2],
    );
    assert!(
        r[0] > p[0] + 0.01,
        "a warp gated on the red axis must fire on a red-rich fixture: {p:?} -> {r:?}"
    );
    assert!(
        (b[0] - p[0]).abs() <= 2.0 / 255.0,
        "a warp gated on the blue axis must not fire on a blue-poor fixture: {p:?} -> {b:?}"
    );

    // And the CPU mirror reaches the same verdict on the same warps.
    let cpu_plain = [0.30_f32, 0.22, 0.16];
    let cpu_red =
        sp_pipeline::match_fit::apply_chroma_warp(&half_plane_warp(17, true, 0.08), cpu_plain);
    let cpu_blue =
        sp_pipeline::match_fit::apply_chroma_warp(&half_plane_warp(17, false, 0.08), cpu_plain);
    assert!(
        cpu_red[0] > cpu_plain[0] * 1.02,
        "CPU mirror disagrees on the red gate: {cpu_plain:?} -> {cpu_red:?}"
    );
    assert!(
        (cpu_blue[0] - cpu_plain[0]).abs() < 1e-4,
        "CPU mirror disagrees on the blue gate: {cpu_plain:?} -> {cpu_blue:?}"
    );
}

#[test]
fn chroma_warp_leaves_neutrals_alone() {
    let Some((ctx, mut renderer)) = setup() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    // Neutral fixture: WB gains of 1 on a flat field put every pixel at
    // chroma ~0, inside the gray fade, so even an aggressive warp must not
    // tint it. This is the guard against coloured mottle in shadows.
    let raw = flat();
    let edit = EditState::default();

    let mut profile = CameraProfile::standard("test", sp_core::color::REC2020_TO_XYZ);
    renderer.set_profile(Some(profile.clone()));
    let before = render_rgb(&ctx, &mut renderer, &raw, &edit);

    let mut warp = ChromaWarp::identity(17);
    for entry in &mut warp.entries {
        entry[0] += 0.4;
        entry[1] += 0.4;
    }
    profile.chroma_warp1 = Some(warp);
    renderer.set_profile(Some(profile));
    let after = render_rgb(&ctx, &mut renderer, &raw, &edit);

    let mid = before.len() / 2;
    let (b, a) = (before[mid], after[mid]);
    for c in 0..3 {
        assert!(
            (a[c] - b[c]).abs() <= 3.0 / 255.0,
            "neutral pixel channel {c} moved: {} -> {}",
            b[c],
            a[c]
        );
    }
}
