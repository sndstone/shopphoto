//! ShopPhoto application entry point.
//!
//! Keep this file to startup wiring only: logging, native options, and the
//! `eframe::run_native` call. All real state lives in [`app::ShopPhotoApp`].

mod app;
mod edit_groups;
mod hdr_view;
mod i18n;
mod inference_cache;
mod jobs;
#[cfg(target_os = "macos")]
mod macos_edr;
#[cfg(target_os = "macos")]
mod macos_open;
mod panels;
mod preview_cache;
mod state;
mod theme;
mod updates;
mod watchdog;
#[cfg(target_os = "windows")]
mod windows_hdr;
mod work_queue;

use anyhow::Context;
use eframe::egui_wgpu::wgpu;

fn app_icon() -> egui::IconData {
    let rgba = image::load_from_memory(include_bytes!("../../../assets/shopphoto-256.png"))
        .expect("embedded ShopPhoto icon must be a valid PNG")
        .to_rgba8();
    egui::IconData {
        width: rgba.width(),
        height: rgba.height(),
        rgba: rgba.into_raw(),
    }
}

fn main() -> anyhow::Result<()> {
    if let Some(result) = app::cli::try_run() {
        return result;
    }
    // Opt into a float (EDR) swapchain on macOS by default — the vendored
    // egui-wgpu reads this before surface configuration. wgpu's Metal
    // backend enables CAMetalLayer extended dynamic range for Rgba16Float
    // surfaces, which is what makes HDR edits visibly brighter than UI
    // white. `SHOPPHOTO_SDR=1` forces the classic 8-bit surface.
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    if std::env::var_os("SHOPPHOTO_SDR").is_none() {
        // Set before eframe/wgpu initialize; single-threaded here.
        unsafe { std::env::set_var("SHOPPHOTO_HDR_SURFACE", "1") };
    }
    // Opt-in on Linux until measured compositor/display coverage is available.
    // wgpu's Vulkan backend exposes Rgba16Float only for advertised
    // EXTENDED_SRGB_LINEAR_EXT surfaces; the existing chooser falls back to SDR.
    #[cfg(target_os = "linux")]
    let linux_hdr_requested = std::env::var("SHOPPHOTO_LINUX_HDR").is_ok_and(|v| v == "1")
        && std::env::var_os("WAYLAND_DISPLAY").is_some()
        && std::env::var_os("SHOPPHOTO_SDR").is_none();
    #[cfg(target_os = "linux")]
    if linux_hdr_requested {
        unsafe { std::env::set_var("SHOPPHOTO_HDR_SURFACE", "1") };
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,wgpu_core=warn,wgpu_hal=warn".into()),
        )
        .init();

    // Raise the texture-size limit to what the adapter actually supports:
    // eframe's default caps 2D textures at 8192, but 100 MP Hasselblad
    // sensors are 11904 px wide (Apple GPUs support 16384).
    let mut wgpu_setup = eframe::egui_wgpu::WgpuSetupCreateNew::without_display_handle();
    wgpu_setup.instance_descriptor.backends = sp_gpu::GpuContext::configured_backends()?;
    #[cfg(target_os = "linux")]
    if linux_hdr_requested {
        wgpu_setup.instance_descriptor.backends = wgpu::Backends::VULKAN;
    }
    wgpu_setup.device_descriptor = std::sync::Arc::new(|adapter| {
        let adapter_limits = adapter.limits();
        wgpu::DeviceDescriptor {
            label: Some("shopphoto device"),
            // Native f16 for the NR/local-contrast inner loops where the
            // adapter offers it (BUILD.md §10.2). Shared with the headless
            // test device via `GpuContext::preferred_features`.
            required_features: sp_gpu::GpuContext::preferred_features(adapter.features()),
            required_limits: wgpu::Limits {
                max_texture_dimension_2d: adapter_limits.max_texture_dimension_2d,
                ..wgpu::Limits::default()
            },
            ..Default::default()
        }
    });

    let initial_size = if std::env::var_os("SP_VISUAL_REGRESSION").is_some() {
        [1600.0, 952.0]
    } else {
        [1440.0, 900.0]
    };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("ShopPhoto")
            // Use the same artwork as the .icns generated for the app/DMG.
            // eframe applies this to the macOS Dock as well as taskbars on
            // Windows and Linux, including when running the bare executable.
            .with_icon(app_icon())
            .with_inner_size(initial_size)
            .with_min_inner_size([1024.0, 640.0]),
        // The wgpu backend is required (BUILD.md §2); eframe is built with
        // only that backend enabled in Cargo.toml.
        wgpu_options: eframe::egui_wgpu::WgpuConfiguration {
            wgpu_setup: eframe::egui_wgpu::WgpuSetup::CreateNew(wgpu_setup),
            ..Default::default()
        },
        ..Default::default()
    };

    #[cfg(target_os = "macos")]
    {
        let event_loop = winit::event_loop::EventLoop::<eframe::UserEvent>::with_user_event()
            .build()
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        // Building the event loop installs winit's NSApplication delegate.
        // Hook document opening into it now, before the run loop's
        // `finishLaunching` delivers the file Finder launched us with.
        macos_open::install_delegate_hooks();
        let mut native_app = eframe::create_native(
            "ShopPhoto",
            options,
            Box::new(|cc| Ok(Box::new(app::ShopPhotoApp::new(cc)))),
            &event_loop,
        );
        event_loop
            .run_app(&mut native_app)
            .map_err(|e| anyhow::anyhow!("{e}"))
            .context("failed to start ShopPhoto window")
    }

    #[cfg(not(target_os = "macos"))]
    eframe::run_native(
        "ShopPhoto",
        options,
        Box::new(|cc| Ok(Box::new(app::ShopPhotoApp::new(cc)))),
    )
    .map_err(|e| anyhow::anyhow!("{e}"))
    .context("failed to start ShopPhoto window")
}
