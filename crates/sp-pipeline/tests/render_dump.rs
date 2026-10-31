//! Visual verification dump: renders a real RAW through the full proxy
//! pipeline and writes the displayable output to a PNG. Env-gated:
//! needs `SP_TEST_RAWS` (input folder) and `SP_DUMP_DIR` (output folder).
//! Not a pass/fail image test — it exists so a human (or agent) can look
//! at what the viewport shows.

use sp_core::EditState;
use sp_gpu::GpuContext;
use sp_pipeline::GpuRenderer;

fn read_output(ctx: &GpuContext, renderer: &GpuRenderer) -> (Vec<u8>, u32, u32) {
    let (tex, w, h) = renderer.output().expect("output");
    let bytes_per_row = (w * 4).div_ceil(256) * 256;
    let buf = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("dump"),
        size: u64::from(bytes_per_row * h),
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
                rows_per_image: Some(h),
            },
        },
        wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
    );
    ctx.queue.submit([enc.finish()]);
    let slice = buf.slice(..);
    slice.map_async(wgpu::MapMode::Read, |result| result.expect("map"));
    ctx.device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll");
    let data = slice.get_mapped_range();
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        let row = &data[(y * bytes_per_row) as usize..][..(w * 4) as usize];
        rgba.extend_from_slice(row);
    }
    (rgba, w, h)
}

fn save_render(
    ctx: &GpuContext,
    renderer: &GpuRenderer,
    dump_dir: &std::path::Path,
    stem: &str,
    suffix: &str,
) {
    let (rgba, w, h) = read_output(ctx, renderer);
    let out = dump_dir.join(format!("{stem}_{suffix}.png"));
    image::save_buffer(&out, &rgba, w, h, image::ColorType::Rgba8).expect("png");
    eprintln!("wrote {} ({w}×{h})", out.display());
}

#[test]
fn dump_rendered_proxies() {
    let (Some(raw_dir), Some(dump_dir)) = (
        std::env::var_os("SP_TEST_RAWS"),
        std::env::var_os("SP_DUMP_DIR"),
    ) else {
        eprintln!("SP_TEST_RAWS / SP_DUMP_DIR unset — skipping render dump");
        return;
    };
    let Ok(ctx) = GpuContext::headless() else {
        return;
    };
    let mut renderer = GpuRenderer::new(ctx.clone()).expect("renderer");
    let dump_dir = std::path::PathBuf::from(dump_dir);
    std::fs::create_dir_all(&dump_dir).expect("dump dir");

    let mut files: Vec<_> = std::fs::read_dir(raw_dir)
        .expect("raw dir")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| sp_raw::is_supported(p))
        .collect();
    files.sort();

    for path in &files {
        let decoded = sp_raw::decode(path).expect("decode");
        eprintln!(
            "{:?}: orientation {:?}, wb {:?}, white {:?}",
            path.file_name().unwrap(),
            decoded.raw.orientation,
            decoded.raw.wb_coeffs,
            decoded.raw.white_levels
        );
        {
            // Per-channel raw stats over the active area (sampled).
            let r = &decoded.raw;
            let mut mx = [0u16; 3];
            let mut sums = [0u64; 3];
            let mut counts = [0u64; 3];
            let a = r.active_area;
            for y in (a.y..a.y + a.height).step_by(31) {
                for x in (a.x..a.x + a.width).step_by(31) {
                    let v = r.cfa_data[(y * r.width + x) as usize];
                    let c = r.cfa.color_at(x, y) as usize;
                    mx[c] = mx[c].max(v);
                    sums[c] += u64::from(v);
                    counts[c] += 1;
                }
            }
            // Sky window (upper-center-right of the upright image).
            let mut smean = [0f64; 3];
            let mut scnt = [0f64; 3];
            let (sx0, sx1) = (a.x + a.width * 55 / 100, a.x + a.width * 75 / 100);
            let (sy0, sy1) = (a.y + a.height * 25 / 100, a.y + a.height * 35 / 100);
            for y in (sy0..sy1).step_by(7) {
                for x in (sx0..sx1).step_by(7) {
                    let v = r.cfa_data[(y * r.width + x) as usize];
                    let c = r.cfa.color_at(x, y) as usize;
                    smean[c] += f64::from(v);
                    scnt[c] += 1.0;
                }
            }
            let sky = [smean[0] / scnt[0], smean[1] / scnt[1], smean[2] / scnt[2]];
            let norm = sky.map(|v| {
                (v - 4096.0_f64.min(f64::from(r.black_levels[0])))
                    / (f64::from(r.white_levels[0]) - f64::from(r.black_levels[0]))
            });
            eprintln!("  sky mean raw R/G/B = {sky:?} normalized {norm:?}");
            eprintln!(
                "  raw max R/G/B = {:?}, mean = {:?}, black {:?}, white {:?}",
                mx,
                [
                    sums[0] / counts[0].max(1),
                    sums[1] / counts[1].max(1),
                    sums[2] / counts[2].max(1)
                ],
                r.black_levels,
                r.white_levels
            );
        }
        renderer.open_image(&decoded.raw).expect("open");
        let default_edit = EditState::default();
        renderer.render(&default_edit);
        ctx.device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("poll");
        let name = path.file_stem().unwrap().to_string_lossy();
        if let Some(p) = &decoded.preview {
            let out = dump_dir.join(format!("{name}_preview.png"));
            image::save_buffer(&out, &p.rgba8, p.width, p.height, image::ColorType::Rgba8)
                .expect("preview png");
            eprintln!("wrote {}", out.display());
        }
        save_render(&ctx, &renderer, &dump_dir, &name, "default");

        if std::env::var_os("SP_DUMP_PRESETS").is_some() {
            let mut starting_edit = default_edit;
            if let Some(histogram) = renderer.poll_histogram() {
                if let Some(stats) =
                    sp_pipeline::auto_tone::HistogramStats::from_rgb_bins(&histogram)
                {
                    eprintln!(
                        "  Histogram: p0.5 {:.3}, p10 {:.3}, p50 {:.3}, p90 {:.3}, p99.5 {:.3}",
                        stats.percentile_encoded(0.005),
                        stats.percentile_encoded(0.10),
                        stats.percentile_encoded(0.50),
                        stats.percentile_encoded(0.90),
                        stats.percentile_encoded(0.995),
                    );
                    starting_edit.tone = sp_pipeline::auto_tone::auto_tone(&stats);
                    eprintln!(
                        "  Auto tone: exposure {:+.2}, brightness {:+.1}, highlights {:+.1}, shadows {:+.1}, whites {:+.1}, blacks {:+.1}",
                        starting_edit.tone.exposure_ev,
                        starting_edit.tone.brightness,
                        starting_edit.tone.highlights,
                        starting_edit.tone.shadows,
                        starting_edit.tone.whites,
                        starting_edit.tone.blacks,
                    );
                    renderer.render(&starting_edit);
                    ctx.device
                        .poll(wgpu::PollType::wait_indefinitely())
                        .expect("poll");
                    save_render(&ctx, &renderer, &dump_dir, &name, "auto-balance");
                }
            }
            for preset in sp_core::preset::DevelopPreset::ALL {
                let mut edit = starting_edit.clone();
                preset.apply(&mut edit);
                renderer.render(&edit);
                ctx.device
                    .poll(wgpu::PollType::wait_indefinitely())
                    .expect("poll");
                let suffix = preset.name().to_ascii_lowercase().replace([' ', '&'], "-");
                save_render(&ctx, &renderer, &dump_dir, &name, &suffix);
            }
        }
    }
}
