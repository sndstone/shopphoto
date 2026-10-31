//! The eframe `App` implementation: frame loop, layout composition, global
//! keyboard shortcuts, and background-job plumbing. Panels live in
//! [`crate::panels`].
//!
//! egui 0.35 API: input/shortcuts and job polling happen in `App::logic`
//! (no painting allowed there); layout is composed in `App::ui` with
//! `egui::Panel`s attached to the root `Ui`, `CentralPanel` last.

use crate::jobs::{JobResult, JobSystem, PersistRequest, SidecarWrite};
use crate::panels;
use crate::state::{
    AppState, CurveChannel, CurvePipetteActive, DefringePickerTarget, Mode, ReferenceMatchReport,
};
use crate::theme;
use crossbeam_channel::Receiver;
use eframe::egui_wgpu;
use egui_wgpu::wgpu;
use sp_core::source::ImageSource;
use sp_core::EditState;
use sp_gpu::GpuContext;
use sp_pipeline::GpuRenderer;
use sp_raw::SourceOutput as DecodeOutput;
use std::path::PathBuf;

mod ai;
mod auto;
mod auto_finish;
pub(crate) mod automation;
pub(crate) mod catalog_tools;
pub(crate) mod cli;
mod config;
mod culling;
pub(crate) mod customize;
pub use culling::CullCell;
pub use shortcuts::SHORTCUTS;
mod delivery;
mod diagnostics;
mod enhance;
pub(crate) mod enhance_runtime;
mod enhance_worker;
mod export;
pub(crate) mod external;
pub(crate) mod gallery;
mod hooks;
mod imports;
pub(crate) mod lens_manager;
mod library;
pub(crate) mod multiframe;
#[cfg(test)]
mod native_acceptance;
pub(crate) mod native_enhance;
mod presets;
mod printer;
mod proofing;
mod render;
mod results;
mod retouch;
pub(crate) mod sequence;
mod shortcuts;
mod sidecar;
pub(crate) mod tether;
#[cfg(test)]
mod ui_checks;
pub(crate) mod workflow_ui;

use auto::*;
use config::*;
use imports::*;
use render::*;

const fn default_ui_scale() -> f32 {
    1.0
}

enum ExportJobEvent {
    Progress(f32),
    Finished { path: PathBuf, count: usize },
    Failed(String),
}

enum ImportJobEvent {
    Progress(f32),
    Finished(sp_catalog::workflow::ImportReport),
    Failed(String),
}

enum ImportReviewEvent {
    Photo {
        entry: crate::state::ImportReviewEntry,
        preview: Option<sp_raw::Preview>,
    },
    Finished(Result<(), String>),
}

enum ModelInstallEvent {
    Progress(f32),
    Finished(String),
    Failed(String),
}

enum CatalogJobEvent {
    Progress(usize),
    Finished {
        photos: Vec<sp_catalog::CatalogPhoto>,
        folders: Vec<sp_catalog::FolderRecord>,
        collections: Vec<sp_catalog::Collection>,
        smart_collections: Vec<sp_catalog::SmartCollection>,
        watchers: Vec<sp_catalog::FolderWatcher>,
        fallback_rescan_folders: Vec<PathBuf>,
        attempted_folders: Vec<PathBuf>,
        folder_errors: Vec<(PathBuf, String)>,
        removed_folder: bool,
    },
    Failed(String),
}

enum ExternalEditEvent {
    Saved {
        source: PathBuf,
        derivative: PathBuf,
    },
    Failed(String),
}

/// Cache key for a full-resolution tile render: (overscanned region, edit,
/// show_original). The overscan lets normal pan/zoom gestures reuse a stable
/// texture instead of alternating between the proxy and a newly rendered
/// tile every frame.
type TileKey = ((u32, u32, u32, u32), EditState, bool);

pub struct ShopPhotoApp {
    sequence_job: Option<Receiver<sequence::Event>>,
    sequence_cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    merge_job: Option<Receiver<multiframe::Event>>,
    mask_refine_job: Option<Receiver<retouch::Refinement>>,
    merge_cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    enhance_runtime: enhance_runtime::Controller,
    catalog_tool_job: Option<Receiver<catalog_tools::Event>>,
    enhancement_job: Option<Receiver<enhance::Event>>,
    enhancement_worker: Option<enhance_worker::InteractiveWorker>,
    enhancement_cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    auto_finish_job: Option<Receiver<auto_finish::Event>>,
    auto_finish_cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    state: AppState,
    jobs: JobSystem,
    /// Pending native file-dialog result (dialog runs on its own thread).
    file_pick: Option<Receiver<Option<PathBuf>>>,
    /// The decoded image currently open. Its shared CFA remains available to
    /// the interactive and export renderers without a second decode.
    current: Option<Box<DecodeOutput>>,
    /// Untouched decode retained so Off and mode changes never compound AI.
    ai_source_raw: Option<std::sync::Arc<sp_core::RawImage>>,
    ai_result_raw: Option<std::sync::Arc<sp_core::RawImage>>,
    /// Unblended model output for the open image, so a mode change only
    /// re-blends instead of re-running inference.
    ai_inferred_raw: Option<std::sync::Arc<sp_core::RawImage>>,
    ai_applied_mode: Option<sp_core::edit::CameraDenoiseMode>,
    ai_applied_compute: Option<sp_ai::ComputeUnits>,
    ai_requested_mode: sp_core::edit::CameraDenoiseMode,
    ai_requested_compute: sp_ai::ComputeUnits,
    ai_generation: Option<u64>,
    /// The path holding the active edit sidecar. For a network source this
    /// is local cache storage; `state.open_image` remains the original path.
    active_edit_source: Option<PathBuf>,
    /// Embedded-preview texture for the viewport (pre-render fallback).
    preview_tex: Option<egui::TextureHandle>,
    /// GPU pipeline; `None` only if eframe has no wgpu render state, which
    /// cannot happen with the wgpu backend (fallback: preview display).
    gpu: Option<GpuState>,
    /// Filmstrip thumbnails per session file.
    thumbs: std::collections::HashMap<PathBuf, egui::TextureHandle>,
    /// Thumbnails of the open photo rendered through each built-in preset,
    /// in `DevelopPreset::ALL` order, and the edit inputs they were made for.
    preset_thumbs: Vec<Option<egui::TextureHandle>>,
    preset_thumbs_key: String,
    preset_thumbs_done: Vec<bool>,
    /// Loaded camera profiles (DCP / Reference Match), by display name.
    profiles: Vec<sp_core::profile::CameraProfile>,
    custom_looks: Vec<(String, sp_core::cube::CubeLut)>,
    custom_proofs: Vec<sp_pipeline::proof::CustomProof>,
    proof_job: Option<Receiver<Result<sp_pipeline::proof::CustomProof, String>>>,
    /// Loaded lens shading profiles, matched to shots by lens and focal
    /// length.
    lenses: Vec<sp_core::lens::LensProfile>,
    /// Pending DCP-import dialog result.
    dcp_pick: Option<Receiver<Option<PathBuf>>>,
    cube_pick: Option<Receiver<Option<PathBuf>>>,
    icc_pick: Option<Receiver<Option<PathBuf>>>,
    user_preset_import_pick: Option<Receiver<Option<PathBuf>>>,
    user_preset_export_pick: Option<Receiver<Option<PathBuf>>>,
    reference_raw_pick: Option<Receiver<Vec<PathBuf>>>,
    reference_tiff_pick: Option<Receiver<Option<PathBuf>>>,
    reference_fit: Option<Receiver<Result<ReferenceJobResult, String>>>,
    export_folder_pick: Option<Receiver<Option<PathBuf>>>,
    export_watermark_pick: Option<Receiver<Option<PathBuf>>>,
    export_profile_pick: Option<Receiver<Result<Option<PathBuf>, String>>>,
    export_job: Option<Receiver<ExportJobEvent>>,
    export_cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    export_queue_active: Option<u64>,
    workflow_rx: Option<Receiver<Result<workflow_ui::WorkflowResult, String>>>,
    gallery_rx: Option<Receiver<Result<gallery::Result, String>>>,
    gallery_request: Option<gallery::Request>,
    export_queue_lock: Option<sp_export::queue::QueueLock>,
    export_queue_prepare: Option<Receiver<Result<Vec<sp_export::queue::ExportTask>, String>>>,
    print_picker: Option<Receiver<Option<PathBuf>>>,
    print_request: Option<printer::PrintRequest>,
    print_open_pending: bool,
    qa_actions_applied: bool,
    external_editor_pick: Option<Receiver<Option<PathBuf>>>,
    /// Source RAW while its dedicated TIFF is being rendered.
    external_edit_source: Option<PathBuf>,
    external_edit_job: Option<Receiver<ExternalEditEvent>>,
    external_edit_recipe: Option<external::Recipe>,
    external_monitor_cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    external_edit_seen_save: bool,
    library_folder_pick: Option<Receiver<Option<PathBuf>>>,
    file_move_folder_pick: Option<Receiver<Option<PathBuf>>>,
    last_file_move_batch: Option<sp_catalog::workflow::FileMoveBatch>,
    import_source_pick: Option<Receiver<Option<PathBuf>>>,
    import_destination_pick: Option<Receiver<Option<PathBuf>>>,
    import_backup_pick: Option<Receiver<Option<PathBuf>>>,
    import_job: Option<Receiver<ImportJobEvent>>,
    import_review_job: Option<Receiver<ImportReviewEvent>>,
    import_thumbs: std::collections::HashMap<PathBuf, egui::TextureHandle>,
    import_cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    model_install: Option<Receiver<ModelInstallEvent>>,
    import_sources_scan: Option<Receiver<Vec<PathBuf>>>,
    catalog_job: Option<Receiver<CatalogJobEvent>>,
    folder_watchers: Vec<sp_catalog::FolderWatcher>,
    /// Local folders whose native watcher could not be installed. Network
    /// shares are deliberately excluded and refreshed only on demand.
    fallback_rescan_folders: Vec<PathBuf>,
    thumbnail_generation: u64,
    thumbnails_requested: std::collections::HashSet<PathBuf>,
    last_visible_thumbnails: std::collections::HashSet<PathBuf>,
    catalog_path: Option<PathBuf>,
    last_logic_at: std::time::Instant,
    window_focused: bool,
    update_check: Option<Receiver<Result<Option<crate::updates::UpdateInfo>, String>>>,
    /// File passed on the command line, opened on the first frame.
    pending_open: Option<PathBuf>,
    /// Renderer being built on a worker for the image just opened or the
    /// AI result. Adopted on the UI thread when ready; a build whose
    /// generation no longer matches was superseded and is dropped.
    pending_gpu_open: Option<(u64, RendererBuild)>,
    gpu_open_generation: u64,
    /// Untouched-RAW comparison renderer being built on a worker.
    pending_compare: Option<(u64, CompareKind, RendererBuild)>,
    /// Side-by-side culling cells (UX-01).
    culling_slots: Vec<culling::CullSlot>,
    culling_test_started: bool,
    /// Frame counter for the SP_SCREENSHOT debug hook.
    frame_count: u64,
    /// Full Auto Balance waits for the asynchronous histogram belonging to
    /// its neutral-tone analysis render. This prevents a stale live histogram
    /// from being mistaken for the analysis result.
    auto_readback: Option<AutoReadback>,
    /// Edit that produced the current automatic-analysis histogram. If the
    /// user changes the image while the GPU readback is in flight, the result
    /// is discarded and rescheduled rather than applied to a different edit.
    auto_analysis_edit: Option<EditState>,
    /// Failed GPU map retries for the active automatic analysis. A persistent
    /// readback failure exits with a toast instead of leaving Analyzing stuck.
    auto_retry_count: u8,
    /// Frame counter for EDR headroom polling (macOS).
    edr_poll_counter: u64,
    /// `--hdr-check`: print an HDR diagnostic report shortly after launch,
    /// then exit.
    hdr_check: bool,
    saved_preferences: AppPreferences,
    last_settings_batch: Option<sp_catalog::workflow::SettingsBatch>,
    gpu_device_loss: sp_gpu::DeviceLossMonitor,
    gpu_recovery_started: bool,
    /// Frame countdown used only by the opt-in device-loss recovery smoke test.
    gpu_recovery_test_frames: u8,
}

impl ShopPhotoApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        #[cfg(target_os = "macos")]
        crate::macos_open::set_context(&cc.egui_ctx);
        theme::apply(&cc.egui_ctx);
        let preferences = load_app_preferences();
        customize::apply_presentation(
            &cc.egui_ctx,
            preferences.high_contrast,
            preferences.reduced_motion,
        );
        let gpu_device_loss = sp_gpu::DeviceLossMonitor::default();
        if let Some(render_state) = &cc.wgpu_render_state {
            gpu_device_loss.install(&render_state.device);
        }
        let gpu =
            cc.wgpu_render_state.clone().and_then(|rs| {
                let ctx = GpuContext::new(rs.device.clone(), rs.queue.clone());
                let hdr_surface = rs.target_format == wgpu::TextureFormat::Rgba16Float;
                if hdr_surface {
                    tracing::info!("float linear scRGB swapchain active");
                    rs.renderer.write().callback_resources.insert(
                        crate::hdr_view::HdrBlitPack::new(&rs.device, rs.target_format),
                    );
                }
                match GpuRenderer::new(ctx) {
                    Ok(mut renderer) => Some({
                        renderer.set_hdr_display(hdr_surface);
                        GpuState {
                            hdr_surface,
                            renderer,
                            render_state: rs,
                            tex_id: None,
                            out_size: egui::Vec2::ZERO,
                            last_rendered: None,
                            last_view: None,
                            tile_id: None,
                            tile_info: None,
                            tile_key: None,
                            loupe_id: None,
                            loupe_info: None,
                            loupe_key: None,
                            last_lens: None,
                            compare: None,
                        }
                    }),
                    Err(e) => {
                        tracing::error!("GPU pipeline unavailable: {e}");
                        None
                    }
                }
            });
        let hdr_surface_active = gpu.as_ref().is_some_and(|g| g.hdr_surface);
        let catalog_path = catalog_path();
        let export_presets = load_export_presets(catalog_path.as_deref());
        let user_presets = user_presets_path()
            .and_then(|path| sp_catalog::workflow::load_user_presets(&path).ok())
            .unwrap_or_default();
        let custom_looks = load_custom_looks();
        let custom_look_names = custom_looks.iter().map(|(name, _)| name.clone()).collect();
        let custom_proofs = load_custom_proofs();
        let custom_proof_names = custom_proofs
            .iter()
            .map(|proof| proof.name.clone())
            .collect::<Vec<_>>();
        let selected_custom_proof = custom_proof_names.first().cloned();
        let first_run_marker =
            dirs::config_dir().map(|dir| dir.join("shopphoto").join("first-run-complete"));
        let update_check = option_env!("SHOPPHOTO_UPDATE_URL").and_then(|url| {
            let stamp = dirs::cache_dir()?.join("shopphoto").join("update-check");
            let (tx, rx) = crossbeam_channel::bounded(1);
            let url = url.to_owned();
            std::thread::Builder::new()
                .name("sp-update-check".into())
                .spawn(move || {
                    let _ = tx.send(crate::updates::check(&url, &stamp));
                })
                .ok()?;
            Some(rx)
        });
        let mut default_export_settings = sp_export::ExportSettings::default();
        if let Some(downloads) = dirs::download_dir() {
            default_export_settings.destination = downloads;
        }
        let mut app = Self {
            sequence_job: None,
            sequence_cancel: None,
            merge_job: None,
            mask_refine_job: None,
            merge_cancel: None,
            enhance_runtime: enhance_runtime::Controller::default(),
            catalog_tool_job: None,
            enhancement_job: None,
            enhancement_worker: None,
            enhancement_cancel: None,
            auto_finish_job: None,
            auto_finish_cancel: None,
            state: AppState {
                hdr_surface: hdr_surface_active,
                proof_space: preferences.proof_space,
                proof_options: preferences.proof_options,
                gamut_warning: preferences.gamut_warning,
                write_xmp: preferences.write_xmp,
                raw_denoise_compute: if preferences.raw_denoise_compute.supported() {
                    preferences.raw_denoise_compute
                } else {
                    sp_ai::ComputeUnits::Automatic
                },
                language: preferences.language,
                left_rail_tab: preferences.left_rail_tab,
                ui_scale: preferences.ui_scale.clamp(0.75, 2.0),
                ui_scale_draft: preferences.ui_scale.clamp(0.75, 2.0),
                external_editor: preferences.external_editor.clone(),
                customize: customize::State {
                    bindings: preferences.shortcuts.clone(),
                    workspaces: preferences.workspaces.clone(),
                    high_contrast: preferences.high_contrast,
                    reduced_motion: preferences.reduced_motion,
                    ..Default::default()
                },
                external: external::load_recipes(),
                lens_catalog_names: hasselblad_xcd_lens_catalog(),
                ai_compare_position: 0.5,
                preset_amount: 100.0,
                mask_point: [0.5, 0.5],
                mask_depth_min: 0.5,
                mask_depth_max: 1.0,
                user_presets,
                custom_look_names,
                custom_proof_names,
                selected_custom_proof,
                retouch_radius: 0.03,
                retouch_feather: 0.5,
                file_rename_pattern: "{date}-{name}-{seq}".into(),
                import_filename_pattern: "{name}-{seq}".into(),
                import_skip_duplicates: true,
                // Mounted volumes are scanned in the background; a slow
                // drive must never hold the window back.
                import_sources_scan_requested: true,
                export_settings: default_export_settings,
                ..Default::default()
            },
            jobs: JobSystem::start(source_cache_dir(), thumbnail_cache_dir()),
            file_pick: None,
            current: None,
            ai_source_raw: None,
            ai_result_raw: None,
            ai_inferred_raw: None,
            ai_applied_mode: None,
            ai_applied_compute: None,
            ai_requested_mode: sp_core::edit::CameraDenoiseMode::Off,
            ai_requested_compute: sp_ai::ComputeUnits::Automatic,
            ai_generation: None,
            active_edit_source: None,
            preview_tex: None,
            gpu,
            thumbs: std::collections::HashMap::new(),
            preset_thumbs: vec![None; sp_core::preset::DevelopPreset::ALL.len()],
            preset_thumbs_key: String::new(),
            preset_thumbs_done: vec![false; sp_core::preset::DevelopPreset::ALL.len()],
            profiles: load_profiles(),
            custom_looks,
            custom_proofs,
            proof_job: None,
            lenses: load_lens_profiles(),
            dcp_pick: None,
            cube_pick: None,
            icc_pick: None,
            user_preset_import_pick: None,
            user_preset_export_pick: None,
            frame_count: 0,
            auto_readback: None,
            auto_analysis_edit: None,
            auto_retry_count: 0,
            edr_poll_counter: 0,
            hdr_check: std::env::args().any(|a| a == "--hdr-check"),
            saved_preferences: preferences,
            last_settings_batch: None,
            gpu_device_loss,
            gpu_recovery_started: false,
            gpu_recovery_test_frames: 0,
            pending_open: std::env::args_os()
                .skip(1)
                .map(PathBuf::from)
                .find(|path| path.is_file() && sp_raw::is_supported_image(path)),
            pending_gpu_open: None,
            gpu_open_generation: 0,
            pending_compare: None,
            culling_slots: Vec::new(),
            culling_test_started: false,
            reference_raw_pick: None,
            reference_tiff_pick: None,
            reference_fit: None,
            export_folder_pick: None,
            export_watermark_pick: None,
            export_profile_pick: None,
            export_job: None,
            export_cancel: None,
            export_queue_active: None,
            workflow_rx: None,
            gallery_rx: None,
            gallery_request: None,
            export_queue_lock: None,
            export_queue_prepare: None,
            print_picker: None,
            print_request: None,
            print_open_pending: false,
            qa_actions_applied: false,
            external_editor_pick: None,
            external_edit_source: None,
            external_edit_job: None,
            external_edit_recipe: None,
            external_monitor_cancel: None,
            external_edit_seen_save: false,
            library_folder_pick: None,
            file_move_folder_pick: None,
            last_file_move_batch: None,
            import_source_pick: None,
            import_destination_pick: None,
            import_backup_pick: None,
            import_job: None,
            import_review_job: None,
            import_thumbs: std::collections::HashMap::new(),
            import_cancel: None,
            model_install: None,
            import_sources_scan: None,
            catalog_job: None,
            folder_watchers: Vec::new(),
            fallback_rescan_folders: Vec::new(),
            thumbnail_generation: 1,
            thumbnails_requested: std::collections::HashSet::new(),
            last_visible_thumbnails: std::collections::HashSet::new(),
            catalog_path,
            last_logic_at: std::time::Instant::now(),
            window_focused: true,
            update_check,
        };
        app.state.export_presets = export_presets;
        app.state.welcome_open = first_run_marker.as_ref().is_none_or(|path| !path.exists());
        if std::env::var_os("SP_VISUAL_REGRESSION").is_some() {
            app.state.welcome_open = false;
        }
        app.refresh_catalog(Vec::new(), None);
        app
    }
}

impl eframe::App for ShopPhotoApp {
    fn on_exit(&mut self) {
        if let Some(cancel) = &self.sequence_cancel {
            cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        if let Some(cancel) = &self.export_cancel {
            cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        if self.state.export_queue_loaded
            && self.state.export_queue_dirty
            && self.state.export_queue_error.is_none()
        {
            self.save_delivery();
        }
        if let Some(cancel) = &self.merge_cancel {
            cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        if let Some(cancel) = &self.auto_finish_cancel {
            cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        if let Some(cancel) = &self.enhancement_cancel {
            cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        self.enhancement_worker.take();
        // Flush the debounced edit and wait for the persist worker so a
        // quit inside the 500 ms window cannot lose the last change.
        self.save_sidecar_if_due(true);
    }

    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if ctx.input(|input| input.viewport().close_requested()) && !self.save_sidecar_if_due(true)
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        }
        self.automation_upkeep();
        self.catalog_tools_upkeep(ctx);
        self.lens_manager_upkeep(ctx);
        self.retouch_requests(ctx);
        let now = std::time::Instant::now();
        self.state.frame_ms = now.duration_since(self.last_logic_at).as_secs_f32() * 1000.0;
        self.last_logic_at = now;
        let mut watchdog = crate::watchdog::Watchdog::start();
        if let Some((target, u, v)) = self.state.defringe_sample_request.take() {
            let sample = self
                .gpu
                .as_ref()
                .map(|gpu| gpu.renderer.sample_display_rgb(u, v));
            match sample {
                Some(Ok(rgb)) => {
                    if let Some((hue, saturation, _value)) = rgb8_hsv(rgb) {
                        let mut edit = self.state.edit.clone();
                        edit.lens.enable = true;
                        edit.lens.defringe = true;
                        match target {
                            DefringePickerTarget::Purple => {
                                edit.lens.defringe_purple_hue = hue;
                                edit.lens.defringe_purple_range = 18.0;
                                edit.lens.defringe_purple = edit.lens.defringe_purple.max(70.0);
                            }
                            DefringePickerTarget::Green => {
                                edit.lens.defringe_green_hue = hue;
                                edit.lens.defringe_green_range = 18.0;
                                edit.lens.defringe_green = edit.lens.defringe_green.max(70.0);
                            }
                        }
                        self.state.apply_edit_step(edit);
                        self.state.push_toast(
                            {
                                let __l10n_args = (&(saturation * 100.0),);
                                crate::i18n::format(
                                    "Fringe color sampled · {0}° · saturation {1}%",
                                    &[format!("{:.0}", hue), format!("{:.0}", __l10n_args.0)],
                                )
                            },
                            false,
                        );
                    } else {
                        self.state.push_toast(
                            "That pixel is nearly neutral; click directly on the colored halo",
                            false,
                        );
                    }
                }
                Some(Err(error)) => self.state.push_toast(
                    crate::i18n::format("Could not sample fringe color: {0}", &[error.to_string()]),
                    true,
                ),
                None => self
                    .state
                    .push_toast("Open an image before sampling", false),
            }
        }
        watchdog.lap("defringe_sample");
        if let Some(uv) = self.state.film_base_sample_request.take() {
            let sample = self
                .gpu
                .as_ref()
                .map(|gpu| gpu.renderer.sample_film_base(uv));
            match sample {
                Some(Ok(base)) => {
                    let mut edit = self.state.edit.clone();
                    edit.negative.base = base;
                    self.state
                        .apply_edit_step_labeled(edit, Some("Sample film base".into()));
                }
                Some(Err(error)) => self.state.push_toast(
                    crate::i18n::format("Could not sample film base: {0}", &[error.to_string()]),
                    true,
                ),
                None => self
                    .state
                    .push_toast("Open an image before sampling", false),
            }
        }
        if let Some((u, v)) = self.state.wb_sample_request.take() {
            // Prefer the untouched sensor data: the neural denoise output is
            // already white-balance neutral in the same camera space.
            let raw = self.ai_source_raw.clone().or_else(|| {
                self.current
                    .as_ref()
                    .and_then(|output| output.source.as_raw().cloned())
            });
            let solved = raw
                .as_ref()
                .and_then(|raw| {
                    let sample = sp_pipeline::match_io::camera_rgb_sample(raw, u, v)?;
                    if sample.iter().any(|c| *c > 0.98) {
                        return None;
                    }
                    sp_core::wb::temp_tint_for_neutral(&raw.cam_to_xyz, sample)
                })
                .or_else(|| {
                    let ImageSource::Raster(image) = &self.current.as_ref()?.source else {
                        return None;
                    };
                    image.neutral_white_balance(u, v)
                });
            match solved {
                Some((temp, tint)) => {
                    let mut edit = self.state.edit.clone();
                    edit.wb.temp_kelvin = Some(temp.round());
                    edit.wb.tint = tint.round();
                    self.state.apply_edit_step_labeled(
                        edit,
                        Some(crate::i18n::format(
                            "White balance {0} K",
                            &[format!("{:.0}", temp)],
                        )),
                    );
                }
                None if self.current.is_none() => self
                    .state
                    .push_toast("Open an image before sampling", false),
                None => self.state.push_toast(
                    "That spot is clipped or too dark to judge; pick a mid-grey area",
                    false,
                ),
            }
        }
        if let Some((index, u, v)) = self.state.color_range_sample_request.take() {
            let sample = self
                .gpu
                .as_ref()
                .map(|gpu| gpu.renderer.sample_display_rgb(u, v));
            match sample {
                Some(Ok(rgb)) => {
                    let target_ab = rgb8_oklab_ab(rgb);
                    let mut edit = self.state.edit.clone();
                    if let Some(sp_core::edit::LocalMask::ColorRange {
                        target_ab: target, ..
                    }) = edit
                        .local_adjustments
                        .get_mut(index)
                        .map(|adjustment| &mut adjustment.mask)
                    {
                        *target = target_ab;
                        self.state.apply_edit_step(edit);
                        self.state.push_toast(
                            {
                                let __l10n_args = (&(target_ab[0]), &(target_ab[1]));
                                crate::i18n::format(
                                    "Color range sampled · a {0} · b {1}",
                                    &[
                                        format!("{:+.3}", __l10n_args.0),
                                        format!("{:+.3}", __l10n_args.1),
                                    ],
                                )
                            },
                            false,
                        );
                    }
                }
                Some(Err(error)) => self.state.push_toast(
                    crate::i18n::format("Could not sample mask color: {0}", &[error.to_string()]),
                    true,
                ),
                None => self
                    .state
                    .push_toast("Open an image before sampling", false),
            }
        }
        watchdog.lap("color_range_sample");
        if let Some((channel, u, v)) = self.state.curve_sample_request.take() {
            let sample = self
                .gpu
                .as_ref()
                .map(|gpu| gpu.renderer.sample_display_rgb(u, v));
            match sample {
                Some(Ok(rgb)) => {
                    let x = curve_input_from_rgb8(rgb, channel).clamp(0.001, 0.999);
                    let mut edit = self.state.edit.clone();
                    let curve = match channel {
                        CurveChannel::Luma => &mut edit.curve.luma,
                        CurveChannel::Red => &mut edit.curve.red,
                        CurveChannel::Green => &mut edit.curve.green,
                        CurveChannel::Blue => &mut edit.curve.blue,
                    };
                    let existing = curve
                        .points
                        .iter()
                        .enumerate()
                        .filter(|(index, _)| *index != 0 && *index + 1 != curve.points.len())
                        .min_by(|(_, a), (_, b)| (a.x - x).abs().total_cmp(&(b.x - x).abs()))
                        .filter(|(_, point)| (point.x - x).abs() <= 0.04)
                        .map(|(index, _)| index);
                    let index = if let Some(index) = existing {
                        index
                    } else {
                        let y = sp_pipeline::tone::sample_curve(curve, x);
                        let index = curve
                            .points
                            .iter()
                            .position(|point| point.x > x)
                            .unwrap_or(curve.points.len() - 1);
                        curve
                            .points
                            .insert(index, sp_core::edit::CurvePoint { x, y });
                        index
                    };
                    let point = curve.points[index];
                    let active = CurvePipetteActive {
                        channel,
                        x: point.x,
                        initial_y: point.y,
                    };
                    self.state.apply_edit_step(edit);
                    self.state.curve_pipette_active = Some(active);
                }
                Some(Err(error)) => self.state.push_toast(
                    crate::i18n::format("Could not sample tone: {0}", &[error.to_string()]),
                    true,
                ),
                None => self
                    .state
                    .push_toast("Open an image before sampling", false),
            }
        }
        watchdog.lap("curve_sample");
        let completed_histogram = self
            .gpu
            .as_mut()
            .and_then(|gpu| gpu.renderer.poll_histogram());
        let received_histogram = completed_histogram.is_some();
        if let Some(histogram) = completed_histogram {
            let analysis_changed = self
                .auto_readback
                .is_some_and(AutoReadback::is_analysis_histogram)
                && self.auto_analysis_edit.as_ref() != Some(&self.state.edit);
            if analysis_changed {
                // A slider, preset, profile, or WB change landed while the
                // readback was in flight. Analyze the new edit instead of
                // applying statistics from a different render.
                self.auto_readback = self
                    .auto_readback
                    .map(AutoReadback::retry_after_changed_edit);
                self.auto_analysis_edit = None;
            } else {
                match self.auto_readback {
                    Some(AutoReadback::BalanceWaitingForNeutralHistogram) => {
                        self.auto_readback = None;
                        self.auto_analysis_edit = None;
                        self.auto_retry_count = 0;
                        self.state.auto_in_flight = false;
                        if let Some(stats) =
                            sp_pipeline::auto_tone::HistogramStats::from_rgb_bins(&histogram)
                        {
                            let mut edit = self.state.edit.clone();
                            edit.tone = sp_pipeline::auto_tone::auto_tone(&stats);
                            let tone = edit.tone;
                            self.state.apply_edit_step(edit);
                            self.state.push_toast(
                                {
                                    let __l10n_args =
                                        (&(tone.exposure_ev), &(tone.highlights), &(tone.shadows));
                                    crate::i18n::format(
                                        "Auto · {0} EV · highlights {1} · shadows {2}",
                                        &[
                                            format!("{:+.2}", __l10n_args.0),
                                            format!("{:+.0}", __l10n_args.1),
                                            format!("{:+.0}", __l10n_args.2),
                                        ],
                                    )
                                },
                                false,
                            );
                        } else {
                            self.state
                                .push_toast("Auto Balance could not read this image", false);
                        }
                    }
                    Some(AutoReadback::ExposureWaitingForCurrentHistogram) => {
                        self.auto_readback = None;
                        self.auto_analysis_edit = None;
                        self.auto_retry_count = 0;
                        self.state.auto_in_flight = false;
                        if let Some(stats) =
                            sp_pipeline::auto_tone::HistogramStats::from_rgb_bins(&histogram)
                        {
                            let correction = sp_pipeline::auto_tone::auto_exposure(&stats);
                            let mut edit = self.state.edit.clone();
                            edit.tone.exposure_ev =
                                (edit.tone.exposure_ev + correction).clamp(-5.0, 5.0);
                            let exposure = edit.tone.exposure_ev;
                            self.state.apply_edit_step(edit);
                            self.state.push_toast(
                                crate::i18n::format(
                                    "Exposure · {0} EV ({1})",
                                    &[format!("{:+.2}", exposure), format!("{:+.2}", correction)],
                                ),
                                false,
                            );
                        } else {
                            self.state
                                .push_toast("Auto Exposure could not read this image", false);
                        }
                    }
                    Some(AutoReadback::DefringeWaitingForHistogram) => {
                        self.auto_readback = None;
                        self.auto_analysis_edit = None;
                        self.auto_retry_count = 0;
                        self.state.auto_in_flight = false;
                        if let Some(result) = sp_pipeline::auto_defringe::recommend(&histogram) {
                            let mut edit = self.state.edit.clone();
                            edit.lens.enable = true;
                            edit.lens.defringe = true;
                            edit.lens.defringe_purple = result.purple;
                            edit.lens.defringe_green = result.green;
                            edit.lens.defringe_radius_px = result.radius_px;
                            edit.lens.defringe_threshold = result.threshold;
                            edit.lens.defringe_wide_bloom = result.wide_bloom;
                            self.state.apply_edit_step(edit);
                            self.state.push_toast(
                                {
                                    let __l10n_args =
                                        (&(result.purple), &(result.green), &(result.radius_px));
                                    crate::i18n::format(
                                        "Auto Defringe · purple {0} · green {1} · width {2}px",
                                        &[
                                            format!("{:.0}", __l10n_args.0),
                                            format!("{:.0}", __l10n_args.1),
                                            format!("{:.0}", __l10n_args.2),
                                        ],
                                    )
                                },
                                false,
                            );
                        } else {
                            self.state.push_toast(
                                "Auto Defringe found no reliable purple or green fringe",
                                false,
                            );
                        }
                    }
                    _ => self.state.histogram = Some(histogram),
                }
            }
        }
        let analysis_readback_dropped = !received_histogram
            && self
                .auto_readback
                .is_some_and(AutoReadback::is_analysis_histogram)
            && self
                .gpu
                .as_ref()
                .is_some_and(|gpu| !gpu.renderer.histogram_pending());
        if analysis_readback_dropped {
            if self.auto_retry_count < 2 {
                self.auto_retry_count += 1;
                self.auto_readback = self
                    .auto_readback
                    .map(AutoReadback::retry_after_changed_edit);
                self.auto_analysis_edit = None;
            } else {
                self.auto_readback = None;
                self.auto_analysis_edit = None;
                self.auto_retry_count = 0;
                self.state.auto_in_flight = false;
                self.state
                    .push_toast("Automatic image analysis readback failed", true);
            }
        }
        if self.auto_readback == Some(AutoReadback::BalanceWaitingForSlot) {
            if self.schedule_auto_balance_histogram() {
                self.auto_readback = Some(AutoReadback::BalanceWaitingForNeutralHistogram);
            }
        } else if self.auto_readback == Some(AutoReadback::ExposureWaitingForSlot)
            && self.schedule_auto_exposure_histogram()
        {
            self.auto_readback = Some(AutoReadback::ExposureWaitingForCurrentHistogram);
        } else if self.auto_readback == Some(AutoReadback::DefringeWaitingForSlot)
            && self.schedule_auto_defringe_histogram()
        {
            self.auto_readback = Some(AutoReadback::DefringeWaitingForHistogram);
        }
        watchdog.lap("histogram");
        if let Some(gpu) = &mut self.gpu {
            self.state.gpu_texture_mb =
                gpu.renderer.estimated_texture_bytes() as f32 / (1024.0 * 1024.0);
        }
        if let Some(path) = self.pending_open.take() {
            self.start_decode(path, ctx);
        }
        #[cfg(target_os = "macos")]
        if let Some(path) = crate::macos_open::take_open_file() {
            if self.state.open_image.as_ref() != Some(&path) {
                self.start_decode(path, ctx);
            }
        }
        self.screenshot_hook(ctx);
        self.gpu_recovery_upkeep(ctx);
        self.macos_edr_upkeep();
        self.hdr_check_hook(ctx);
        if self.state.profile_names.len() != self.profiles.len() {
            self.refresh_profile_names();
        }
        watchdog.lap("hooks");
        self.handle_shortcuts(ctx);
        if std::mem::take(&mut self.state.external_editor_pick_requested) {
            self.pick_external_editor();
        }
        if std::mem::take(&mut self.state.external_edit_requested) {
            self.start_external_edit(ctx);
        }
        self.poll_external_edit(ctx);
        let preferences = AppPreferences {
            proof_space: self.state.proof_space,
            proof_options: self.state.proof_options,
            gamut_warning: self.state.gamut_warning,
            write_xmp: self.state.write_xmp,
            raw_denoise_compute: self.state.raw_denoise_compute,
            language: self.state.language,
            left_rail_tab: self.state.left_rail_tab,
            ui_scale: self.state.ui_scale,
            external_editor: self.state.external_editor.clone(),
            shortcuts: self.state.customize.bindings.clone(),
            workspaces: self.state.customize.workspaces.clone(),
            high_contrast: self.state.customize.high_contrast,
            reduced_motion: self.state.customize.reduced_motion,
        };
        if preferences != self.saved_preferences {
            if preferences.high_contrast != self.saved_preferences.high_contrast
                || preferences.reduced_motion != self.saved_preferences.reduced_motion
            {
                customize::apply_presentation(
                    ctx,
                    preferences.high_contrast,
                    preferences.reduced_motion,
                );
            }
            save_app_preferences(preferences.clone());
            self.saved_preferences = preferences;
        }
        ctx.set_zoom_factor(self.state.ui_scale.clamp(0.75, 2.0));
        self.state.sharpen_mask_preview = ctx.data_mut(|data| {
            data.remove_temp::<bool>(egui::Id::new("sharpen-mask-preview"))
                .unwrap_or(false)
        });
        self.state.alt_clipping = ctx.input(|input| {
            input.modifiers.alt && input.pointer.primary_down() && !self.state.sharpen_mask_preview
        });
        watchdog.lap("shortcuts");
        self.poll_background(ctx);
        self.poll_auto_finish(ctx);
        self.poll_enhancements(ctx);
        self.enhance_runtime.upkeep(&mut self.state, ctx);
        self.multiframe_upkeep(ctx);
        self.tether_upkeep(ctx);
        self.sequence_upkeep(ctx);
        self.sync_proof_options(ctx);
        if let Some(kind) = self.state.enhancement_requested.take() {
            self.start_mask(kind, ctx);
        }
        if std::mem::take(&mut self.state.mask_import_requested) {
            self.import_mask(ctx);
        }
        if std::mem::take(&mut self.state.upscale_model_pick_requested) {
            self.pick_upscale_model(ctx);
        }
        if std::mem::take(&mut self.state.enhancement_cancel_requested) {
            if let Some(cancel) = &self.enhancement_cancel {
                cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        }
        if std::mem::take(&mut self.state.auto_finish_requested) {
            self.start_auto_finish(ctx);
        }
        if std::mem::take(&mut self.state.auto_finish_cancel_requested) {
            if let Some(cancel) = &self.auto_finish_cancel {
                cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        }
        watchdog.lap("poll_background");
        self.adopt_ready_renderers(ctx);
        watchdog.lap("adopt_renderers");
        self.culling_upkeep(ctx);
        watchdog.lap("culling");
        let catalog_changed = self
            .folder_watchers
            .iter_mut()
            .any(sp_catalog::FolderWatcher::poll_rescan);
        let focused = ctx.input(|input| input.viewport().focused.unwrap_or(true));
        let regained_focus = focused && !self.window_focused;
        self.window_focused = focused;
        // Watchers continue receiving changes while the app is unfocused, so
        // a focus change does not justify rescanning every network share.
        // Use focus-based fallback only for folders whose watcher could not
        // be installed.
        let rescan_folders = if catalog_changed {
            self.state
                .catalog_folders
                .iter()
                .map(|folder| folder.path.clone())
                .collect()
        } else if regained_focus {
            self.fallback_rescan_folders.clone()
        } else {
            Vec::new()
        };
        if !rescan_folders.is_empty() && self.catalog_job.is_none() {
            let folders = rescan_folders;
            self.refresh_catalog(folders, None);
        }
        watchdog.lap("catalog_rescan");
        if self.state.auto_tone_requested
            || self.state.auto_exposure_requested
            || self.state.auto_defringe_requested
        {
            let requested_balance = self.state.auto_tone_requested;
            let requested_exposure = self.state.auto_exposure_requested;
            let requested_defringe = self.state.auto_defringe_requested;
            self.state.auto_tone_requested = false;
            self.state.auto_exposure_requested = false;
            self.state.auto_defringe_requested = false;
            // Buttons are disabled while analyzing, but this also guards
            // delayed shortcut/gesture requests from replacing the histogram
            // state machine mid-readback.
            if !self.state.auto_in_flight && self.gpu.is_none() {
                self.state
                    .push_toast("Automatic image analysis requires the GPU renderer", false);
            } else if !self.state.auto_in_flight && requested_balance {
                // Full Auto Balance is absolute. Its neutral-tone render and
                // asynchronous histogram are tracked explicitly so an older
                // live histogram can never be applied by mistake.
                self.auto_readback = Some(AutoReadback::BalanceWaitingForSlot);
                self.auto_retry_count = 0;
                self.state.auto_in_flight = true;
                if self.schedule_auto_balance_histogram() {
                    self.auto_readback = Some(AutoReadback::BalanceWaitingForNeutralHistogram);
                }
            } else if !self.state.auto_in_flight && requested_exposure {
                // Exposure-only is relative, but still analyzes a freshly
                // rendered copy of the current edit before adding correction.
                self.auto_readback = Some(AutoReadback::ExposureWaitingForSlot);
                self.auto_retry_count = 0;
                self.state.auto_in_flight = true;
                if self.schedule_auto_exposure_histogram() {
                    self.auto_readback = Some(AutoReadback::ExposureWaitingForCurrentHistogram);
                }
            } else if !self.state.auto_in_flight && requested_defringe {
                self.auto_readback = Some(AutoReadback::DefringeWaitingForSlot);
                self.auto_retry_count = 0;
                self.state.auto_in_flight = true;
                if self.schedule_auto_defringe_histogram() {
                    self.auto_readback = Some(AutoReadback::DefringeWaitingForHistogram);
                }
            }
        }
        if self.state.auto_ca_requested {
            self.state.auto_ca_requested = false;
            if !self.state.auto_in_flight {
                self.run_auto_ca();
            }
        }
        if self.state.auto_level_requested {
            self.state.auto_level_requested = false;
            if !self.state.auto_in_flight {
                self.run_auto_level();
            }
        }
        if self.state.save_lens_geometry_requested {
            self.state.save_lens_geometry_requested = false;
            self.save_lens_geometry();
        }
        watchdog.lap("auto_requests");
        self.sync_ai_denoise(ctx);
        self.sync_ai_compare();
        watchdog.lap("ai_sync");
        self.state.record_edit_gesture(std::time::Instant::now());
        self.update_preset_thumbnails(ctx);
        self.gpu_rerender_if_dirty(ctx);
        watchdog.lap("rerender");
        self.sync_ai_split_compare(ctx);
        watchdog.lap("split_compare");
        self.save_sidecar_if_due(false);
        watchdog.lap("sidecar");
        // Wake up for the pending debounced save.
        if self.state.sidecar_dirty {
            ctx.request_repaint_after(std::time::Duration::from_millis(300));
        }
        if self.state.open_dialog_requested {
            self.state.open_dialog_requested = false;
            self.open_file_dialog();
        }
        if self.state.virtual_copy_create_requested {
            self.state.virtual_copy_create_requested = false;
            self.create_virtual_copy();
        }
        if let Some(copy_id) = self.state.virtual_copy_switch_requested.take() {
            self.switch_virtual_copy(copy_id);
        }
        if std::mem::take(&mut self.state.virtual_copy_rename_requested) {
            self.rename_active_virtual_copy();
        }
        if std::mem::take(&mut self.state.virtual_copy_trash_requested) {
            self.trash_active_virtual_copy();
        }
        if let Some((path, id)) = self.state.virtual_copy_open_requested.take() {
            if self.state.open_image.as_ref() == Some(&path) {
                self.switch_virtual_copy(id);
            } else {
                self.start_decode(path.clone(), ctx);
                self.state.pending_virtual_copy = Some((path, id));
            }
            self.state.mode = Mode::Develop;
        }
        if self.state.sync_settings_requested {
            self.state.sync_settings_requested = false;
            if let Some(active) = self.state.open_image.clone() {
                let targets: Vec<_> = self
                    .state
                    .selected_photos
                    .iter()
                    .filter(|path| **path != active)
                    .cloned()
                    .collect();
                let source = sp_catalog::Sidecar {
                    version: sp_core::edit::EDIT_SCHEMA_VERSION,
                    edit: self.state.edit.clone(),
                    annotations: self.state.annotations,
                    metadata: self.state.descriptive_metadata.clone(),
                    snapshots: self.state.snapshots.clone(),
                    history: self.state.persistent_history.clone(),
                    virtual_copy: self.state.virtual_copy.clone(),
                    virtual_copy_name: self.state.virtual_copy_name.clone(),
                };
                match sp_catalog::workflow::sync_settings(
                    &source,
                    &targets,
                    self.state.settings_groups,
                ) {
                    Ok(batch) => {
                        self.last_settings_batch = Some(batch);
                        self.state.push_toast_with_action(
                            {
                                let __l10n_args = (
                                    &(targets.len()),
                                    &(if targets.len() == 1 { "" } else { "s" }),
                                );
                                crate::i18n::format(
                                    "Synced settings to {0} photo{1}",
                                    &[__l10n_args.0.to_string(), __l10n_args.1.to_string()],
                                )
                            },
                            false,
                            Some(crate::state::ToastAction::UndoSync),
                        );
                    }
                    Err(error) => self.state.push_toast(
                        crate::i18n::format("Could not sync settings: {0}", &[error.to_string()]),
                        true,
                    ),
                }
            }
        }
        if let Some((change, paths)) = self.state.library_annotate.take() {
            self.annotate_paths(change, &paths);
        }
        match self.state.toast_action_requested.take() {
            Some(crate::state::ToastAction::UndoFileMove) => {
                self.state.file_move_undo_requested = true;
            }
            Some(crate::state::ToastAction::UndoSync) => self.state.undo_sync_requested = true,
            None => {}
        }
        if self.state.undo_sync_requested {
            self.state.undo_sync_requested = false;
            if let Some(batch) = self.last_settings_batch.take() {
                match batch.undo() {
                    Ok(()) => self.state.push_toast("Settings sync undone", false),
                    Err(error) => self.state.push_toast(
                        crate::i18n::format("Could not undo sync: {0}", &[error.to_string()]),
                        true,
                    ),
                }
            }
        }
        if self.state.import_dcp_requested {
            self.state.import_dcp_requested = false;
            self.import_dcp_dialog();
        }
        if self.state.import_cube_requested {
            self.state.import_cube_requested = false;
            self.import_cube_dialog();
        }
        if self.state.import_icc_requested {
            self.state.import_icc_requested = false;
            self.import_icc_dialog();
        }
        if self.state.ai_model_install_requested {
            self.state.ai_model_install_requested = false;
            self.start_model_install();
        }
        if self.state.import_sources_scan_requested && self.import_sources_scan.is_none() {
            self.state.import_sources_scan_requested = false;
            let (tx, rx) = crossbeam_channel::bounded(1);
            std::thread::Builder::new()
                .name("sp-import-sources".into())
                .spawn(move || {
                    let _ = tx.send(sp_catalog::workflow::detected_import_sources());
                })
                .expect("spawn import source scan");
            self.import_sources_scan = Some(rx);
            self.state.import_sources_scanning = true;
        }
        if let Some(rx) = &self.import_sources_scan {
            match rx.try_recv() {
                Ok(sources) => {
                    self.import_sources_scan = None;
                    self.state.import_sources_scanning = false;
                    self.state.import_detected_sources = sources;
                }
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    self.import_sources_scan = None;
                    self.state.import_sources_scanning = false;
                }
                Err(crossbeam_channel::TryRecvError::Empty) => {
                    ctx.request_repaint_after(std::time::Duration::from_millis(100));
                }
            }
        }
        if self.state.user_preset_import_requested {
            self.state.user_preset_import_requested = false;
            self.pick_user_preset_import();
        }
        if self.state.user_preset_export_requested {
            self.state.user_preset_export_requested = false;
            self.pick_user_preset_export();
        }
        if self.state.import_source_requested {
            self.state.import_source_requested = false;
            self.pick_import_source();
        }
        if self.state.import_destination_requested {
            self.state.import_destination_requested = false;
            self.pick_import_destination();
        }
        if self.state.import_backup_requested {
            self.state.import_backup_requested = false;
            self.pick_import_backup();
        }
        if self.state.import_start_requested {
            self.state.import_start_requested = false;
            self.start_import(ctx);
        }
        if self.state.import_review_requested {
            self.state.import_review_requested = false;
            self.start_import_review(ctx);
        }
        if self.state.import_cancel_requested {
            self.state.import_cancel_requested = false;
            if let Some(cancel) = &self.import_cancel {
                cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        }
        if self.state.reference_pick_raws_requested {
            self.state.reference_pick_raws_requested = false;
            self.pick_reference_raws();
        }
        if self.state.reference_pick_tiff_requested {
            self.state.reference_pick_tiff_requested = false;
            self.pick_reference_tiff_folder();
        }
        if self.state.reference_fit_requested {
            self.state.reference_fit_requested = false;
            self.start_reference_fit();
        }
        if self.state.export_destination_requested {
            self.state.export_destination_requested = false;
            self.pick_export_folder();
        }
        self.handle_export_profile(ctx);
        self.poll_printer(ctx);
        self.poll_delivery(ctx);
        self.poll_workflow(ctx);
        self.poll_gallery(ctx);
        if self.state.export_watermark_requested {
            self.state.export_watermark_requested = false;
            self.pick_export_watermark();
        }
        if self.state.export_start_requested {
            self.state.export_start_requested = false;
            self.start_export(ctx);
        }
        if self.state.export_cancel_requested {
            self.state.export_cancel_requested = false;
            if let Some(cancel) = &self.export_cancel {
                cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        }
        if self.state.export_preset_save_requested {
            self.state.export_preset_save_requested = false;
            let name = self.state.export_preset_name.trim().to_owned();
            if !name.is_empty() {
                let preset = sp_export::ExportPreset {
                    name: name.clone(),
                    settings: self.state.export_settings.clone(),
                };
                self.state
                    .export_presets
                    .retain(|candidate| candidate.name != name);
                self.state.export_presets.push(preset);
                self.state
                    .export_presets
                    .sort_by_key(|preset| preset.name.to_lowercase());
                let saved_json = export_presets_path()
                    .ok_or_else(|| "No configuration directory is available".to_owned())
                    .and_then(|path| {
                        sp_export::save_presets(&path, &self.state.export_presets)
                            .map_err(|error| error.to_string())
                    });
                let saved_db = self.catalog_path.as_deref().map_or(Ok(()), |path| {
                    let settings = serde_json::to_string(&self.state.export_settings)
                        .map_err(|error| error.to_string())?;
                    sp_catalog::CatalogDb::open(path)
                        .and_then(|db| db.save_export_preset(&name, &settings))
                        .map_err(|error| error.to_string())
                });
                match saved_json.and(saved_db) {
                    Ok(()) => self.state.push_toast(
                        crate::i18n::format("Saved preset “{0}”", std::slice::from_ref(&name)),
                        false,
                    ),
                    Err(error) => self.state.push_toast(
                        crate::i18n::format(
                            "Could not save export preset: {0}",
                            std::slice::from_ref(&error),
                        ),
                        true,
                    ),
                }
            }
        }
        if self.state.user_preset_save_requested {
            self.state.user_preset_save_requested = false;
            let name = self.state.user_preset_name.trim().to_owned();
            if !name.is_empty() {
                let preset = sp_core::preset::UserPreset::from_edit(
                    &name,
                    self.state.settings_groups,
                    &self.state.edit,
                );
                self.state.user_presets.retain(|item| item.name != name);
                self.state.user_presets.push(preset);
                self.state
                    .user_presets
                    .sort_by_key(|item| item.name.to_lowercase());
                match user_presets_path()
                    .ok_or_else(|| "No configuration directory is available".to_owned())
                    .and_then(|path| {
                        sp_catalog::workflow::save_user_presets(&path, &self.state.user_presets)
                            .map_err(|e| e.to_string())
                    }) {
                    Ok(()) => {
                        self.state.user_preset_name.clear();
                        self.state.push_toast(
                            crate::i18n::format(
                                "Saved develop preset “{0}”",
                                std::slice::from_ref(&name),
                            ),
                            false,
                        );
                    }
                    Err(error) => self.state.push_toast(
                        crate::i18n::format(
                            "Could not save develop preset: {0}",
                            std::slice::from_ref(&error),
                        ),
                        true,
                    ),
                }
            }
        }
        if self.state.library_add_folder_requested {
            self.state.library_add_folder_requested = false;
            self.pick_library_folder();
        }
        if std::mem::take(&mut self.state.library_refresh_requested) && self.catalog_job.is_none() {
            let folders = self
                .state
                .catalog_folders
                .iter()
                .map(|folder| folder.path.clone())
                .collect();
            self.refresh_catalog(folders, None);
        }
        if self.state.file_rename_requested
            && self.catalog_job.is_none()
            && !self.state.export_busy
            && !self.state.catalog_tool_busy
        {
            self.state.file_rename_requested = false;
            if !self.save_sidecar_if_due(true) {
                return;
            }
            let sources = self.selected_file_sources();
            let pattern = self.state.file_rename_pattern.clone();
            match sp_catalog::workflow::rename_photos(&sources, &pattern) {
                Ok(batch) => self.adopt_completed_file_moves(batch),
                Err(error) => self.state.push_toast(error.to_string(), true),
            }
        }
        if self.state.file_move_requested {
            self.state.file_move_requested = false;
            self.pick_file_move_folder();
        }
        if self.state.file_trash_requested
            && self.catalog_job.is_none()
            && !self.state.catalog_tool_busy
        {
            self.state.file_trash_requested = false;
            if !self.save_sidecar_if_due(true) {
                return;
            }
            let sources = self.selected_file_sources();
            let mut removed = 0usize;
            for (path, _) in sources {
                match sp_catalog::workflow::trash_photo(&path) {
                    Ok(()) => removed += 1,
                    Err(error) => {
                        self.state.push_toast(error.to_string(), true);
                        break;
                    }
                }
            }
            if removed > 0 {
                self.state.push_toast(
                    crate::i18n::format("Moved {0} photo(s) to trash", &[removed.to_string()]),
                    false,
                );
                if self
                    .state
                    .open_image
                    .as_ref()
                    .is_some_and(|path| !path.exists())
                {
                    self.state.open_image = None;
                    self.current = None;
                }
                let folders = self
                    .state
                    .catalog_folders
                    .iter()
                    .map(|folder| folder.path.clone())
                    .collect();
                self.refresh_catalog(folders, None);
            }
        }
        if self.state.file_move_undo_requested
            && self.catalog_job.is_none()
            && !self.state.export_busy
            && !self.state.catalog_tool_busy
        {
            self.state.file_move_undo_requested = false;
            self.undo_completed_file_moves();
        }
        if self.catalog_job.is_none() {
            if let Some(folder_id) = self.state.library_remove_folder_requested.take() {
                self.refresh_catalog(Vec::new(), Some(folder_id));
            }
        }
        if self.state.collection_create_requested {
            self.state.collection_create_requested = false;
            let name = self.state.new_collection_name.trim().to_owned();
            if !name.is_empty() {
                if let Some(db_path) = &self.catalog_path {
                    match sp_catalog::CatalogDb::open(db_path).and_then(|db| {
                        db.create_collection_in(&name, self.state.new_collection_parent)
                    }) {
                        Ok(id) => {
                            self.state.catalog_collections.push(sp_catalog::Collection {
                                id,
                                name: name.clone(),
                                parent_id: self.state.new_collection_parent,
                            });
                            self.state.new_collection_name.clear();
                        }
                        Err(error) => self.state.push_toast(error.to_string(), true),
                    }
                }
            }
        }
        if self.state.smart_collection_save_requested {
            self.state.smart_collection_save_requested = false;
            let name = self.state.new_smart_collection_name.trim().to_owned();
            if !name.is_empty() {
                let result = self.catalog_path.as_deref().map_or_else(
                    || Err("No catalog is available".to_owned()),
                    |path| {
                        let db = sp_catalog::CatalogDb::open(path).map_err(|e| e.to_string())?;
                        let id = db
                            .save_smart_collection(&name, &self.state.library_filter)
                            .map_err(|e| e.to_string())?;
                        Ok(id)
                    },
                );
                match result {
                    Ok(id) => {
                        self.state
                            .catalog_smart_collections
                            .retain(|collection| collection.name != name);
                        self.state
                            .catalog_smart_collections
                            .push(sp_catalog::SmartCollection {
                                id,
                                name: name.clone(),
                                filter: self.state.library_filter.clone(),
                            });
                        self.state
                            .catalog_smart_collections
                            .sort_by_key(|collection| collection.name.to_lowercase());
                        self.state.new_smart_collection_name.clear();
                        self.state.push_toast("Smart collection saved", false);
                    }
                    Err(error) => self.state.push_toast(error, true),
                }
            }
        }
        if let Some((collection, path)) = self.state.collection_add_requested.take() {
            if let Some(db_path) = &self.catalog_path {
                match sp_catalog::CatalogDb::open(db_path)
                    .and_then(|db| db.add_to_collection(collection, &path))
                {
                    Ok(()) => {
                        if let Some(photo) = self
                            .state
                            .catalog_photos
                            .iter_mut()
                            .find(|photo| photo.path == path)
                        {
                            if !photo.collection_ids.contains(&collection) {
                                photo.collection_ids.push(collection);
                            }
                        }
                        self.state.push_toast("Added to collection", false);
                    }
                    Err(error) => self.state.push_toast(error.to_string(), true),
                }
            }
        }
        let visible: std::collections::HashSet<_> =
            self.state.visible_thumbnail_paths.iter().cloned().collect();
        if visible != self.last_visible_thumbnails {
            self.thumbnail_generation = self.jobs.begin_thumbnail_generation();
            self.thumbnails_requested
                .retain(|path| visible.contains(path));
            let open = self.state.open_image.as_ref();
            self.thumbs
                .retain(|path, _| visible.contains(path) || open == Some(path));
            self.last_visible_thumbnails = visible;
        }
        // Bound GPU thumbnail textures as well as the worker's CPU preview cache.
        let mut thumbnail_bytes: usize = self
            .thumbs
            .values()
            .map(|texture| texture.size()[0] * texture.size()[1] * 4)
            .sum();
        if thumbnail_bytes > self.jobs.thumbnail_budget() {
            let mut victims: Vec<_> = self
                .thumbs
                .keys()
                .filter(|path| self.state.open_image.as_ref() != Some(*path))
                .cloned()
                .collect();
            victims.sort();
            for path in victims {
                if thumbnail_bytes <= self.jobs.thumbnail_budget() {
                    break;
                }
                if let Some(texture) = self.thumbs.remove(&path) {
                    thumbnail_bytes -= texture.size()[0] * texture.size()[1] * 4;
                    self.thumbnails_requested.remove(&path);
                }
            }
        }
        for path in std::mem::take(&mut self.state.thumbnail_requests) {
            if !self.thumbs.contains_key(&path) && self.thumbnails_requested.insert(path.clone()) {
                self.jobs.request_thumbnail(path, self.thumbnail_generation);
            }
        }
        if let Some(path) = self.state.filmstrip_clicked.take() {
            if self.state.open_image.as_ref() != Some(&path) {
                self.start_decode(path, ctx);
            }
        }
        watchdog.lap("thumbnails");
        if let Some(report) = watchdog.finish() {
            self.state.last_stall = Some(report);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        crate::i18n::set_application_language(self.state.language);
        lens_manager::window(ui.ctx(), &mut self.state.lens_manager, &self.lenses);
        catalog_tools::auto_stack_window(ui.ctx(), &mut self.state);
        external::recipe_window(ui.ctx(), &mut self.state.external);
        enhance_runtime::window(ui.ctx(), &mut self.state.enhance_runtime);
        multiframe::window(ui.ctx(), &mut self.state.merge);
        tether::window(ui.ctx(), &mut self.state.tether);
        sequence::window(ui.ctx(), &mut self.state.sequence);
        self.state.visible_thumbnail_paths.clear();
        // Layout order matters: top/bottom/side panels claim space first;
        // CentralPanel (viewport) takes the remainder (BUILD.md §5.2).
        egui::Panel::top("top_bar")
            .exact_size(theme::TOP_BAR_H)
            .show(ui, |ui| panels::top_bar(ui, &mut self.state));

        egui::Panel::bottom("filmstrip")
            .exact_size(theme::FILMSTRIP_H)
            .show(ui, |ui| {
                panels::filmstrip(ui, &mut self.state, &self.thumbs)
            });

        if !self.state.rails_hidden {
            egui::Panel::left("left_rail")
                .exact_size(theme::LEFT_RAIL_W)
                .resizable(false)
                .show(ui, |ui| {
                    let enabled = self.state.mode == Mode::Library || !self.state.decode_in_flight;
                    ui.add_enabled_ui(enabled, |ui| {
                        panels::left_rail(ui, &mut self.state, &self.preset_thumbs)
                    });
                });

            egui::Panel::right("right_panel")
                .exact_size(theme::RIGHT_PANEL_W)
                .resizable(false)
                .show(ui, |ui| {
                    let loupe = self.gpu.as_ref().and_then(|g| g.loupe_id.zip(g.loupe_info));
                    ui.add_enabled_ui(!self.state.decode_in_flight, |ui| {
                        panels::right_panel(ui, &mut self.state, loupe)
                    });
                });
        }

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(theme::BG_VIEWPORT))
            .show(ui, |ui| {
                if self.state.mode == Mode::Library {
                    panels::library_grid(ui, &mut self.state, &self.thumbs);
                    return;
                }
                let gpu_tex = self
                    .gpu
                    .as_ref()
                    .and_then(|g| g.tex_id.map(|id| (id, g.out_size)));
                let tile = self.gpu.as_ref().and_then(|g| g.tile_id.zip(g.tile_info));
                let compare_gpu_tex = self.gpu.as_ref().and_then(|g| {
                    g.compare
                        .as_ref()
                        .map(|compare| (compare.tex_id, compare.out_size))
                });
                let compare_tile = self.gpu.as_ref().and_then(|g| {
                    g.compare
                        .as_ref()
                        .and_then(|compare| compare.tile_id.zip(compare.tile_info))
                });
                let hdr_blit = self
                    .gpu
                    .as_ref()
                    .is_some_and(|g| g.hdr_surface && g.renderer.hdr_display_view().is_some());
                let culling = self.culling_cells();
                panels::viewport(
                    ui,
                    &mut self.state,
                    panels::ViewportTextures {
                        hdr_blit,
                        developed: gpu_tex,
                        developed_tile: tile,
                        before_denoise: compare_gpu_tex,
                        before_denoise_tile: compare_tile,
                        preview: self.preview_tex.as_ref(),
                        culling: &culling,
                    },
                );
            });

        panels::toasts_overlay(ui.ctx(), &mut self.state);
        automation::console_ui(ui.ctx(), &mut self.state.command_console);
        panels::import_dialog(ui.ctx(), &mut self.state, &self.import_thumbs);
        panels::export_dialog(ui.ctx(), &mut self.state);
        delivery::queue_window(ui.ctx(), &mut self.state);
        workflow_ui::window(ui.ctx(), &mut self.state);
        gallery::window(ui.ctx(), &mut self.state);
        customize::window(ui.ctx(), &mut self.state);
        self.diagnostics_window(ui.ctx());
        printer::print_window(ui.ctx(), &mut self.state);
        panels::reference_match_wizard(ui.ctx(), &mut self.state);
        panels::shortcuts_dialog(ui.ctx(), &mut self.state);
        panels::debug_overlay(ui.ctx(), &self.state);
        panels::welcome_dialog(ui.ctx(), &mut self.state);
    }
}

#[cfg(test)]
mod auto_readback_tests {
    use super::{hasselblad_xcd_lens_catalog, overscanned_tile_region, rgb8_hsv, AutoReadback};

    #[test]
    fn tile_overscan_absorbs_normal_high_zoom_pans() {
        let cached = overscanned_tile_region((4_000, 3_000, 800, 600), (11_904, 8_842));
        assert_eq!(cached, (3_744, 2_744, 1_312, 1_112));
        let panned = (4_180, 2_850, 800, 600);
        assert!(cached.0 <= panned.0);
        assert!(cached.1 <= panned.1);
        assert!(cached.0 + cached.2 >= panned.0 + panned.2);
        assert!(cached.1 + cached.3 >= panned.1 + panned.3);
    }

    #[test]
    fn changed_edits_retry_the_same_kind_of_analysis() {
        assert_eq!(
            AutoReadback::BalanceWaitingForNeutralHistogram.retry_after_changed_edit(),
            AutoReadback::BalanceWaitingForSlot
        );
        assert_eq!(
            AutoReadback::ExposureWaitingForCurrentHistogram.retry_after_changed_edit(),
            AutoReadback::ExposureWaitingForSlot
        );
        assert!(AutoReadback::BalanceWaitingForNeutralHistogram.is_analysis_histogram());
        assert!(AutoReadback::ExposureWaitingForCurrentHistogram.is_analysis_histogram());
        assert_eq!(
            AutoReadback::DefringeWaitingForHistogram.retry_after_changed_edit(),
            AutoReadback::DefringeWaitingForSlot
        );
        assert!(AutoReadback::DefringeWaitingForHistogram.is_analysis_histogram());
        assert!(!AutoReadback::BalanceWaitingForSlot.is_analysis_histogram());
    }

    #[test]
    fn xcd_catalog_is_preloaded_and_has_no_duplicate_identity() {
        let catalog = hasselblad_xcd_lens_catalog();
        let unique: std::collections::HashSet<_> = catalog.iter().collect();
        assert_eq!(catalog.len(), 22);
        assert_eq!(catalog.len(), unique.len());
        for required in [
            "XCD 21",
            "XCD 38V",
            "XCD 55V",
            "XCD 90V",
            "XCD 20-35E",
            "XCD 45 II",
        ] {
            assert!(catalog.iter().any(|name| name == required));
        }
    }

    #[test]
    fn fringe_picker_hue_is_stable_and_rejects_neutral_pixels() {
        let purple = rgb8_hsv([255, 0, 255]).expect("purple hue");
        let green = rgb8_hsv([0, 255, 0]).expect("green hue");
        assert!((purple.0 - 300.0).abs() < 0.01);
        assert!((green.0 - 120.0).abs() < 0.01);
        assert!(rgb8_hsv([240, 240, 240]).is_none());
    }
}
