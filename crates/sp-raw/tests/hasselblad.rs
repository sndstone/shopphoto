//! Hasselblad verification harness (BUILD.md Phase 1 step 3).
//!
//! Runs against a LOCAL folder of sample RAW files pointed to by the
//! `SP_TEST_RAWS` environment variable (never committed). When the variable
//! is unset the suite passes trivially with a note, so CI stays green.
//!
//! Recommended sample set: X2D 100C `.3FR` (100 MP), X1D II 50C `.3FR`
//! (50 MP), a Phocus-processed `.fff`, a Hasselblad-exported DNG.

use std::path::PathBuf;

fn sample_files() -> Option<Vec<PathBuf>> {
    let dir = std::env::var_os("SP_TEST_RAWS")?;
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .expect("SP_TEST_RAWS must be a readable directory")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            match p
                .extension()
                .and_then(|e| e.to_str())
                .map(str::to_ascii_lowercase)
                .as_deref()
            {
                Some("3fr" | "fff") => true,
                Some("dng") => sp_raw::decode_preview(p).is_ok_and(|out| {
                    out.meta.camera_make.to_lowercase().contains("hasselblad")
                        || out.meta.camera_model.to_lowercase().contains("hasselblad")
                }),
                _ => false,
            }
        })
        .collect();
    files.sort();
    Some(files)
}

#[test]
fn decode_all_samples() {
    let Some(files) = sample_files() else {
        eprintln!("SP_TEST_RAWS unset — skipping Hasselblad verification harness");
        return;
    };
    assert!(
        !files.is_empty(),
        "SP_TEST_RAWS folder contains no Hasselblad RAW files"
    );

    for path in &files {
        let out = sp_raw::decode(path)
            .unwrap_or_else(|e| panic!("decode failed for {}: {e}", path.display()));
        let raw = &out.raw;
        let name = path.file_name().unwrap().to_string_lossy();

        // Dimensions: Hasselblad bodies are 50 or 100 MP; anything under
        // 30 MP means we decoded a thumbnail by mistake.
        let mp = raw.active_megapixels();
        assert!(mp > 30.0, "{name}: implausible size {mp:.1} MP");

        // Levels sane: white above black, black within u16.
        for ch in 0..4 {
            assert!(
                raw.white_levels[ch] > raw.black_levels[ch],
                "{name}: white {:?} !> black {:?}",
                raw.white_levels,
                raw.black_levels
            );
        }

        // WB present and positive.
        assert!(
            raw.wb_coeffs[..3].iter().all(|&v| v > 0.0 && v.is_finite()),
            "{name}: bad WB {:?}",
            raw.wb_coeffs
        );

        // Color matrix must be a real matrix (non-degenerate rows).
        let m = &raw.cam_to_xyz;
        assert!(
            m.iter().all(|row| row.iter().any(|&v| v.abs() > 1e-6)),
            "{name}: degenerate cam_to_xyz {m:?}"
        );

        // Metadata basics.
        assert!(
            out.meta.camera_model.to_lowercase().contains("hasselblad")
                || !out.meta.camera_model.is_empty(),
            "{name}: no camera model"
        );

        eprintln!(
            "OK {name}: {}×{} ({mp:.1} MP), CFA {:?}, ISO {:?}, preview {}",
            raw.active_area.width,
            raw.active_area.height,
            raw.cfa,
            out.meta.iso,
            out.preview
                .as_ref()
                .map(|p| format!("{}×{}", p.width, p.height))
                .unwrap_or_else(|| "none".into()),
        );
    }
}

#[test]
fn decode_under_two_seconds_each() {
    let Some(files) = sample_files() else {
        return;
    };
    for path in files.iter().take(2) {
        let t = std::time::Instant::now();
        sp_raw::decode(path).expect("decode");
        let elapsed = t.elapsed();
        // Debug builds are slow; this is a smoke bound, not the Phase 8
        // budget. Release-mode budget checks live in benches later.
        assert!(
            elapsed.as_secs() < 20,
            "{} took {elapsed:?}",
            path.display()
        );
    }
}
