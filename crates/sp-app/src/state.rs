//! Application state, UI-side. Owns the current mode, selection, and the
//! open image's `EditState`. Pipeline/decode results arrive as messages
//! (BUILD.md §3.3); no egui types in here so logic stays testable.

use sp_core::{preset::DevelopPreset, EditState, ImageMeta};
use std::path::PathBuf;
use std::time::Instant;

/// Follow-up a toast can offer, wired to an existing undo path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastAction {
    UndoFileMove,
    UndoSync,
}

impl ToastAction {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            ToastAction::UndoFileMove | ToastAction::UndoSync => "Undo",
        }
    }
}

/// A user-facing notification (errors, confirmations). Errors stay until
/// dismissed; others count down, and the countdown pauses while hovered.
#[derive(Debug)]
pub struct Toast {
    pub id: u64,
    pub message: String,
    pub is_error: bool,
    pub action: Option<ToastAction>,
    /// Seconds left on screen; ignored for errors.
    pub remaining: f32,
}

/// How long a non-error toast stays on screen.
pub const TOAST_TTL_SECS: f32 = 6.0;
/// Toasts shown at once; older ones collapse into a "+N more" line.
pub const TOAST_VISIBLE: usize = 3;

/// Before/after presentation in the viewport. `\` toggles the whole frame
/// independently of these.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CompareMode {
    #[default]
    Off,
    Split,
    SideBySide,
}

impl CompareMode {
    #[must_use]
    pub fn next(self) -> Self {
        match self {
            CompareMode::Off => CompareMode::Split,
            CompareMode::Split => CompareMode::SideBySide,
            CompareMode::SideBySide => CompareMode::Off,
        }
    }
}

/// Last completed Reference Match run, displayed in the wizard.
#[derive(Debug, Clone)]
pub struct ReferenceMatchReport {
    pub profile_name: String,
    pub paired_images: usize,
    pub samples: usize,
    pub mean_delta_e_00: f32,
    pub p95_delta_e_00: f32,
    pub all_embedded_icc: bool,
}

/// Top-level UI mode (Library ↔ Develop toggle in the top bar).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Mode {
    Library,
    #[default]
    Develop,
}

/// Develop sidebar content, independent of the Library folder rail.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LeftRailTab {
    #[default]
    Presets,
    Ai,
    Workflow,
}

/// How the AI rail builds a selection. Only one method's controls are shown
/// at a time so the steps for that method read top to bottom.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum MaskMethod {
    #[default]
    Subject,
    Describe,
    Click,
    Depth,
}

impl MaskMethod {
    pub const ALL: [Self; 4] = [Self::Subject, Self::Describe, Self::Click, Self::Depth];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Subject => "Subject",
            Self::Describe => "Describe",
            Self::Click => "Click",
            Self::Depth => "Depth",
        }
    }

    pub const fn hint(self) -> &'static str {
        match self {
            Self::Subject => "Find the sky, people or vehicles automatically.",
            Self::Describe => "Type what to select, such as \"the red car\".",
            Self::Click => "Click an object in the photo, then refine with points.",
            Self::Depth => "Select everything within a distance range.",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum MaskSubject {
    #[default]
    Sky,
    People,
    Cars,
    Motorcycles,
}

impl MaskSubject {
    pub const ALL: [Self; 4] = [Self::Sky, Self::People, Self::Cars, Self::Motorcycles];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Sky => "Sky",
            Self::People => "People",
            Self::Cars => "Cars",
            Self::Motorcycles => "Motorcycles",
        }
    }

    pub const fn request_kind(self) -> &'static str {
        match self {
            Self::Sky => "semantic_sky",
            Self::People => "semantic_people",
            Self::Cars => "semantic_cars",
            Self::Motorcycles => "semantic_motorcycles",
        }
    }
}

/// A transient proposal preview. Pixels are small; full-resolution masks are
/// generated only after the user chooses which instances need editing.
pub struct MaskCandidate {
    pub id: u32,
    pub confidence: f32,
    pub bounds: [f32; 4],
    pub preview: egui::ColorImage,
    pub texture: Option<egui::TextureHandle>,
    /// Coverage-only copy of `preview`, tinted in the viewport.
    pub overlay: Option<egui::TextureHandle>,
    pub selected: bool,
}

impl MaskCandidate {
    /// Canonical-image rectangle the preview covers. Both mask backends crop
    /// the detection bounds with an 8% margin before thumbnailing.
    pub fn preview_rect(&self) -> [f32; 4] {
        let [x, y, w, h] = self.bounds;
        let margin = w.max(h) * 0.08;
        [
            (x - margin).max(0.0),
            (y - margin).max(0.0),
            (x + w + margin).min(1.0),
            (y + h + margin).min(1.0),
        ]
    }

    /// Detected coverage at a canonical image position, 0..=1.
    pub fn coverage_at(&self, point: [f32; 2]) -> f32 {
        let [x0, y0, x1, y1] = self.preview_rect();
        if point[0] < x0 || point[1] < y0 || point[0] > x1 || point[1] > y1 {
            return 0.0;
        }
        let [width, height] = self.preview.size;
        if width == 0 || height == 0 {
            return 0.0;
        }
        let px = (((point[0] - x0) / (x1 - x0).max(1e-6)) * width as f32) as usize;
        let py = (((point[1] - y0) / (y1 - y0).max(1e-6)) * height as f32) as usize;
        let index = py.min(height - 1) * width + px.min(width - 1);
        f32::from(self.preview.pixels[index].a()) / 255.0
    }
}

impl std::fmt::Debug for MaskCandidate {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MaskCandidate")
            .field("id", &self.id)
            .field("confidence", &self.confidence)
            .field("bounds", &self.bounds)
            .field("selected", &self.selected)
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
pub struct MaskCandidates {
    pub source: PathBuf,
    pub generation: u64,
    pub before: EditState,
    pub kind: String,
    pub backend: String,
    pub items: Vec<MaskCandidate>,
}

/// Which right-panel tool groups are expanded. Defaults follow BUILD.md §5.2
/// (Basic open, the rest collapsed until used).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct PanelOpen {
    pub history: bool,
    pub basic: bool,
    pub curve: bool,
    pub color: bool,
    pub local_retouch: bool,
    pub detail: bool,
    pub lens: bool,
    pub effects: bool,
    pub resolution: bool,
    pub geometry: bool,
    pub hdr: bool,
    pub metadata: bool,
}

impl Default for PanelOpen {
    fn default() -> Self {
        Self {
            history: false,
            basic: true,
            curve: false,
            color: false,
            local_retouch: false,
            detail: false,
            lens: false,
            effects: false,
            resolution: false,
            geometry: false,
            hdr: false,
            metadata: false,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CurveChannel {
    #[default]
    Luma,
    Red,
    Green,
    Blue,
}

#[derive(Debug, Clone, Copy)]
pub struct CurvePipetteActive {
    pub channel: CurveChannel,
    pub x: f32,
    pub initial_y: f32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RetouchTool {
    #[default]
    None,
    Brush,
    MaskAdd,
    MaskSubtract,
    Radial,
    LinearGradient,
    Heal,
    Clone,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum GeometryTool {
    #[default]
    None,
    UprightVertical,
    UprightHorizontal,
    DistortionLine,
}

/// Transient authoring state; corrections themselves live in EditState.
#[derive(Debug, Clone)]
pub struct SpotUi {
    pub selected: Option<usize>,
    pub source: Option<[f32; 2]>,
    pub pick_source: bool,
    /// (correction index, source handle rather than target).
    pub drag_handle: Option<(usize, bool)>,
    pub opacity: f32,
    pub heal_texture: f32,
    pub source_request: Option<usize>,
    pub refine_request: Option<usize>,
    pub refine_after_stroke: bool,
    pub refine_busy: bool,
    pub refine_stroke_pending: bool,
    pub refine_radius: f32,
    pub refine_tolerance: f32,
    pub detect_dust: bool,
    pub dust_sensitivity: f32,
    pub dust_candidates: Vec<sp_core::retouch::DustCandidate>,
    pub dust_context: Option<std::sync::Arc<sp_core::source::RasterImage>>,
    pub dust_selected: Option<usize>,
    pub dust_apply: Option<usize>,
    pub aligned: bool,
    pub offset: Option<[f32; 2]>,
}

impl Default for SpotUi {
    fn default() -> Self {
        Self {
            selected: None,
            source: None,
            pick_source: false,
            drag_handle: None,
            opacity: 1.0,
            heal_texture: 100.0,
            source_request: None,
            refine_request: None,
            refine_after_stroke: false,
            refine_busy: false,
            refine_stroke_pending: false,
            refine_radius: 6.0,
            refine_tolerance: 0.08,
            detect_dust: false,
            dust_sensitivity: 0.5,
            dust_candidates: Vec::new(),
            dust_context: None,
            dust_selected: None,
            dust_apply: None,
            aligned: true,
            offset: None,
        }
    }
}

impl SpotUi {
    pub fn correction(
        &mut self,
        target: [f32; 2],
        radius: f32,
        feather: f32,
        clone: bool,
        stroke: bool,
    ) -> sp_core::edit::SpotCorrection {
        let source = if self.aligned && self.source.is_some() && self.offset.is_some() {
            let offset = self.offset.unwrap_or([0.0; 2]);
            [target[0] + offset[0], target[1] + offset[1]]
        } else if let Some(source) = self.source {
            self.offset = Some([source[0] - target[0], source[1] - target[1]]);
            source
        } else {
            [
                target[0]
                    + if target[0] > radius * 3.0 {
                        -radius * 2.5
                    } else {
                        radius * 2.5
                    },
                target[1],
            ]
        };
        sp_core::edit::SpotCorrection {
            target,
            source: source.map(|v| v.clamp(0.0, 1.0)),
            radius,
            feather,
            opacity: self.opacity.clamp(0.0, 1.0),
            heal_texture: self.heal_texture,
            enabled: true,
            clone,
            stroke: if stroke { vec![target] } else { Vec::new() },
        }
    }
}

/// The whole UI state. One instance, owned by `ShopPhotoApp`.
#[derive(Debug)]
pub struct ImportReviewEntry {
    pub path: PathBuf,
    pub selected: bool,
    pub captured_at: Option<String>,
    pub warning: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum VersionExportMode {
    #[default]
    Current,
    Primary,
    All,
    Selected,
}

#[derive(Debug, Default)]
pub struct AppState {
    pub sequence: crate::app::sequence::State,
    pub merge: crate::app::multiframe::State,
    pub tether: crate::app::tether::State,
    pub enhance_runtime: crate::app::enhance_runtime::State,
    pub auto_finish_requested: bool,
    pub auto_finish_cancel_requested: bool,
    pub auto_finish_busy: bool,
    pub auto_finish_pending: usize,
    pub auto_finish_saved: usize,
    pub auto_finish_analyzed: bool,
    pub auto_finish_progress: f32,
    pub auto_finish_status: String,
    pub auto_finish_errors: Vec<String>,
    pub command_console: crate::app::automation::Console,
    pub enhancement_requested: Option<String>,
    pub enhancement_busy: bool,
    pub enhancement_cancel_requested: bool,
    pub enhancement_status: String,
    pub upscale_model_pick_requested: bool,
    pub mask_import_requested: bool,
    pub mask_text_prompt: String,
    pub mask_subject: MaskSubject,
    pub mask_candidates: Option<MaskCandidates>,
    pub mask_method: MaskMethod,
    /// Candidate row under the pointer in the AI rail, emphasised in the viewport.
    pub mask_candidate_hover: Option<usize>,
    pub mask_point: [f32; 2],
    pub mask_point_pick: bool,
    pub mask_prompt_pick: Option<bool>,
    pub mask_prompts: Vec<([f32; 2], bool)>,
    pub mask_show_overlay: bool,
    pub mask_overlay_texture: Option<egui::TextureId>,
    pub lens_coordinate_map: sp_core::geometry::LensMap,
    pub mask_depth_min: f32,
    pub mask_depth_max: f32,
    pub language: crate::i18n::Language,
    pub lens_manager: crate::app::lens_manager::State,
    /// Scale currently applied to egui. The draft is edited at the old scale
    /// and only committed by the Apply button, so the control cannot move out
    /// from under the pointer while it is being fine-tuned.
    pub ui_scale: f32,
    pub ui_scale_draft: f32,
    pub mode: Mode,
    pub left_rail_tab: LeftRailTab,
    /// Currently open image.
    pub open_image: Option<PathBuf>,
    /// Metadata of the open image (arrives with the decode result).
    pub meta: Option<ImageMeta>,
    /// Source domain, independent of camera EXIF retained by rendered files.
    pub source_is_raw: bool,
    pub source_offline: bool,
    pub source_pinned: bool,
    pub xmp_revision: Option<std::sync::Arc<std::sync::Mutex<sp_catalog::xmp::XmpRevision>>>,
    pub catalog_tool_requested: Option<crate::app::catalog_tools::Action>,
    pub catalog_tool_busy: bool,
    pub catalog_tool_status: String,
    /// A decode is running; the viewport shows a loading state.
    pub decode_in_flight: bool,
    /// Active toasts, newest last. Pruned each frame by TTL.
    pub toasts: Vec<Toast>,
    /// Set by panels (Open button); consumed by the app in `logic`.
    pub open_dialog_requested: bool,
    /// Latest histogram bins. The first four 256-bin planes are R, G, B and
    /// true luminance; explicit optical analysis may append score planes.
    pub histogram: Option<Vec<u32>>,
    /// Before/after: render with a neutral edit while true (`\` toggles).
    pub show_original: bool,
    /// Selected HSL band index (0–7) in the Color panel.
    pub hsl_selected: usize,
    pub curve_channel: CurveChannel,
    /// Undo/redo stacks of edit snapshots. Slider drags are coalesced into
    /// one step by the gesture-gap rule in `record_edit_gesture`.
    pub undo_stack: Vec<EditState>,
    pub redo_stack: Vec<EditState>,
    /// Last edit value seen by the undo tracker.
    pub undo_last_seen: EditState,
    /// When the edit last changed (drives gesture coalescing + debounced
    /// sidecar saves).
    pub last_edit_change: Option<Instant>,
    /// Sidecar needs saving.
    pub sidecar_dirty: bool,
    /// Edit for the open image. Kept at neutral defaults when no image is open
    /// so controls and export presets remain inspectable.
    pub edit: EditState,
    pub panels: PanelOpen,
    /// Left/right rails hidden (Tab toggle, Lightroom-style).
    pub rails_hidden: bool,
    /// Viewport zoom: None = fit; Some(z) = screen px per full-res image px
    /// (1.0 = 100%).
    pub zoom: Option<f32>,
    /// View center in normalized active-area coordinates (pan position).
    pub view_center: (f32, f32),
    /// Viewport rect size in screen px, captured each frame by the panel
    /// so `logic` can compute the 1:1 tile region.
    pub viewport_size: (f32, f32),
    /// Active-area dimensions of the open image.
    pub active_size: Option<(u32, u32)>,
    /// Crop tool engaged (Geometry panel toggle); forces fit view.
    pub crop_mode: bool,
    /// Active on-canvas local-adjustment or spot-correction tool.
    pub retouch_tool: RetouchTool,
    pub retouch_active_local: Option<usize>,
    pub retouch_active_spot: Option<usize>,
    pub spot_ui: SpotUi,
    pub retouch_drag_origin: Option<[f32; 2]>,
    pub retouch_radius: f32,
    pub retouch_feather: f32,
    /// Local-adjustment index waiting for a color-range sample.
    pub color_range_picker: Option<usize>,
    pub color_range_sample_request: Option<(usize, f32, f32)>,
    /// On-canvas tone-curve targeted adjustment.
    pub curve_picker: bool,
    pub curve_sample_request: Option<(CurveChannel, f32, f32)>,
    pub curve_pipette_active: Option<CurvePipetteActive>,
    /// Guided upright and clean-room plumb-line calibration authoring.
    pub geometry_tool: GeometryTool,
    pub geometry_drag_origin: Option<[f32; 2]>,
    pub upright_guides: Vec<sp_core::edit::UprightGuide>,
    pub distortion_lines: Vec<sp_core::lens::StraightLineSample>,
    /// Session filmstrip: every file opened this run, in order.
    pub session: Vec<PathBuf>,
    /// Set by the filmstrip on click; consumed by the app in `logic`.
    pub filmstrip_clicked: Option<PathBuf>,
    /// Names of available camera profiles (for the Basic-panel dropdown).
    pub profile_names: Vec<String>,
    pub custom_look_names: Vec<String>,
    pub import_cube_requested: bool,
    /// Names of every loaded lens shading profile, for the override picker.
    pub lens_names: Vec<String>,
    /// Hasselblad XCD lens-identification catalog. These names help recover
    /// missing maker-note metadata but are not themselves correction grids.
    pub lens_catalog_names: Vec<String>,
    /// Name of the profile auto-matched to the open shot, if any. `None`
    /// with a lens recorded means "this lens is not calibrated yet".
    pub matched_lens: Option<String>,
    /// Set by the profile dropdown; consumed by the app in `logic`.
    pub import_dcp_requested: bool,
    /// The swapchain is float scRGB (Metal EDR active): HDR edits display
    /// with real headroom instead of the SDR fallback.
    pub hdr_surface: bool,
    /// View-only soft-proof state (never written to image sidecars).
    pub proof_space: sp_core::edit::ProofSpace,
    pub gamut_warning: bool,
    pub custom_proof_names: Vec<String>,
    pub selected_custom_proof: Option<String>,
    pub import_icc_requested: bool,
    pub custom_proof_generation: u64,
    pub proof_options: sp_pipeline::proof::ProofOptions,
    pub proof_busy: bool,
    /// 0 off, 1 shadow clipping, 2 highlight clipping, 3 both.
    pub clipping_mode: u32,
    /// Transient Alt-drag threshold view.
    pub alt_clipping: bool,
    pub sharpen_mask_preview: bool,
    pub auto_tone_requested: bool,
    pub auto_exposure_requested: bool,
    pub auto_defringe_requested: bool,
    pub auto_ca_requested: bool,
    pub auto_level_requested: bool,
    pub save_lens_geometry_requested: bool,
    /// Active image eyedropper target for a custom fringe hue.
    pub defringe_picker: Option<DefringePickerTarget>,
    /// Normalized active-image position captured by the viewport.
    pub defringe_sample_request: Option<(DefringePickerTarget, f32, f32)>,
    /// An automatic tone operation is waiting for a freshly rendered histogram.
    pub auto_in_flight: bool,
    /// Native Core ML RAW enhancement state shown in the Detail panel.
    pub ai_denoise_busy: bool,
    pub ai_denoise_available: bool,
    pub ai_denoise_status: Option<String>,
    pub ai_model_install_requested: bool,
    pub ai_model_install_busy: bool,
    pub ai_model_install_progress: f32,
    /// Denoise-only before/after; all other develop settings remain active.
    pub ai_compare_original: bool,
    /// Draw the untouched and AI-denoised RAW developments together in the
    /// viewport, separated by a draggable vertical divider.
    pub ai_compare_split: bool,
    /// Normalized horizontal position of the denoise comparison divider.
    pub ai_compare_position: f32,
    /// Last built-in recipe chosen. Kept while the user fine-tunes so the
    /// preset row can show a stable "modified" state instead of disappearing.
    pub active_preset: Option<DevelopPreset>,
    pub user_presets: Vec<sp_core::preset::UserPreset>,
    pub user_preset_name: String,
    pub user_preset_save_requested: bool,
    pub user_preset_import_requested: bool,
    pub user_preset_export_requested: bool,
    /// Hover-only look override; outer Some means a picker row is hovered.
    pub look_preview: Option<Option<String>>,
    /// The layer colorspace shim has been applied (macOS).
    pub edr_colorspace_applied: bool,
    /// Screen's current EDR headroom multiple (1.0 = none available).
    pub edr_headroom: f32,
    /// Panel's potential EDR headroom (capability, not moment): ~16 on XDR.
    pub edr_potential: f32,
    /// Reference Match modal workflow state.
    pub reference_wizard_open: bool,
    pub reference_raws: Vec<PathBuf>,
    pub reference_tiff_dir: Option<PathBuf>,
    pub reference_pick_raws_requested: bool,
    pub reference_pick_tiff_requested: bool,
    pub reference_fit_requested: bool,
    pub reference_fit_busy: bool,
    pub reference_report: Option<ReferenceMatchReport>,
    /// Phase-5 export dialog and worker progress.
    pub export_dialog_open: bool,
    pub export_settings: sp_export::ExportSettings,
    pub export_destination_requested: bool,
    pub export_watermark_requested: bool,
    pub export_profile_requested: bool,
    pub export_start_requested: bool,
    pub export_cancel_requested: bool,
    pub export_busy: bool,
    pub export_progress: f32,
    pub export_queue: sp_export::queue::ExportQueue,
    pub export_queue_open: bool,
    pub workflow: crate::app::workflow_ui::WorkflowUi,
    pub customize: crate::app::customize::State,
    pub diagnostics_open: bool,
    pub gallery: crate::app::gallery::State,
    pub print_dialog_open: bool,
    pub print_layout: sp_export::print::PrintLayout,
    pub print_choose_requested: bool,
    pub print_open_viewer: bool,
    pub export_queue_loaded: bool,
    pub export_queue_dirty: bool,
    pub export_queue_error: Option<String>,
    pub export_enqueue_requested: bool,
    pub export_enqueue_busy: bool,
    pub export_recipe_names: std::collections::BTreeSet<String>,
    pub export_version_mode: VersionExportMode,
    pub export_version_choices: std::collections::HashSet<(PathBuf, String)>,
    pub export_batch: bool,
    pub export_presets: Vec<sp_export::ExportPreset>,
    pub export_preset_name: String,
    pub export_preset_save_requested: bool,
    /// The source-aware first export size is applied once per app session;
    /// subsequent dialog opens preserve the photographer's last choice.
    pub export_defaults_applied: bool,
    /// Prevent repeatedly replacing a format the user explicitly chose while
    /// the current edit is targeting HDR.
    pub export_hdr_defaults_applied: bool,
    pub external_edit_requested: bool,
    pub external_editor_pick_requested: bool,
    pub external_editor: Option<PathBuf>,
    pub external: crate::app::external::RecipeState,
    /// Phase-7 library/catalog state.
    pub library_add_folder_requested: bool,
    pub library_refresh_requested: bool,
    /// Folder id queued for non-destructive removal from the catalog.
    pub library_remove_folder_requested: Option<i64>,
    pub catalog_scanning: bool,
    pub catalog_scan_count: usize,
    pub catalog_photos: Vec<sp_catalog::CatalogPhoto>,
    pub catalog_folders: Vec<sp_catalog::FolderRecord>,
    /// Last scan error per folder. Existing catalog rows stay visible when a
    /// disconnected network share cannot be enumerated.
    pub catalog_folder_errors: std::collections::HashMap<PathBuf, String>,
    pub catalog_collections: Vec<sp_catalog::Collection>,
    pub catalog_smart_collections: Vec<sp_catalog::SmartCollection>,
    pub library_filter: sp_catalog::LibraryFilter,
    pub library_sort: sp_catalog::LibrarySort,
    pub thumbnail_requests: Vec<PathBuf>,
    /// Files whose thumbnail could not be decoded, with the reason. The
    /// grid and filmstrip show a "No preview" tile instead of a shimmer.
    pub thumbnail_failures: std::collections::HashMap<PathBuf, ThumbnailFailure>,
    /// Paths painted by the virtualized grid/filmstrip in the last frame.
    pub visible_thumbnail_paths: Vec<PathBuf>,
    /// Source-of-truth annotations loaded/saved with the current sidecar.
    pub annotations: sp_catalog::Annotations,
    pub descriptive_metadata: sp_catalog::DescriptiveMetadata,
    pub snapshots: Vec<sp_catalog::NamedSnapshot>,
    pub snapshot_name: String,
    pub persistent_history: Vec<sp_catalog::HistoryEntry>,
    pub virtual_copy: Option<String>,
    pub virtual_copy_name: Option<String>,
    pub virtual_copy_name_draft: String,
    pub virtual_copy_names: std::collections::HashMap<String, String>,
    pub virtual_copy_rename_requested: bool,
    pub virtual_copy_trash_requested: bool,
    pub virtual_copy_trash_confirm: bool,
    pub virtual_copy_open_requested: Option<(PathBuf, String)>,
    pub pending_virtual_copy: Option<(PathBuf, String)>,
    pub virtual_copies: Vec<String>,
    pub virtual_copy_create_requested: bool,
    /// Empty string selects the primary edit; otherwise a virtual-copy id.
    pub virtual_copy_switch_requested: Option<String>,
    /// Mirror ratings/flags into an `.xmp` sidecar on every save.
    /// Persisted in the app preferences.
    pub write_xmp: bool,
    pub raw_denoise_compute: sp_ai::ComputeUnits,
    pub new_collection_name: String,
    pub new_collection_parent: Option<i64>,
    pub library_collection: Option<i64>,
    pub collection_create_requested: bool,
    pub new_smart_collection_name: String,
    pub smart_collection_save_requested: bool,
    pub file_rename_pattern: String,
    pub file_rename_requested: bool,
    pub file_move_requested: bool,
    pub file_trash_requested: bool,
    pub file_trash_confirm_open: bool,
    pub file_move_undo_requested: bool,
    pub file_move_undo_available: bool,
    pub import_dialog_open: bool,
    pub import_source: Option<PathBuf>,
    pub import_destination: Option<PathBuf>,
    pub import_backup: Option<PathBuf>,
    pub import_detected_sources: Vec<PathBuf>,
    /// Rescan mounted volumes for camera cards off the UI thread.
    pub import_sources_scan_requested: bool,
    pub import_sources_scanning: bool,
    pub import_filename_pattern: String,
    pub import_preset_name: Option<String>,
    pub import_skip_duplicates: bool,
    pub import_source_requested: bool,
    pub import_destination_requested: bool,
    pub import_backup_requested: bool,
    pub import_start_requested: bool,
    pub import_cancel_requested: bool,
    pub import_busy: bool,
    pub import_progress: f32,
    pub import_review_requested: bool,
    pub import_review_busy: bool,
    pub import_review_source: Option<PathBuf>,
    pub import_review: Vec<ImportReviewEntry>,
    pub import_folder_layout: sp_catalog::workflow::ImportFolderLayout,
    pub import_metadata: sp_catalog::DescriptiveMetadata,
    pub import_keywords_draft: String,
    pub collection_add_requested: Option<(i64, PathBuf)>,
    pub dragged_photo: Option<PathBuf>,
    /// Command/Ctrl-click multi-selection used by batch export.
    pub selected_photos: std::collections::HashSet<PathBuf>,
    pub expanded_stacks: std::collections::HashSet<i64>,
    pub preview_quality: std::collections::HashMap<PathBuf, sp_catalog::quality::PreviewQuality>,
    pub detail_sort: bool,
    pub similarity_reference: Option<PathBuf>,
    pub auto_stack_open: bool,
    pub auto_stack_mode: sp_catalog::workflow::AutoStackMode,
    pub auto_stack_span_s: u32,
    pub auto_stack_suggestions: Vec<(bool, Vec<PathBuf>)>,
    pub settings_groups: sp_core::preset::EditGroups,
    pub sync_settings_requested: bool,
    pub undo_sync_requested: bool,
    pub shortcuts_open: bool,
    pub debug_overlay: bool,
    /// Active side-by-side culling session, if any.
    pub culling: Option<CullingState>,
    pub culling_requested: bool,
    pub culling_exit_requested: bool,
    /// Double-click in a culling cell: open that photo in Develop.
    pub culling_open_requested: Option<PathBuf>,
    pub culling_annotate: Option<CullingAnnotate>,
    /// Before/after split or side-by-side for the whole edit.
    pub compare_mode: CompareMode,
    /// What "before" means: `None` is the unedited original, otherwise a
    /// named snapshot or history step.
    pub compare_before: Option<(String, EditState)>,
    /// Locked crop aspect ratio (width / height in pixels) while cropping.
    pub crop_aspect: Option<f32>,
    /// Crop rectangle when the crop tool was entered, for Cancel / Esc.
    pub crop_saved: Option<[f32; 4]>,
    /// Inspector groups switched off for a quick comparison (render only).
    pub bypass_mask: u16,
    /// Opening one inspector card closes the others.
    pub solo_panels: bool,
    /// Inspector tool search text.
    pub inspector_query: String,
    /// Section the jump bar asked to scroll to, consumed by the inspector.
    pub inspector_jump: Option<usize>,
    pub inspector_focus_card: Option<String>,
    /// Section currently at the top of the inspector scroll.
    pub inspector_section: usize,
    /// Inspector cards whose "Advanced" controls are expanded.
    pub advanced_open: u16,
    /// Preset hovered in the presets rail, previewed without an undo step.
    pub preset_preview: Option<DevelopPreset>,
    /// The edit before the active preset was applied, for Amount.
    pub preset_base: Option<EditState>,
    /// Preset strength, 0–100.
    pub preset_amount: f32,
    /// History step (or snapshot) hovered in the history card.
    pub history_preview: Option<EditState>,
    /// History entry the edit was restored to; later entries are redo.
    pub history_cursor: Option<usize>,
    /// The edit before the gesture currently being recorded.
    pub history_gesture_base: Option<EditState>,
    /// White-balance eyedropper is waiting for a click in the image.
    pub wb_picker: bool,
    pub film_base_picker: bool,
    pub sample_picker: bool,
    pub sample_drag: Option<usize>,
    pub scopes_enabled: bool,
    pub analysis_dirty: bool,
    pub analysis_frame: Option<std::sync::Arc<sp_core::source::RasterImage>>,
    pub film_base_sample_request: Option<[f32; 2]>,
    pub wb_sample_request: Option<(f32, f32)>,
    /// Color-mixer tab: 0 hue, 1 saturation, 2 luminance.
    pub hsl_tab: u8,
    /// Toast countdowns are paused (pointer over the toast stack).
    pub toasts_paused: bool,
    pub toast_action_requested: Option<ToastAction>,
    pub next_toast_id: u64,
    /// Grid order of the photos shown by the Library, for keyboard
    /// navigation and range selection.
    pub library_visible: Vec<PathBuf>,
    /// Keyboard focus and range anchor in the Library grid.
    pub grid_focus: Option<PathBuf>,
    pub grid_anchor: Option<PathBuf>,
    /// Rating / flag / label change for Library photos, applied by the app.
    pub library_annotate: Option<(Annotate, Vec<PathBuf>)>,
    pub frame_ms: f32,
    /// Last main-thread stall report from the watchdog, for the F12 overlay.
    pub last_stall: Option<String>,
    pub gpu_texture_mb: f32,
    pub render_count: u64,
    pub welcome_open: bool,
    pub first_edit_tip: bool,
    pub update_available: Option<(String, String, String)>,
}

/// Side-by-side culling of two to four photos (UX-01).
#[derive(Debug, Clone)]
pub struct CullingState {
    pub paths: Vec<PathBuf>,
    pub focused: usize,
    /// Pan and zoom apply to every cell, or only to the focused one.
    pub locked: bool,
    /// Per-cell view centre (normalised) used when unlocked.
    pub centers: Vec<(f32, f32)>,
    /// Visible region of each cell in its own active-area pixels, written
    /// by the viewport and read by the app to render 1:1 tiles.
    pub regions: Vec<Option<(u32, u32, u32, u32)>>,
}

impl CullingState {
    #[must_use]
    pub fn new(paths: Vec<PathBuf>) -> Self {
        let n = paths.len();
        Self {
            paths,
            focused: 0,
            locked: true,
            centers: vec![(0.5, 0.5); n],
            regions: vec![None; n],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CullingAnnotate {
    Flag(sp_catalog::Flag),
    Rating(u8),
}

/// A rating, flag or color-label change for several Library photos.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Annotate {
    Rating(u8),
    Flag(sp_catalog::Flag),
    /// Toggles: when every target already has this label it is cleared.
    Label(sp_catalog::ColorLabel),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefringePickerTarget {
    Purple,
    Green,
}

impl AppState {
    /// Start a new preset/preview from the original creative edit, while
    /// honoring HDR settings changed since that baseline was captured.
    pub(crate) fn builtin_preset_base(&self) -> EditState {
        let mut base = match (self.active_preset, &self.preset_base) {
            (Some(_), Some(base)) => base.clone(),
            _ => self.edit.clone(),
        };
        base.hdr = self.edit.hdr;
        base
    }

    /// Element detections still describe the open photo and its current edit.
    pub fn mask_candidates_current(&self) -> bool {
        self.mask_candidates.as_ref().is_some_and(|candidates| {
            self.open_image.as_ref() == Some(&candidates.source) && candidates.before == self.edit
        })
    }

    /// Keyboard shortcuts that only mutate state (BUILD.md §5.2). Rendering-
    /// dependent shortcuts (before/after `\`) land with the pipeline.
    pub fn toggle_rails(&mut self) {
        self.rails_hidden = !self.rails_hidden;
    }

    pub fn toggle_zoom(&mut self) {
        self.zoom = match self.zoom {
            None => Some(1.0),
            Some(_) => None,
        };
    }

    /// Reset pan/zoom when a new image opens.
    pub fn reset_view(&mut self) {
        self.zoom = None;
        self.view_center = (0.5, 0.5);
        self.crop_mode = false;
    }

    /// The effective zoom given the current viewport, plus the fit scale.
    /// Returns `(zoom, fit_scale)`; zoom == fit_scale when in fit mode.
    pub fn effective_zoom(&self) -> Option<(f32, f32)> {
        let (aw, ah) = self.active_size?;
        let (vw, vh) = self.viewport_size;
        if vw < 1.0 || vh < 1.0 {
            return None;
        }
        let fit = (vw / aw as f32).min(vh / ah as f32).min(1.0);
        let z = match self.zoom {
            None => fit,
            Some(z) => z.clamp(fit.min(1.0) * 0.5, 6.0),
        };
        Some((z, fit))
    }

    /// Pull the pan center back onto what is actually displayed.
    /// [`Self::visible_region`] clamps the view inside the image, so a pan
    /// into an edge leaves `view_center` drifting past the shown region;
    /// the next cursor-anchored zoom or pan then jumps, and the drag has to
    /// be "unwound" before the image moves again. Only a real divergence
    /// (more than a pixel) is corrected, so slow sub-pixel drags at high
    /// zoom keep accumulating.
    pub fn settle_view_center(&mut self) {
        let (Some((aw, ah)), Some((x, y, w, h))) = (self.active_size, self.visible_region()) else {
            return;
        };
        let shown = (x as f32 + w as f32 / 2.0, y as f32 + h as f32 / 2.0);
        let stored = (
            self.view_center.0 * aw as f32,
            self.view_center.1 * ah as f32,
        );
        if (shown.0 - stored.0).abs() > 1.5 {
            self.view_center.0 = shown.0 / aw as f32;
        }
        if (shown.1 - stored.1).abs() > 1.5 {
            self.view_center.1 = shown.1 / ah as f32;
        }
    }

    /// Visible region in active-area pixel coordinates for the current
    /// pan/zoom, clamped inside the image. `None` when nothing is open.
    pub fn visible_region(&self) -> Option<(u32, u32, u32, u32)> {
        let (aw, ah) = self.active_size?;
        let (z, _) = self.effective_zoom()?;
        let (vw, vh) = self.viewport_size;
        let w = ((vw / z).round() as u32).clamp(1, aw);
        let h = ((vh / z).round() as u32).clamp(1, ah);
        let cx = (self.view_center.0 * aw as f32).round() as i64;
        let cy = (self.view_center.1 * ah as f32).round() as i64;
        let x = (cx - i64::from(w) / 2).clamp(0, i64::from(aw - w)) as u32;
        let y = (cy - i64::from(h) / 2).clamp(0, i64::from(ah - h)) as u32;
        Some((x, y, w, h))
    }

    /// Gap between input gestures that separates undo steps.
    pub const UNDO_GESTURE_GAP_SECS: f32 = 0.5;
    /// Maximum retained undo steps.
    pub const UNDO_CAP: usize = 200;

    /// Call once per frame. Detects edit changes, maintains the undo stack
    /// (new gesture ⇒ snapshot pushed; continuous drags coalesce), and
    /// marks the sidecar dirty. Returns true if the edit changed this frame.
    pub fn record_edit_gesture(&mut self, now: Instant) -> bool {
        if self.decode_in_flight || self.edit == self.undo_last_seen {
            return false;
        }
        let new_gesture = self
            .last_edit_change
            .is_none_or(|t| now.duration_since(t).as_secs_f32() > Self::UNDO_GESTURE_GAP_SECS);
        if new_gesture {
            let before = self.undo_last_seen.clone();
            self.push_history(&before, None);
            self.undo_stack.push(before);
            if self.undo_stack.len() > Self::UNDO_CAP {
                self.undo_stack.remove(0);
            }
            self.redo_stack.clear();
        } else if let Some(base) = &self.history_gesture_base {
            // A continuing drag: the step's result and label follow it.
            let label = crate::edit_groups::describe(base, &self.edit);
            if let Some(last) = self.persistent_history.last_mut() {
                last.label = label;
                last.edit = self.edit.clone();
            }
        }
        self.undo_last_seen = self.edit.clone();
        self.last_edit_change = Some(now);
        self.sidecar_dirty = true;
        true
    }

    fn now_s() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            .min(i64::MAX as u64) as i64
    }

    /// Record the step from `before` to the current edit. History entries
    /// hold the state *after* each step, labelled with what changed; the
    /// first entry of a photo is the state it was opened in.
    fn push_history(&mut self, before: &EditState, label: Option<String>) {
        if let Some(cursor) = self.history_cursor.take() {
            // Editing after jumping back discards the redo branch.
            self.persistent_history.truncate(cursor + 1);
        }
        let created_s = Self::now_s();
        if self.persistent_history.is_empty() {
            self.persistent_history.push(sp_catalog::HistoryEntry {
                label: "Opened".into(),
                created_s,
                edit: before.clone(),
            });
        }
        self.persistent_history.push(sp_catalog::HistoryEntry {
            label: label.unwrap_or_else(|| crate::edit_groups::describe(before, &self.edit)),
            created_s,
            edit: self.edit.clone(),
        });
        while self.persistent_history.len() > Self::UNDO_CAP {
            self.persistent_history.remove(0);
        }
        self.history_gesture_base = Some(before.clone());
    }

    /// Restore a history entry as one undoable step without adding a new
    /// entry; later entries stay visible as redo until the next edit.
    pub fn jump_to_history(&mut self, index: usize) {
        let Some(entry) = self.persistent_history.get(index) else {
            return;
        };
        let target = entry.edit.clone();
        if target != self.edit {
            self.undo_stack.push(self.edit.clone());
            self.redo_stack.clear();
            self.edit = target.clone();
            self.undo_last_seen = target;
            self.sidecar_dirty = true;
        }
        self.history_cursor = Some(index);
        self.history_gesture_base = None;
        self.last_edit_change = None;
    }

    /// Index of the history entry the current edit corresponds to.
    #[must_use]
    pub fn history_current(&self) -> Option<usize> {
        self.history_cursor
            .or_else(|| self.persistent_history.len().checked_sub(1))
    }

    pub fn undo(&mut self) {
        if let Some(prev) = self.undo_stack.pop() {
            self.redo_stack.push(self.edit.clone());
            self.edit = prev.clone();
            self.undo_last_seen = prev;
            self.sidecar_dirty = true;
        }
    }

    pub fn redo(&mut self) {
        if let Some(next) = self.redo_stack.pop() {
            self.undo_stack.push(self.edit.clone());
            self.edit = next.clone();
            self.undo_last_seen = next;
            self.sidecar_dirty = true;
        }
    }

    /// Reset undo/sidecar tracking when a (new) image's edit is loaded.
    pub fn adopt_edit(&mut self, edit: EditState) {
        self.edit = edit.clone();
        self.undo_last_seen = edit;
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.last_edit_change = None;
        self.sidecar_dirty = false;
        self.show_original = false;
        self.history_cursor = None;
        self.history_gesture_base = None;
        self.history_preview = None;
        self.compare_before = None;
        self.bypass_mask = 0;
        self.preset_base = None;
        self.preset_preview = None;
        self.crop_saved = None;
    }

    /// Retire the outgoing document before the incoming photo is decoded.
    /// Its path becomes selectable immediately, but its saved edit is not
    /// available yet and must never inherit undo, metadata or versions.
    pub fn begin_decode(&mut self, path: PathBuf) {
        self.mask_candidates = None;
        self.mask_prompts.clear();
        self.mask_point_pick = false;
        self.mask_prompt_pick = None;
        self.adopt_edit(EditState::default());
        self.annotations = Default::default();
        self.descriptive_metadata = Default::default();
        self.snapshots.clear();
        self.persistent_history.clear();
        self.virtual_copy = None;
        self.virtual_copy_name = None;
        self.virtual_copy_name_draft.clear();
        self.virtual_copies.clear();
        self.virtual_copy_names.clear();
        self.active_size = None;
        self.histogram = None;
        self.decode_in_flight = true;
        self.open_image = Some(path);
    }

    /// Replace the edit as one explicit undo step (buttons such as Auto).
    pub fn apply_command(
        &mut self,
        command: &sp_core::commands::EditCommand,
    ) -> Result<(), sp_core::SpError> {
        let next = command.apply(&self.edit)?;
        self.apply_edit_step_labeled(next, Some(command.id().into()));
        Ok(())
    }

    pub fn apply_edit_step(&mut self, edit: EditState) {
        self.apply_edit_step_labeled(edit, None);
    }

    /// [`Self::apply_edit_step`] with an explicit history label, for steps
    /// whose field diff would read poorly ("Preset: Film Warm").
    pub fn apply_edit_step_labeled(&mut self, edit: EditState, label: Option<String>) {
        if edit == self.edit {
            return;
        }
        let before = std::mem::replace(&mut self.edit, edit.clone());
        self.push_history(&before, label);
        self.undo_stack.push(before);
        if self.undo_stack.len() > Self::UNDO_CAP {
            self.undo_stack.remove(0);
        }
        self.redo_stack.clear();
        self.undo_last_seen = edit;
        self.last_edit_change = Some(Instant::now());
        self.sidecar_dirty = true;
    }

    pub fn push_toast(&mut self, message: impl Into<String>, is_error: bool) {
        self.push_toast_with_action(message, is_error, None);
    }

    pub fn push_toast_with_action(
        &mut self,
        message: impl Into<String>,
        is_error: bool,
        action: Option<ToastAction>,
    ) {
        self.next_toast_id += 1;
        self.toasts.push(Toast {
            id: self.next_toast_id,
            message: message.into(),
            is_error,
            action,
            remaining: TOAST_TTL_SECS,
        });
    }

    pub fn dismiss_toast(&mut self, id: u64) {
        self.toasts.retain(|toast| toast.id != id);
    }

    /// Count down non-error toasts by `dt` seconds unless paused, and drop
    /// expired ones; returns true if any remain (repaint needed).
    pub fn prune_toasts(&mut self, dt: f32) -> bool {
        if !self.toasts_paused {
            for toast in &mut self.toasts {
                if !toast.is_error {
                    toast.remaining -= dt;
                }
            }
        }
        self.toasts.retain(|t| t.is_error || t.remaining > 0.0);
        !self.toasts.is_empty()
    }
}

/// Why a grid/filmstrip thumbnail could not be shown.
#[derive(Clone, Debug)]
pub struct ThumbnailFailure {
    pub error: String,
    /// The file no longer exists at its catalog path.
    pub missing: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidate_hits_follow_the_preview_coverage_inside_its_margin() {
        // 10×10 preview: left half covered, right half empty.
        let pixels = (0..100)
            .map(|i| {
                if i % 10 < 5 {
                    egui::Color32::WHITE
                } else {
                    egui::Color32::TRANSPARENT
                }
            })
            .collect();
        let candidate = MaskCandidate {
            id: 1,
            confidence: 0.9,
            bounds: [0.4, 0.4, 0.2, 0.2],
            preview: egui::ColorImage::new([10, 10], pixels),
            texture: None,
            overlay: None,
            selected: false,
        };
        let rect = candidate.preview_rect();
        assert!((rect[0] - 0.384).abs() < 1e-6 && (rect[2] - 0.616).abs() < 1e-6);
        assert_eq!(candidate.coverage_at([0.45, 0.5]), 1.0);
        assert_eq!(candidate.coverage_at([0.55, 0.5]), 0.0);
        assert_eq!(candidate.coverage_at([0.1, 0.1]), 0.0);
    }

    #[test]
    fn clone_alignment_and_independent_source_modes() {
        let mut ui = SpotUi {
            source: Some([0.2, 0.3]),
            ..Default::default()
        };
        let first = ui.correction([0.5, 0.5], 0.03, 0.5, true, false);
        assert_eq!(first.source, [0.2, 0.3]);
        let second = ui.correction([0.6, 0.6], 0.03, 0.5, true, false);
        assert!((second.source[0] - 0.3).abs() < 1e-6);
        assert!((second.source[1] - 0.4).abs() < 1e-6);
        ui.aligned = false;
        assert_eq!(
            ui.correction([0.6, 0.6], 0.03, 0.5, true, false).source,
            [0.2, 0.3]
        );
    }

    #[test]
    fn retouch_editing_is_undoable_without_changing_original_source() {
        let mut state = AppState::default();
        let correction = state.spot_ui.correction([0.5, 0.5], 0.03, 0.5, false, true);
        let mut edit = state.edit.clone();
        edit.spots.push(correction);
        state.apply_edit_step_labeled(edit, Some("Heal spot".into()));
        let mut edit = state.edit.clone();
        edit.spots[0].source = [0.15, 0.3];
        edit.spots[0].opacity = 0.5;
        state.apply_edit_step_labeled(edit, Some("Refine source".into()));
        assert_eq!(state.edit.spots[0].source, [0.15, 0.3]);
        state.undo();
        assert_eq!(state.edit.spots[0].opacity, 1.0);
        state.undo();
        assert!(state.edit.spots.is_empty());
        state.redo();
        state.redo();
        assert_eq!(state.edit.spots[0].source, [0.15, 0.3]);
    }

    #[test]
    fn undo_coalesces_drags_and_round_trips() {
        let mut s = AppState::default();
        let t0 = Instant::now();
        // A drag: many changes in quick succession = ONE undo step.
        for i in 1..=5 {
            s.edit.tone.exposure_ev = i as f32 * 0.2;
            s.record_edit_gesture(t0);
        }
        assert_eq!(s.undo_stack.len(), 1);
        // A second gesture after the gap.
        let t1 = t0 + std::time::Duration::from_secs(2);
        s.edit.tone.contrast = 30.0;
        s.record_edit_gesture(t1);
        assert_eq!(s.undo_stack.len(), 2);

        s.undo();
        assert_eq!(s.edit.tone.contrast, 0.0);
        assert_eq!(s.edit.tone.exposure_ev, 1.0);
        s.undo();
        assert_eq!(s.edit.tone.exposure_ev, 0.0);
        s.redo();
        assert_eq!(s.edit.tone.exposure_ev, 1.0);
        s.redo();
        assert_eq!(s.edit.tone.contrast, 30.0);
    }

    #[test]
    fn adopt_edit_resets_tracking() {
        let mut s = AppState::default();
        s.edit.tone.exposure_ev = 1.0;
        s.record_edit_gesture(Instant::now());
        let mut loaded = EditState::default();
        loaded.tone.contrast = 10.0;
        s.adopt_edit(loaded.clone());
        assert_eq!(s.edit, loaded);
        assert!(s.undo_stack.is_empty());
        assert!(!s.sidecar_dirty);
    }

    #[test]
    fn starting_decode_cannot_carry_the_outgoing_document_to_the_new_photo() {
        let mut state = AppState {
            open_image: Some("outgoing.dng".into()),
            virtual_copy: Some("copy-0001".into()),
            virtual_copy_name: Some("Portrait".into()),
            active_size: Some((6000, 4000)),
            ..Default::default()
        };
        state.edit.tone.exposure_ev = 2.0;
        state.record_edit_gesture(Instant::now());
        state.annotations.rating = 5;
        state.descriptive_metadata.caption = "Outgoing photo".into();
        state.begin_decode("incoming.dng".into());
        assert_eq!(
            state.open_image.as_deref(),
            Some(std::path::Path::new("incoming.dng"))
        );
        assert!(state.decode_in_flight);
        assert_eq!(state.edit, EditState::default());
        assert_eq!(state.annotations.rating, 0);
        assert!(state.descriptive_metadata.caption.is_empty());
        assert!(state.virtual_copy.is_none());
        assert!(state.virtual_copy_name.is_none());
        assert!(state.active_size.is_none());
        assert!(state.undo_stack.is_empty());
        assert!(state.persistent_history.is_empty());
        // Late controls or cleanup of camera-specific values cannot make
        // an edit saveable before the incoming sidecar has been loaded.
        state.edit.tone.exposure_ev = 1.0;
        assert!(!state.record_edit_gesture(Instant::now()));
        assert!(!state.sidecar_dirty);
        let mut incoming = EditState::default();
        incoming.tone.exposure_ev = -0.5;
        state.decode_in_flight = false;
        state.adopt_edit(incoming.clone());
        state.undo();
        assert_eq!(state.edit, incoming);
    }

    #[test]
    fn settling_the_view_center_undoes_edge_drift_but_keeps_subpixel_pans() {
        // Dragged well past the left/top edge: the region is clamped at 0.
        let mut s = AppState {
            active_size: Some((4000, 3000)),
            viewport_size: (1000.0, 800.0),
            zoom: Some(1.0),
            view_center: (0.02, 0.01),
            ..Default::default()
        };
        s.settle_view_center();
        let (x, y, w, h) = s.visible_region().unwrap();
        assert_eq!((x, y), (0, 0));
        assert!((s.view_center.0 * 4000.0 - w as f32 / 2.0).abs() < 1e-3);
        assert!((s.view_center.1 * 3000.0 - h as f32 / 2.0).abs() < 1e-3);
        // A sub-pixel pan in the interior is left alone.
        s.view_center = (0.5 + 0.3 / 4000.0, 0.5);
        let before = s.view_center;
        s.settle_view_center();
        assert_eq!(s.view_center, before);
    }

    #[test]
    fn zoom_toggles_between_fit_and_one_to_one() {
        let mut s = AppState::default();
        assert_eq!(s.zoom, None);
        s.toggle_zoom();
        assert_eq!(s.zoom, Some(1.0));
        s.toggle_zoom();
        assert_eq!(s.zoom, None);
    }

    #[test]
    fn switching_and_previewing_presets_honors_hdr_changed_after_first_preset() {
        use sp_core::edit::HdrTarget;

        for initially_enabled in [false, true] {
            let mut original = EditState::default();
            original.hdr.enabled = initially_enabled;
            original.hdr.target = if initially_enabled {
                HdrTarget::Hdr
            } else {
                HdrTarget::Sdr
            };
            original.tone.contrast = 7.0;
            let mut state = AppState {
                preset_base: Some(original.clone()),
                edit: original.clone(),
                active_preset: Some(DevelopPreset::GoldenHourRadiance),
                ..Default::default()
            };
            DevelopPreset::GoldenHourRadiance.apply(&mut state.edit);
            state.edit.hdr.enabled = !initially_enabled;
            state.edit.hdr.target = if initially_enabled {
                HdrTarget::Sdr
            } else {
                HdrTarget::Hdr
            };
            state.edit.hdr.headroom_stops = 1.25;
            state.edit.hdr.scene_white_ev = 0.25;
            state.edit.hdr.highlight_rolloff = 32.0;
            let current_hdr = state.edit.hdr;

            let base = state.builtin_preset_base();
            assert_eq!(base.hdr, current_hdr);
            assert_eq!(base.tone, original.tone);
            let mut preview = base.clone();
            DevelopPreset::NeonNight.apply(&mut preview);
            assert_eq!(preview.hdr.enabled, current_hdr.enabled);
            assert_eq!(preview.hdr.target, current_hdr.target);
            assert_eq!(preview.hdr.scene_white_ev, current_hdr.scene_white_ev);
            if !current_hdr.enabled {
                assert_eq!(preview.hdr, current_hdr);
            }
            // Committing captures this refreshed baseline for the Amount slider.
            state.preset_base = Some(base.clone());
            state.edit = preview.clone();
            assert_eq!(
                crate::edit_groups::blend(&base, &preview, 0.0).hdr,
                current_hdr
            );
            assert_eq!(crate::edit_groups::blend(&base, &preview, 1.0), preview);
            assert_eq!(original.hdr.enabled, initially_enabled);
        }
    }

    #[test]
    fn inactive_preset_does_not_restore_a_stale_baseline_after_reset() {
        let mut stale = EditState::default();
        stale.tone.exposure_ev = 1.5;
        let state = AppState {
            preset_base: Some(stale),
            active_preset: None,
            edit: EditState::default(),
            ..Default::default()
        };
        assert_eq!(state.builtin_preset_base(), state.edit);
    }
}
