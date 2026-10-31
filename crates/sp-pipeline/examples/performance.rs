//! Offline, deterministic timings. Run alone, without competing GPU jobs:
//! cargo run -p sp-pipeline --release --example performance
//! SHOPPHOTO_EXPORT_READBACK_SLOTS=1 repeats exports with serial transfers.
use sp_core::{
    raw::{ActiveArea, Orientation},
    CfaPattern, EditState, RawImage,
};
use sp_export::{ExportImage, ExportSettings, SizeMode};
use sp_gpu::GpuContext;
use sp_pipeline::GpuRenderer;
use std::{hint::black_box, sync::atomic::AtomicBool, time::Instant};

fn measure<T>(name: &str, mut action: impl FnMut() -> T) {
    black_box(action());
    let mut times = Vec::new();
    for _ in 0..5 {
        let start = Instant::now();
        let result = action();
        times.push(start.elapsed().as_secs_f64());
        black_box(result);
    }
    times.sort_by(f64::total_cmp);
    println!("{name}: median {:.6} s; samples {times:?}", times[2]);
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (width, height) = (1200, 800);
    let image = ExportImage::new(
        width,
        height,
        (0..width * height * 4)
            .map(|i| {
                if i % 4 == 3 {
                    255
                } else {
                    ((u64::from(i) * 127) % 256) as u8
                }
            })
            .collect(),
    )
    .ok_or("fixture")?;
    let settings = ExportSettings {
        size: SizeMode::LongEdgePx(600),
        ..Default::default()
    };
    measure("1200x800 -> 600x400 preparation", || {
        sp_export::prepare_image_already_converted(&image, Default::default(), &settings)
    });
    let large =
        ExportImage::new(4000, 3000, [97, 113, 151, 255].repeat(12_000_000)).ok_or("fixture")?;
    measure("12MP neutral borrowed preparation", || {
        sp_export::prepare_image_already_converted(&large, Default::default(), &Default::default())
    });
    let large = sp_export::prepare_image_already_converted_owned(
        large,
        Default::default(),
        &Default::default(),
    );
    assert_eq!(large.rgba8, [97, 113, 151, 255].repeat(12_000_000));

    let ctx = GpuContext::headless()?;
    let mut renderer = GpuRenderer::new(ctx.clone())?;
    let (width, height) = (4018, 3000);
    let raw = RawImage {
        width,
        height,
        cfa_data: (0..width * height)
            .map(|i| (8192 + (i * 17 % 40_000)) as u16)
            .collect(),
        cfa: CfaPattern::Rggb,
        black_levels: [0.0; 4],
        white_levels: [65535.0; 4],
        wb_coeffs: [1.0; 4],
        cam_to_xyz: sp_core::color::REC2020_TO_XYZ,
        active_area: ActiveArea {
            x: 0,
            y: 0,
            width,
            height,
        },
        orientation: Orientation::Normal,
    };
    renderer.open_image(&raw)?;
    let mut edit = EditState::default();
    edit.nr.luminance = 40.0;
    edit.nr.chroma = 35.0;
    for cached in [false, true] {
        renderer.set_preview_cache_enabled(cached);
        measure(
            if cached {
                "proxy tone changes, prefix cached"
            } else {
                "proxy tone changes, prefix uncached"
            },
            || {
                edit.tone.exposure_ev += 0.01;
                renderer.render_without_histogram(&edit);
                ctx.wait_for_gpu().expect("GPU render");
            },
        );
    }
    println!(
        "Preview cache hits/misses: {:?}",
        renderer.preview_cache_stats()
    );
    println!(
        "Readback slots: {}",
        std::env::var("SHOPPHOTO_EXPORT_READBACK_SLOTS").unwrap_or_else(|_| "2".into())
    );
    for precision in [false, true] {
        measure(
            if precision {
                "12MP tiled 16-bit export"
            } else {
                "12MP tiled 8-bit export"
            },
            || {
                renderer
                    .render_export(
                        &edit,
                        false,
                        precision,
                        sp_export::ExportColorSpace::Srgb,
                        &AtomicBool::new(false),
                        |_| {},
                    )
                    .expect("export")
            },
        );
    }
    Ok(())
}
