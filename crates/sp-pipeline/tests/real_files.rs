//! End-to-end timing on the user's real Hasselblad files (BUILD.md Phase 2
//! acceptance). Env-gated like the sp-raw harness: needs `SP_TEST_RAWS`
//! and a GPU; skips cleanly otherwise. Run with `--nocapture` to see times.

use sp_core::EditState;
use sp_gpu::GpuContext;
use sp_pipeline::GpuRenderer;
use std::time::Instant;

#[test]
fn open_and_render_real_files() {
    let Some(dir) = std::env::var_os("SP_TEST_RAWS") else {
        eprintln!("SP_TEST_RAWS unset — skipping real-file pipeline test");
        return;
    };
    let Ok(ctx) = GpuContext::headless() else {
        eprintln!("no GPU adapter — skipping real-file pipeline test");
        return;
    };
    let mut renderer = GpuRenderer::new(ctx.clone()).expect("renderer");

    let mut files: Vec<_> = std::fs::read_dir(dir)
        .expect("readable dir")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| sp_raw::is_supported(p))
        .collect();
    files.sort();
    assert!(!files.is_empty());

    for path in &files {
        let name = path.file_name().unwrap().to_string_lossy();

        let t0 = Instant::now();
        let decoded = sp_raw::decode(path).expect("decode");
        let t_decode = t0.elapsed();

        let t1 = Instant::now();
        renderer.open_image(&decoded.raw).expect("open");
        renderer.render(&EditState::default());
        // Force completion so the timing is honest.
        ctx.device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("poll");
        let t_gpu = t1.elapsed();

        let (_, w, h) = renderer.output().expect("output");
        eprintln!(
            "{name}: decode {} ms, gpu open+render {} ms, proxy {w}×{h}",
            t_decode.as_millis(),
            t_gpu.as_millis()
        );

        // Phase 2 acceptance: first color-correct frame < 500 ms after
        // decode (dev-build slack: assert a loose 2 s bound; release is
        // far faster and Phase 8 tightens budgets with criterion).
        assert!(t_gpu.as_millis() < 2000, "{name}: GPU path too slow");

        let mut effects = EditState::default();
        effects.local.clarity = 50.0;
        effects.local.texture = 25.0;
        effects.local.dehaze = 10.0;
        let t2 = Instant::now();
        let histogram = renderer.render(&effects);
        ctx.device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("poll local contrast");
        let t_local = t2.elapsed();
        eprintln!(
            "{name}: local-contrast proxy render {} ms",
            t_local.as_millis()
        );
        assert!(
            t_local.as_millis() < 2000,
            "{name}: local contrast too slow"
        );
        if let Some(stats) = histogram
            .as_deref()
            .and_then(sp_pipeline::auto_tone::HistogramStats::from_rgb_bins)
        {
            let automatic = sp_pipeline::auto_tone::auto_tone(&stats);
            eprintln!(
                "{name}: auto exposure {:+.2} EV, whites {:+.0}, blacks {:+.0}",
                automatic.exposure_ev, automatic.whites, automatic.blacks
            );
            assert!(automatic.exposure_ev.is_finite());
        }
    }
}
