//! Sony ARW decode verification harness.
//!
//! Point `SP_TEST_SONY_RAWS` at a local folder of `.ARW` samples. The suite
//! skips when unset so large camera originals never need to enter the repo.

use std::path::PathBuf;

fn sample_files() -> Option<Vec<PathBuf>> {
    let dir = std::env::var_os("SP_TEST_SONY_RAWS")?;
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .expect("SP_TEST_SONY_RAWS must be a readable directory")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("arw"))
        })
        .collect();
    files.sort();
    Some(files)
}

#[test]
fn decode_all_sony_arw_samples() {
    let Some(files) = sample_files() else {
        eprintln!("SP_TEST_SONY_RAWS unset — skipping Sony ARW verification harness");
        return;
    };
    assert!(
        !files.is_empty(),
        "SP_TEST_SONY_RAWS folder contains no ARW files"
    );

    for path in files {
        assert!(sp_raw::is_supported(&path));
        let output = sp_raw::decode(&path)
            .unwrap_or_else(|error| panic!("decode failed for {}: {error}", path.display()));
        let raw = &output.raw;
        assert!(
            output
                .meta
                .camera_make
                .to_ascii_lowercase()
                .contains("sony"),
            "{}: unexpected make {:?}",
            path.display(),
            output.meta.camera_make
        );
        assert!(raw.active_area.width > 0 && raw.active_area.height > 0);
        assert_eq!(
            raw.cfa_data.len(),
            raw.width as usize * raw.height as usize,
            "{}: decoded CFA dimensions disagree",
            path.display()
        );
        for channel in 0..4 {
            assert!(
                raw.white_levels[channel] > raw.black_levels[channel],
                "{}: invalid black/white levels {:?}/{:?}",
                path.display(),
                raw.black_levels,
                raw.white_levels
            );
        }
        assert!(
            raw.wb_coeffs
                .iter()
                .all(|value| value.is_finite() && *value > 0.0),
            "{}: invalid white balance {:?}",
            path.display(),
            raw.wb_coeffs
        );
    }
}
