//! End-to-end Ultra HDR: the container format is unit-tested inside
//! sp-export, but only a real render can show that the PQ rendition
//! actually carries headroom the SDR rendition rolled off — which is the
//! entire point of shipping a gain map.

use sp_core::raw::{ActiveArea, CfaPattern, Orientation};
use sp_core::{EditState, RawImage};
use sp_gpu::GpuContext;
use sp_pipeline::GpuRenderer;
use std::sync::atomic::AtomicBool;

const W: u32 = 128;
const H: u32 = 128;

/// Half deep midtone, half bright sensor data. Exposure in the test pushes
/// the bright half above diffuse white, creating a real scene-linear
/// highlight that SDR must roll off and HDR must keep.
fn highlight_raw() -> RawImage {
    let mut cfa = vec![0u16; (W * H) as usize];
    for y in 0..H {
        for x in 0..W {
            let v: f32 = if x < W / 2 { 0.06 } else { 0.75 };
            cfa[(y * W + x) as usize] = (v * 65_535.0) as u16;
        }
    }
    RawImage {
        cfa_data: cfa,
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

fn render(
    renderer: &mut GpuRenderer,
    edit: &EditState,
    hdr_pq: bool,
    space: sp_core::color::OutputColorSpace,
) -> sp_export::ExportImage {
    let rendered = renderer
        .render_export(edit, hdr_pq, hdr_pq, space, &AtomicBool::new(false), |_| {})
        .expect("export render");
    rendered
        .rgba16
        .map_or_else(
            || sp_export::ExportImage::new(rendered.width, rendered.height, rendered.rgba8),
            |rgba16| sp_export::ExportImage::new_16(rendered.width, rendered.height, rgba16),
        )
        .expect("valid buffer")
}

#[test]
fn gain_map_captures_headroom_the_sdr_render_lost() {
    let Ok(ctx) = GpuContext::headless() else {
        eprintln!("no GPU adapter — skipping Ultra HDR test");
        return;
    };
    let mut renderer = GpuRenderer::new(ctx).expect("renderer");
    renderer.open_image(&highlight_raw()).expect("open");

    let mut edit = EditState::default();
    edit.hdr.enabled = true;
    edit.hdr.target = sp_core::edit::HdrTarget::Hdr;
    // Put the right half at roughly 1.8× reference white while leaving the
    // left half safely below it. This exercises the shoulder rather than
    // relying on transfer-function differences near the noise floor.
    edit.tone.exposure_ev = 1.25;

    let sdr = render(
        &mut renderer,
        &edit,
        false,
        sp_core::color::OutputColorSpace::Srgb,
    );
    let hdr = render(
        &mut renderer,
        &edit,
        true,
        sp_core::color::OutputColorSpace::Rec2020,
    );
    assert_eq!((sdr.width, sdr.height), (hdr.width, hdr.height));

    let map = sp_export::gainmap::compute(&sdr, &hdr).expect("gain map");
    eprintln!(
        "gain map {}×{}, max boost {:.2} stops",
        map.width, map.height, map.params.max_log2
    );
    assert!(
        map.params.max_log2 > 0.25,
        "PQ render carried no headroom over SDR: {:.3} stops",
        map.params.max_log2
    );

    // The boost must live in the highlight half, not the mid-gray half.
    let row = (map.height / 2) as usize;
    let left = map.data[row * map.width as usize + (map.width / 4) as usize];
    let right = map.data[row * map.width as usize + (map.width * 3 / 4) as usize];
    assert!(
        right > left + 8,
        "gain is not concentrated in the highlights (left {left}, right {right})"
    );
}

#[test]
fn reference_white_is_inert_in_sdr_and_active_in_hdr() {
    let Ok(ctx) = GpuContext::headless() else {
        eprintln!("no GPU adapter — skipping HDR reference-white test");
        return;
    };
    let mut renderer = GpuRenderer::new(ctx).expect("renderer");
    renderer.open_image(&highlight_raw()).expect("open");

    let base = EditState::default();
    let sdr_base = render(
        &mut renderer,
        &base,
        false,
        sp_core::color::OutputColorSpace::Srgb,
    );
    let mut hidden_control = base.clone();
    hidden_control.hdr.scene_white_ev = 2.0;
    let sdr_changed = render(
        &mut renderer,
        &hidden_control,
        false,
        sp_core::color::OutputColorSpace::Srgb,
    );
    assert_eq!(
        sdr_base.rgba8, sdr_changed.rgba8,
        "disabled HDR reference white changed the SDR render"
    );

    let mut hdr = base;
    hdr.hdr.enabled = true;
    hdr.hdr.target = sp_core::edit::HdrTarget::Hdr;
    let hdr_base = render(
        &mut renderer,
        &hdr,
        false,
        sp_core::color::OutputColorSpace::Srgb,
    );
    hdr.hdr.scene_white_ev = 1.0;
    let hdr_shifted = render(
        &mut renderer,
        &hdr,
        false,
        sp_core::color::OutputColorSpace::Srgb,
    );
    assert_ne!(hdr_base.rgba8, hdr_shifted.rgba8);
}

#[test]
fn zero_headroom_pq_keeps_reference_white_at_203_nits() {
    let Ok(ctx) = GpuContext::headless() else {
        eprintln!("no GPU adapter — skipping PQ reference-white test");
        return;
    };
    let mut renderer = GpuRenderer::new(ctx).expect("renderer");
    renderer.open_image(&highlight_raw()).expect("open");

    let mut edit = EditState::default();
    edit.hdr.enabled = true;
    edit.hdr.target = sp_core::edit::HdrTarget::Hdr;
    edit.hdr.headroom_stops = 0.0;
    edit.tone.exposure_ev = (1.0_f32 / 0.75).log2();
    let pq = render(
        &mut renderer,
        &edit,
        true,
        sp_core::color::OutputColorSpace::Rec2020,
    );
    let values = pq.rgba16.as_ref().expect("high-precision PQ");
    let x = pq.width * 3 / 4;
    let y = pq.height / 2;
    let encoded = values[((y * pq.width + x) * 4) as usize] as f32 / 65_535.0;
    let nits = sp_core::color::pq_eotf(encoded) * 10_000.0;
    assert!(
        (201.0..=205.0).contains(&nits),
        "reference white encoded at {nits:.2} nits"
    );
}
