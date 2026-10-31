//! GPU contract for display-only renders used by the Auto state machine.

use sp_core::raw::{ActiveArea, CfaPattern, Orientation};
use sp_core::{EditState, RawImage};
use sp_gpu::pipelines::HISTOGRAM_BINS;
use sp_gpu::GpuContext;
use sp_pipeline::GpuRenderer;

fn gray_raw() -> RawImage {
    const W: u32 = 64;
    const H: u32 = 64;
    RawImage {
        cfa_data: vec![24_000; (W * H) as usize],
        width: W,
        height: H,
        cfa: CfaPattern::Rggb,
        black_levels: [0.0; 4],
        white_levels: [65_535.0; 4],
        wb_coeffs: [1.0; 4],
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

#[test]
fn display_only_render_neither_consumes_nor_schedules_a_histogram() {
    let Ok(ctx) = GpuContext::headless() else {
        eprintln!("no GPU adapter - skipping histogram contract test");
        return;
    };
    let mut renderer = GpuRenderer::new(ctx.clone()).expect("renderer");
    renderer.open_image(&gray_raw()).expect("open");
    let edit = EditState::default();

    assert!(renderer.render(&edit).is_none());
    assert!(renderer.histogram_pending());
    ctx.device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("wait for histogram map");

    renderer.render_without_histogram(&edit);
    assert!(
        renderer.histogram_pending(),
        "display-only render consumed Auto's private readback"
    );
    let histogram = renderer.poll_histogram().expect("completed histogram");
    assert_eq!(histogram.len(), HISTOGRAM_BINS);
    assert!(!renderer.histogram_pending());

    renderer.render_without_histogram(&edit);
    assert!(
        !renderer.histogram_pending(),
        "display-only render queued a competing live histogram"
    );
    assert!(renderer.poll_histogram().is_none());
}

#[test]
fn explicit_fringe_analysis_populates_score_planes_without_false_positive_on_gray() {
    let Ok(ctx) = GpuContext::headless() else {
        eprintln!("no GPU adapter - skipping fringe histogram test");
        return;
    };
    let mut renderer = GpuRenderer::new(ctx.clone()).expect("renderer");
    renderer.open_image(&gray_raw()).expect("open");
    let edit = EditState::default();

    assert!(renderer.render_for_fringe_analysis(&edit).is_none());
    ctx.device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("wait for fringe histogram map");
    let histogram = renderer.poll_histogram().expect("fringe histogram");
    assert_eq!(histogram.len(), HISTOGRAM_BINS);
    let purple_total: u64 = histogram[1024..1280]
        .iter()
        .map(|&count| u64::from(count))
        .sum();
    let green_total: u64 = histogram[1280..1536]
        .iter()
        .map(|&count| u64::from(count))
        .sum();
    let purple_width_total: u64 = histogram[1536..1792]
        .iter()
        .map(|&count| u64::from(count))
        .sum();
    let green_width_total: u64 = histogram[1792..2048]
        .iter()
        .map(|&count| u64::from(count))
        .sum();
    assert_eq!(purple_total, 16 * 16);
    assert_eq!(green_total, 16 * 16);
    assert_eq!(purple_width_total, 16 * 16);
    assert_eq!(green_width_total, 16 * 16);
    assert!(sp_pipeline::auto_defringe::recommend(&histogram).is_none());
}
