//! Criterion benches for the develop pipeline (BUILD.md §7).
//!
//! These measure the paths whose cost the phase docs quote by hand:
//! image open (CFA upload + proxy build), an interactive proxy re-render,
//! a 1:1 tile with and without the noise passes, and the CPU tone-LUT
//! build. Run with:
//!
//! ```sh
//! cargo bench -p sp-pipeline
//! ```
//!
//! Benches report; they never fail. The pass/fail counterpart lives in
//! `tests/perf_budget.rs`, which asserts ceilings on the same operations.
//!
//! GPU benches are skipped (with a message) when no adapter is available,
//! so this file stays runnable on headless CI.

use criterion::{criterion_group, criterion_main, Criterion};
use sp_core::edit::{CurveParams, ToneParams};
use sp_core::raw::{ActiveArea, CfaPattern, Orientation};
use sp_core::{EditState, RawImage};
use sp_gpu::GpuContext;
use sp_pipeline::tone::{build_tone_lut, standard_base_curve};
use sp_pipeline::GpuRenderer;
use std::hint::black_box;

/// Bench sensor size. Small enough that a criterion sample set finishes in
/// seconds, large enough that per-dispatch overhead is not the whole story.
const BENCH_W: u32 = 3008;
const BENCH_H: u32 = 2000;

/// Deterministic noisy CFA so the NR passes have real work to do.
fn bench_raw(w: u32, h: u32) -> RawImage {
    let mut state = 0x1234_5678u32;
    let mut cfa = vec![0u16; (w * h) as usize];
    for (i, site) in cfa.iter_mut().enumerate() {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let noise = (state >> 16) as f32 / 65_535.0 - 0.5;
        let base = 0.2 + 0.5 * ((i as u32 % w) as f32 / w as f32);
        *site = ((base + noise * 0.05).clamp(0.0, 1.0) * 65_535.0) as u16;
    }
    RawImage {
        cfa_data: cfa,
        width: w,
        height: h,
        cfa: CfaPattern::Rggb,
        black_levels: [0.0; 4],
        white_levels: [65_535.0; 4],
        wb_coeffs: [1.0, 1.0, 1.0, 1.0],
        cam_to_xyz: sp_core::color::REC2020_TO_XYZ,
        active_area: ActiveArea {
            x: 0,
            y: 0,
            width: w,
            height: h,
        },
        orientation: Orientation::Normal,
    }
}

fn nr_edit() -> EditState {
    let mut edit = EditState::default();
    edit.nr.luminance = 50.0;
    edit.nr.chroma = 50.0;
    edit
}

/// Block until the GPU retires the submitted work. `render`/`open_image`
/// only encode and submit, so without this every bench below would measure
/// command encoding instead of the pipeline.
fn sync(ctx: &GpuContext) {
    ctx.device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll");
}

fn gpu_benches(c: &mut Criterion) {
    let Ok(ctx) = GpuContext::headless() else {
        eprintln!("no GPU adapter available — skipping GPU benches");
        return;
    };
    let gpu = ctx.clone();
    let mut renderer = GpuRenderer::new(ctx).expect("renderer");
    let raw = bench_raw(BENCH_W, BENCH_H);
    let neutral = EditState::default();
    let noisy = nr_edit();

    let mut group = c.benchmark_group("gpu");
    group.sample_size(20);

    group.bench_function("open_image", |b| {
        b.iter(|| {
            renderer.open_image(black_box(&raw)).expect("open");
            sync(&gpu);
        });
    });

    renderer.open_image(&raw).expect("open");
    group.bench_function("proxy_render", |b| {
        b.iter(|| {
            black_box(renderer.render(black_box(&neutral)));
            sync(&gpu);
        });
    });
    group.bench_function("proxy_render_nr", |b| {
        b.iter(|| {
            black_box(renderer.render(black_box(&noisy)));
            sync(&gpu);
        });
    });
    group.bench_function("tile_1to1", |b| {
        b.iter(|| {
            black_box(renderer.render_tile(black_box(&neutral), 512, 512, 1024, 1024));
            sync(&gpu);
        });
    });
    // Exercises nr_raw (pre-demosaic) + nr_main (post-demosaic) together.
    group.bench_function("tile_1to1_nr", |b| {
        b.iter(|| {
            black_box(renderer.render_tile(black_box(&noisy), 512, 512, 1024, 1024));
            sync(&gpu);
        });
    });
    group.finish();
}

fn cpu_benches(c: &mut Criterion) {
    let base = standard_base_curve();
    let tone = ToneParams {
        exposure_ev: 0.5,
        contrast: 20.0,
        highlights: -30.0,
        shadows: 25.0,
        ..Default::default()
    };
    let curve = CurveParams::default();
    c.bench_function("build_tone_lut", |b| {
        b.iter(|| {
            black_box(build_tone_lut(
                black_box(&tone),
                black_box(&curve),
                Some(&base),
            ))
        });
    });
}

criterion_group!(benches, gpu_benches, cpu_benches);
criterion_main!(benches);
