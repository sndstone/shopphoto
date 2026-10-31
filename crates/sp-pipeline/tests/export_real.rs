//! Opt-in real-file acceptance for the exact Bayer 2× export path.

use sp_core::edit::ResolutionMode;
use sp_gpu::GpuContext;
use sp_pipeline::GpuRenderer;
use std::sync::atomic::AtomicBool;

#[test]
fn super2_export_real_file() {
    let Some(dir) = std::env::var_os("SP_TEST_RAWS") else {
        eprintln!("SP_TEST_RAWS unset — skipping real export test");
        return;
    };
    let Some(path) = std::fs::read_dir(dir)
        .expect("sample directory")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| sp_raw::is_supported(path))
        .min()
    else {
        panic!("SP_TEST_RAWS contains no supported file");
    };
    let Ok(ctx) = GpuContext::headless() else {
        eprintln!("no GPU adapter — skipping real export test");
        return;
    };
    let decoded = sp_raw::decode(&path).expect("decode");
    let mut renderer = GpuRenderer::new(ctx).expect("renderer");
    renderer.open_image(&decoded.raw).expect("open");
    let mut edit = sp_core::EditState::default();
    edit.resolution.mode = ResolutionMode::Super2x2;
    let started = std::time::Instant::now();
    let rendered = renderer
        .render_export(
            &edit,
            false,
            true,
            sp_core::color::OutputColorSpace::Srgb,
            &AtomicBool::new(false),
            |_| {},
        )
        .expect("export render");
    assert_eq!(rendered.width, decoded.raw.active_area.width / 2);
    assert_eq!(rendered.height, decoded.raw.active_area.height / 2);
    assert_eq!(
        rendered.rgba8.len(),
        rendered.width as usize * rendered.height as usize * 4
    );
    assert_eq!(
        rendered.rgba16.as_ref().map(Vec::len),
        Some(rendered.width as usize * rendered.height as usize * 4)
    );
    eprintln!(
        "{}: 2× export rendered in {} ms at {}×{}",
        path.display(),
        started.elapsed().as_millis(),
        rendered.width,
        rendered.height
    );
}
