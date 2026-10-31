use std::{path::Path, time::Instant};

fn roughness(data: &[u16], width: usize, area: sp_core::raw::ActiveArea) -> f64 {
    let mut total = 0f64;
    let mut n = 0u64;
    // Same-colour two-pixel differences avoid treating the Bayer pattern as noise.
    for y in (area.y as usize..(area.y + area.height) as usize).step_by(8) {
        for x in (area.x as usize..(area.x + area.width - 2) as usize).step_by(8) {
            total += (data[y * width + x] as f64 - data[y * width + x + 2] as f64).abs();
            n += 1;
        }
    }
    total / n.max(1) as f64
}

fn main() {
    let raw_path = std::env::args()
        .nth(1)
        .expect("usage: eval_raw RAW [REFERENCE_RAW]");
    let root = sp_ai::model_root().expect("set SHOPPHOTO_DENOISE_MODELS");
    let decoded = sp_raw::decode(Path::new(&raw_path)).expect("decode input RAW");
    println!(
        "camera={} {} iso={:?} model={} frame={}x{} active={:?} cfa={:?} black={:?} white={:?}",
        decoded.meta.camera_make,
        decoded.meta.camera_model,
        decoded.meta.iso,
        sp_ai::model_path(&root).display(),
        decoded.raw.width,
        decoded.raw.height,
        decoded.raw.active_area,
        decoded.raw.cfa,
        decoded.raw.black_levels,
        decoded.raw.white_levels
    );
    let before = roughness(
        &decoded.raw.cfa_data,
        decoded.raw.width as usize,
        decoded.raw.active_area,
    );
    let started = Instant::now();
    let strength: f32 = std::env::var("SHOPPHOTO_AI_STRENGTH")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.75);
    let output = sp_ai::denoise(&decoded.raw, &root, strength).expect("AI denoise");
    // Raw little-endian u16 frame for comparisons with other tools.
    if let Some(dump) = std::env::var_os("SHOPPHOTO_AI_DUMP") {
        let bytes: Vec<u8> = output
            .cfa_data
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        std::fs::write(dump, bytes).expect("write SHOPPHOTO_AI_DUMP");
    }
    let after = roughness(&output.cfa_data, output.width as usize, output.active_area);
    let mut changed = 0u64;
    let mut abs = 0f64;
    let mut max = 0u16;
    for (&a, &b) in decoded.raw.cfa_data.iter().zip(&output.cfa_data) {
        let d = a.abs_diff(b);
        if d != 0 {
            changed += 1
        };
        abs += d as f64;
        max = max.max(d);
    }
    println!("elapsed_s={:.2} changed={} mean_abs_delta={:.3} max_delta={} roughness_before={:.3} roughness_after={:.3} ratio={:.4}", started.elapsed().as_secs_f64(),changed,abs/decoded.raw.cfa_data.len() as f64,max,before,after,after/before);
    if let Some(reference) = std::env::args().nth(2) {
        let reference = sp_raw::decode(Path::new(&reference)).expect("decode reference RAW");
        println!(
            "reference frame={}x{} active={:?} cfa={:?} black={:?} white={:?}",
            reference.raw.width,
            reference.raw.height,
            reference.raw.active_area,
            reference.raw.cfa,
            reference.raw.black_levels,
            reference.raw.white_levels
        );
        if reference.raw.cfa_data.len() == output.cfa_data.len() {
            let mut base = 0f64;
            let mut ai = 0f64;
            let mut dot = 0f64;
            let mut aa = 0f64;
            let mut bb = 0f64;
            for ((&src, &got), &want) in decoded
                .raw
                .cfa_data
                .iter()
                .zip(&output.cfa_data)
                .zip(&reference.raw.cfa_data)
            {
                base += src.abs_diff(want) as f64;
                ai += got.abs_diff(want) as f64;
                let a = got as f64 - src as f64;
                let b = want as f64 - src as f64;
                dot += a * b;
                aa += a * a;
                bb += b * b;
            }
            let rr = roughness(
                &reference.raw.cfa_data,
                reference.raw.width as usize,
                reference.raw.active_area,
            );
            println!("reference_mae_before={:.3} reference_mae_after={:.3} ratio={:.4} reference_roughness={:.3} delta_cosine={:.4}",base/output.cfa_data.len() as f64,ai/output.cfa_data.len() as f64,ai/base,rr,dot/(aa*bb).sqrt());
        } else {
            println!("reference_dimensions_do_not_match");
        }
    }
}
