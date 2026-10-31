//! Performance budget assertions (BUILD.md §7, Phase 8).
//!
//! The phase docs quote hand-measured timings, which means a 2× regression
//! can land without anything failing. This test measures the same
//! operations `benches/render.rs` reports on and asserts ceilings.
//!
//! Two tiers, because one number cannot serve both a Mac and a CI runner:
//!
//! * default — generous ceilings any real GPU should clear with room to
//!   spare. Catches order-of-magnitude regressions (a lost pass cull, an
//!   accidental full-res render in the interactive path, a synchronous
//!   readback per frame).
//! * `SP_PERF_STRICT=1` — roughly 2× what the development machine (M-series
//!   Mac) achieves, cross-checked against `cargo bench -p sp-pipeline`.
//!   Run before recording timings in a phase doc.
//!
//! Skipped entirely when there is no adapter, and on software rasterizers
//! (lavapipe/WARP), where absolute numbers are meaningless. Timings exclude
//! first-run pipeline warm-up and wait for the GPU, not just for submit.

use sp_core::raw::{ActiveArea, CfaPattern, Orientation};
use sp_core::{EditState, RawImage};
use sp_gpu::GpuContext;
use sp_pipeline::GpuRenderer;
use std::time::{Duration, Instant};

const W: u32 = 3008;
const H: u32 = 2000;
/// Timed repeats; the median is compared against the budget.
const REPEATS: usize = 5;

struct Budget {
    /// Operation name, also used in the failure message.
    name: &'static str,
    relaxed_ms: u128,
    strict_ms: u128,
}

const BUDGETS: &[Budget] = &[
    Budget {
        name: "open_image",
        relaxed_ms: 400,
        strict_ms: 20,
    },
    Budget {
        name: "proxy_render",
        relaxed_ms: 150,
        strict_ms: 10,
    },
    Budget {
        name: "proxy_render_nr",
        relaxed_ms: 400,
        strict_ms: 25,
    },
    Budget {
        name: "tile_1to1_nr",
        relaxed_ms: 600,
        strict_ms: 25,
    },
    // CPU side of a proxy render only: uniform buffers, bind groups, command
    // encoding and submit, without waiting for the GPU. This is what a
    // slider drag costs the UI thread per tick, so it is the number that
    // decides whether persistent buffers (review item PERF-01) are worth it.
    Budget {
        name: "proxy_render_cpu",
        relaxed_ms: 40,
        strict_ms: 4,
    },
];

fn budget(name: &str) -> &'static Budget {
    BUDGETS
        .iter()
        .find(|b| b.name == name)
        .expect("budget declared for every measured operation")
}

fn noisy_raw() -> RawImage {
    let mut state = 0x1234_5678u32;
    let mut cfa = vec![0u16; (W * H) as usize];
    for (i, site) in cfa.iter_mut().enumerate() {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let noise = (state >> 16) as f32 / 65_535.0 - 0.5;
        let base = 0.2 + 0.5 * ((i as u32 % W) as f32 / W as f32);
        *site = ((base + noise * 0.05).clamp(0.0, 1.0) * 65_535.0) as u16;
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

/// Block until the GPU has retired everything submitted so far. Without
/// this the renderer's `submit` returns immediately and every measurement
/// below reports command-encoding time only (~0 ms).
fn sync(ctx: &GpuContext) {
    ctx.device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll");
}

/// Median wall-clock of `REPEATS` runs, after one warm-up run.
fn median_ms(mut op: impl FnMut()) -> u128 {
    op();
    let mut samples: Vec<Duration> = (0..REPEATS)
        .map(|_| {
            let start = Instant::now();
            op();
            start.elapsed()
        })
        .collect();
    samples.sort();
    samples[REPEATS / 2].as_millis()
}

fn check(name: &str, strict: bool, measured: u128) {
    let budget = budget(name);
    let ceiling = if strict {
        budget.strict_ms
    } else {
        budget.relaxed_ms
    };
    let tier = if strict { "strict" } else { "relaxed" };
    eprintln!("{name}: {measured} ms (budget {ceiling} ms, {tier})");
    assert!(
        measured <= ceiling,
        "{name} took {measured} ms, over the {tier} budget of {ceiling} ms"
    );
}

#[test]
fn pipeline_stays_within_budget() {
    let Ok(ctx) = GpuContext::headless() else {
        eprintln!("no GPU adapter — skipping perf budget");
        return;
    };
    if ctx.is_software() {
        eprintln!("software adapter — skipping perf budget");
        return;
    }
    let strict = std::env::var_os("SP_PERF_STRICT").is_some();
    let gpu = ctx.clone();
    let mut renderer = GpuRenderer::new(ctx).expect("renderer");
    let raw = noisy_raw();
    let neutral = EditState::default();
    let mut noisy = EditState::default();
    noisy.nr.luminance = 50.0;
    noisy.nr.chroma = 50.0;

    check(
        "open_image",
        strict,
        median_ms(|| {
            renderer.open_image(&raw).expect("open");
            sync(&gpu);
        }),
    );
    check(
        "proxy_render",
        strict,
        median_ms(|| {
            renderer.render(&neutral);
            sync(&gpu);
        }),
    );
    check(
        "proxy_render_nr",
        strict,
        median_ms(|| {
            renderer.render(&noisy);
            sync(&gpu);
        }),
    );
    check(
        "tile_1to1_nr",
        strict,
        median_ms(|| {
            renderer.render_tile(&noisy, 512, 512, 1024, 1024);
            sync(&gpu);
        }),
    );
    // Drain the queue first so the CPU number does not include waiting for
    // a busy GPU inside submit.
    sync(&gpu);
    let cpu_us = median_us(|| {
        renderer.render_without_histogram(&noisy);
    });
    sync(&gpu);
    eprintln!(
        "proxy_render_cpu: {} us per render on the calling thread",
        cpu_us
    );
    check("proxy_render_cpu", strict, cpu_us.div_ceil(1000));
}

/// Median wall-clock in microseconds of `REPEATS` runs, after a warm-up.
fn median_us(mut op: impl FnMut()) -> u128 {
    op();
    let mut samples: Vec<Duration> = (0..REPEATS)
        .map(|_| {
            let start = Instant::now();
            op();
            start.elapsed()
        })
        .collect();
    samples.sort();
    samples[REPEATS / 2].as_micros()
}

#[test]
fn every_budget_is_reachable() {
    // A typo in a name would silently skip a budget; every declared budget
    // must be asserted by the test above.
    for b in BUDGETS {
        assert!(b.strict_ms <= b.relaxed_ms, "{} tiers inverted", b.name);
        assert!(b.strict_ms > 0, "{} strict budget is zero", b.name);
    }
    assert_eq!(BUDGETS.len(), 5, "update the budget test when adding one");
}
