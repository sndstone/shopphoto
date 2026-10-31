//! Optional embedded-preview verification against a real camera file.
//!
//! Set `SP_TEST_PREVIEW_RAW` to exercise the lightweight library-thumbnail
//! path. This is especially useful with an SMB-mounted sample.

#[test]
fn decode_embedded_preview_sample() {
    let Some(path) = std::env::var_os("SP_TEST_PREVIEW_RAW") else {
        eprintln!("SP_TEST_PREVIEW_RAW unset — skipping embedded-preview harness");
        return;
    };
    let path = std::path::PathBuf::from(path);
    let started = std::time::Instant::now();
    let output = sp_raw::decode_preview(&path)
        .unwrap_or_else(|error| panic!("preview failed for {}: {error}", path.display()));
    let preview = output
        .preview
        .unwrap_or_else(|| panic!("{} has no embedded preview", path.display()));
    assert!(preview.width > 0 && preview.height > 0);
    assert!(preview.width <= 512 && preview.height <= 512);
    assert_eq!(
        preview.rgba8.len(),
        preview.width as usize * preview.height as usize * 4
    );
    eprintln!(
        "OK {}: {}x{} in {:?}",
        path.display(),
        preview.width,
        preview.height,
        started.elapsed()
    );
}
