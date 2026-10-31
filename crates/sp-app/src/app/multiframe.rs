//! Selected-photo HDR, burst and focus masters with explicit provenance.
use super::*;
use sp_core::multiframe::{MergeKind, MergeOptions};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

#[derive(Debug)]
pub(crate) struct State {
    pub open: bool,
    pub requested: bool,
    pub preview_requested: bool,
    pub repairs: Vec<sp_core::multiframe::FocusRepair>,
    pub focus_source: usize,
    pub selected_repair: Option<usize>,
    preview: Option<FocusPreview>,
    preview_source: bool,
    pub busy: bool,
    pub cancel_requested: bool,
    pub kind: MergeKind,
    pub reference: usize,
    pub align: bool,
    pub motion_threshold: f32,
    pub exposure_values: String,
    recipe_picker: Option<Receiver<Result<Option<FocusRecipe>, String>>>,
    pub panorama: sp_core::panorama::Options,
    pub pixel_shift: sp_core::pixel_shift::Descriptor,
    pub pixel_shift_confirmed: bool,
    pub astro: AstroPaths,
    pub sigma: f32,
    calibration_picker: Option<(usize, Receiver<Option<Vec<PathBuf>>>)>,
    pub sources: Vec<PathBuf>,
    pub progress: f32,
    pub status: String,
}

impl Default for State {
    fn default() -> Self {
        Self {
            open: false,
            requested: false,
            preview_requested: false,
            repairs: Vec::new(),
            focus_source: 0,
            selected_repair: None,
            preview: None,
            preview_source: false,
            busy: false,
            cancel_requested: false,
            kind: MergeKind::Hdr,
            reference: 0,
            align: true,
            motion_threshold: 0.15,
            exposure_values: String::new(),
            recipe_picker: None,
            panorama: sp_core::panorama::Options::default(),
            pixel_shift: Default::default(),
            pixel_shift_confirmed: false,
            astro: AstroPaths::default(),
            sigma: 3.0,
            calibration_picker: None,
            sources: Vec::new(),
            progress: 0.0,
            status: String::new(),
        }
    }
}

#[derive(Debug, Clone, Default, serde::Serialize)]
pub(crate) struct AstroPaths {
    groups: [Vec<PathBuf>; 4],
    equipment_confirmed: bool,
}

fn calibration_master(
    paths: &[PathBuf],
    cache: Option<&std::path::Path>,
    cancel: &AtomicBool,
) -> Result<Option<sp_core::astro::Master>, String> {
    if paths.len() > 64 {
        return Err("Use at most 64 frames per calibration group".into());
    }
    let mut master: Option<sp_core::astro::Master> = None;
    for source in paths {
        if cancel.load(Ordering::Relaxed) {
            return Err("Astro calibration cancelled".into());
        }
        let prepared = cache
            .map(|cache| sp_catalog::source_cache::prepare(cache, source))
            .transpose()
            .map_err(|e| e.to_string())?;
        let path = prepared
            .as_ref()
            .map_or(source.as_path(), |p| p.decode_path.as_path());
        let decoded = sp_raw::decode(path).map_err(|e| e.to_string())?;
        if let Some(master) = &mut master {
            master.add(&decoded.raw, &decoded.meta, cancel)?;
        } else {
            master = Some(sp_core::astro::Master::from_raw(
                &decoded.raw,
                &decoded.meta,
                cancel,
            )?);
        }
    }
    Ok(master)
}

#[derive(Debug, serde::Deserialize)]
struct FocusRecipe {
    operation: MergeKind,
    sources: Vec<PathBuf>,
    reference: usize,
    #[serde(default)]
    focus_repairs: Vec<sp_core::multiframe::FocusRepair>,
    #[serde(default)]
    align: Option<bool>,
}
fn read_focus_recipe(path: &std::path::Path) -> Result<FocusRecipe, String> {
    if std::fs::metadata(path).map_err(|e| e.to_string())?.len() > 4 * 1024 * 1024 {
        return Err("Merge recipe exceeds 4 MB".into());
    }
    let recipe: FocusRecipe =
        serde_json::from_slice(&std::fs::read(path).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    if recipe.operation != MergeKind::Focus
        || !(2..=16).contains(&recipe.sources.len())
        || recipe.reference >= recipe.sources.len()
        || recipe.focus_repairs.len() > 128
    {
        return Err("Choose a valid ShopPhoto focus-stack recipe".into());
    }
    Ok(recipe)
}

pub(super) enum Event {
    Progress(f32, String),
    Done(Result<Outcome, String>),
}

pub(super) enum Outcome {
    Saved(PathBuf, Option<String>),
    Preview {
        merged: egui::ColorImage,
        sources: Vec<egui::ColorImage>,
        crop: [u32; 4],
        dimensions: [u32; 2],
    },
}
struct FocusPreview {
    merged: egui::TextureHandle,
    sources: Vec<egui::TextureHandle>,
    crop: [u32; 4],
    dimensions: [u32; 2],
}
impl std::fmt::Debug for FocusPreview {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FocusPreview")
            .field("crop", &self.crop)
            .finish()
    }
}
fn preview_image(
    image: &sp_core::source::RasterImage,
    registration: sp_core::multiframe::Registration,
    crop: [u32; 4],
) -> egui::ColorImage {
    let scale = (crop[2].max(crop[3]) as f32 / 1024.0).max(1.0);
    let width = (crop[2] as f32 / scale).round() as usize;
    let height = (crop[3] as f32 / scale).round() as usize;
    let mut pixels = Vec::with_capacity(width * height);
    for y in 0..height {
        for x in 0..width {
            let rx = crop[0] as f32 + (x as f32 + 0.5) * crop[2] as f32 / width as f32 - 0.5;
            let ry = crop[1] as f32 + (y as f32 + 0.5) * crop[3] as f32 / height as f32 - 0.5;
            let (sx, sy) = registration.point(rx, ry, image.width, image.height);
            let p = sp_core::multiframe::sample(image, sx, sy);
            let rgb = sp_core::color::mat3_apply(
                &[
                    [1.660491, -0.587641, -0.072850],
                    [-0.124550, 1.132_9, -0.008349],
                    [-0.018151, -0.100579, 1.118_73],
                ],
                [p[0], p[1], p[2]],
            );
            let rgb =
                rgb.map(|v| (sp_core::color::srgb_oetf(v.clamp(0.0, 1.0)) * 255.0).round() as u8);
            pixels.push(egui::Color32::from_rgb(rgb[0], rgb[1], rgb[2]));
        }
    }
    egui::ColorImage::new([width, height], pixels)
}

fn exposure(meta: &sp_core::ImageMeta) -> Result<f32, String> {
    let time = meta
        .shutter_s
        .filter(|v| v.is_finite() && *v > 0.0)
        .ok_or("HDR needs capture shutter times or explicit exposure EV values")?;
    let aperture = meta
        .aperture_f
        .filter(|v| v.is_finite() && *v > 0.0)
        .ok_or("HDR needs capture apertures or explicit exposure EV values")?;
    let iso = meta
        .iso
        .filter(|v| *v > 0)
        .ok_or("HDR needs capture ISO values or explicit exposure EV values")?;
    Ok((time * iso as f32 / (aperture * aperture)).log2())
}

fn parse_ev(value: &str, count: usize) -> Result<Option<Vec<f32>>, String> {
    if value.trim().is_empty() {
        return Ok(None);
    }
    let normalized = value.replace('−', "-");
    let values = normalized
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|v| !v.is_empty())
        .map(|v| {
            v.parse::<f32>()
                .map_err(|_| crate::i18n::format("Invalid exposure EV: {0}", &[v.to_string()]))
        })
        .collect::<Result<Vec<_>, _>>()?;
    if values.len() != count || values.iter().any(|v| !v.is_finite() || v.abs() > 30.0) {
        return Err(
            "Enter one finite exposure EV per source (−30 to +30), in the displayed order".into(),
        );
    }
    Ok(Some(values))
}

fn available_destination(source: &std::path::Path, kind: MergeKind) -> Result<PathBuf, String> {
    let parent = source.parent().ok_or("Source has no parent folder")?;
    let stem = source.file_stem().unwrap_or_default().to_string_lossy();
    for index in 0..10000 {
        let path = parent.join(if index == 0 {
            format!("{stem}-{}.tif", kind.suffix())
        } else {
            format!("{stem}-{}-{index}.tif", kind.suffix())
        });
        if !path.exists() && !path.with_extension("spmerge.json").exists() {
            return Ok(path);
        }
    }
    Err("Could not choose an unused merge output name".into())
}

impl ShopPhotoApp {
    pub(super) fn multiframe_upkeep(&mut self, ctx: &egui::Context) {
        if let Some(receiver) = &self.state.merge.recipe_picker {
            let result = match receiver.try_recv() {
                Ok(result) => Some(result),
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    Some(Err("Recipe picker stopped".into()))
                }
                Err(crossbeam_channel::TryRecvError::Empty) => None,
            };
            if let Some(result) = result {
                self.state.merge.recipe_picker = None;
                match result {
                    Ok(Some(recipe)) => {
                        self.state.merge.reset_focus();
                        self.state.merge.sources = recipe.sources;
                        self.state.merge.reference = recipe.reference;
                        self.state.merge.repairs = recipe.focus_repairs;
                        self.state.merge.align = recipe.align.unwrap_or(true);
                        self.state.merge.kind = MergeKind::Focus;
                        self.state.merge.status =
                            "Focus recipe loaded; rebuild preview to inspect or change repairs"
                                .into();
                    }
                    Ok(None) => {}
                    Err(error) => {
                        self.state.merge.status = error;
                    }
                }
            }
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        if let Some((group, receiver)) = &self.state.merge.calibration_picker {
            match receiver.try_recv() {
                Ok(paths) => {
                    if let Some(paths) = paths {
                        self.state.merge.astro.groups[*group] = paths;
                    }
                    self.state.merge.calibration_picker = None;
                }
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    self.state.merge.calibration_picker = None;
                }
                Err(crossbeam_channel::TryRecvError::Empty) => {}
            }
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        if std::mem::take(&mut self.state.merge.cancel_requested) {
            if let Some(cancel) = &self.merge_cancel {
                cancel.store(true, Ordering::Relaxed);
            }
        }
        if let Some(receiver) = &self.merge_job {
            let mut done = None;
            loop {
                match receiver.try_recv() {
                    Ok(Event::Progress(progress, status)) => {
                        self.state.merge.progress = progress;
                        self.state.merge.status = status;
                    }
                    Ok(Event::Done(result)) => {
                        done = Some(result);
                        break;
                    }
                    Err(crossbeam_channel::TryRecvError::Empty) => break,
                    Err(crossbeam_channel::TryRecvError::Disconnected) => {
                        done = Some(Err("Photo merge worker stopped unexpectedly".into()));
                        break;
                    }
                }
            }
            if let Some(result) = done {
                self.merge_job = None;
                self.merge_cancel = None;
                self.state.merge.busy = false;
                match result {
                    Ok(Outcome::Saved(path, warning)) => {
                        self.state.merge.status = {
                            let __l10n_args = (&(path.display()),);
                            crate::i18n::format("Created {0}", &[__l10n_args.0.to_string()])
                        };
                        self.state.push_toast(
                            warning.unwrap_or_else(|| self.state.merge.status.clone()),
                            false,
                        );
                        self.refresh_catalog(Vec::new(), None);
                        self.state.selected_photos.clear();
                        self.state.selected_photos.insert(path.clone());
                        self.start_decode(path, ctx);
                    }
                    Ok(Outcome::Preview {
                        merged,
                        sources,
                        crop,
                        dimensions,
                    }) => {
                        self.state.merge.preview = Some(FocusPreview {
                            merged: ctx.load_texture(
                                "focus-merged-preview",
                                merged,
                                egui::TextureOptions::LINEAR,
                            ),
                            sources: sources
                                .into_iter()
                                .enumerate()
                                .map(|(i, image)| {
                                    ctx.load_texture(
                                        format!("focus-source-{i}"),
                                        image,
                                        egui::TextureOptions::LINEAR,
                                    )
                                })
                                .collect(),
                            crop,
                            dimensions,
                        });
                        self.state.merge.status="Preview ready. Choose a source, then click a region to repair; rebuild the preview after changes.".into();
                    }
                    Err(error) => {
                        self.state.merge.status = error.clone();
                        self.state.push_toast(error, true);
                    }
                }
            }
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        let preview = std::mem::take(&mut self.state.merge.preview_requested);
        if std::mem::take(&mut self.state.merge.requested) || preview {
            self.start_photo_merge(ctx, preview);
        }
    }

    fn start_photo_merge(&mut self, ctx: &egui::Context, preview_only: bool) {
        if self.merge_job.is_some()
            || self.state.export_busy
            || self.state.enhancement_busy
            || self.state.ai_denoise_busy
        {
            self.state.merge.status = "Wait for the active image operation to finish".into();
            return;
        }
        let sources = self.state.merge.sources.clone();
        if !(2..=16).contains(&sources.len()) {
            self.state.merge.status = "Select 2–16 photographs in the Library".into();
            return;
        }
        if self.state.merge.kind == MergeKind::Astro
            && (sources.len() < 3 || !self.state.merge.astro.equipment_confirmed)
        {
            self.state.merge.status="Astro needs 3–16 RAW light frames and confirmation of matched calibration equipment/temperature".into();
            return;
        }
        if self.state.merge.kind == MergeKind::PixelShift
            && (sources.len() != 4 || !self.state.merge.pixel_shift_confirmed)
        {
            self.state.merge.status="Pixel shift requires exactly four original Bayer RAWs and confirmed sensor offsets for a static scene".into();
            return;
        }
        let ev = match parse_ev(&self.state.merge.exposure_values, sources.len()) {
            Ok(values) => values,
            Err(error) => {
                self.state.merge.status = error;
                return;
            }
        };
        let Some(gpu) = &self.gpu else {
            return;
        };
        let gpu_ctx = GpuContext::new(
            gpu.render_state.device.clone(),
            gpu.render_state.queue.clone(),
        );
        let pipelines = gpu.renderer.shared_pipelines();
        let options = MergeOptions {
            kind: self.state.merge.kind,
            reference: if self.state.merge.kind == MergeKind::PixelShift {
                0
            } else {
                self.state.merge.reference.min(sources.len() - 1)
            },
            exposure_ev: Vec::new(),
            motion_threshold: if self.state.merge.kind == MergeKind::Astro {
                self.state.merge.sigma
            } else {
                self.state.merge.motion_threshold
            },
            align: self.state.merge.align,
            focus_repairs: self.state.merge.repairs.clone(),
        };
        let catalog = self.catalog_path.clone();
        let panorama = self.state.merge.panorama.clone();
        let astro = self.state.merge.astro.clone();
        let pixel_shift = self.state.merge.pixel_shift.clone();
        let source_cache = source_cache_dir();
        let cancel = Arc::new(AtomicBool::new(false));
        let cancelled = cancel.clone();
        let (tx, rx) = crossbeam_channel::unbounded();
        let repaint = ctx.clone();
        std::thread::spawn(move || {
            let result = (|| -> Result<Outcome, String> {
                let mut renderer =
                    GpuRenderer::with_pipelines(gpu_ctx, pipelines).map_err(|e| e.to_string())?;
                let mut calibration = None;
                let mut masters = Vec::new();
                if options.kind == MergeKind::Astro {
                    for (index, group) in astro.groups.iter().enumerate() {
                        let _ = tx.send(Event::Progress(0.0, {
                            let __l10n_args = (&(["bias", "dark", "flat", "flat-dark"][index]),);
                            crate::i18n::format(
                                "Preparing {0} calibration frames…",
                                &[__l10n_args.0.to_string()],
                            )
                        }));
                        repaint.request_repaint();
                        masters.push(calibration_master(
                            group,
                            source_cache.as_deref(),
                            &cancelled,
                        )?);
                    }
                }
                let mut frames = Vec::with_capacity(sources.len());
                let mut sensor_frames = Vec::new();
                let mut sensor_identity = None;
                let mut raw_wb = None;
                let mut inherited_metadata = sp_catalog::DescriptiveMetadata::default();
                let mut total_pixels = 0u64;
                let mut order: Vec<_> = (0..sources.len()).collect();
                order.swap(0, options.reference);
                for (loaded, index) in order.into_iter().enumerate() {
                    if cancelled.load(Ordering::Relaxed) {
                        return Err("Merge cancelled".into());
                    }
                    let source = &sources[index];
                    let _ = tx.send(Event::Progress(
                        0.3 * loaded as f32 / sources.len() as f32,
                        {
                            let __l10n_args = (
                                &(loaded + 1),
                                &(sources.len()),
                                &(source.file_name().unwrap_or_default().to_string_lossy()),
                            );
                            crate::i18n::format(
                                "Preparing {0} of {1} · {2}",
                                &[
                                    __l10n_args.0.to_string(),
                                    __l10n_args.1.to_string(),
                                    __l10n_args.2.to_string(),
                                ],
                            )
                        },
                    ));
                    repaint.request_repaint();
                    let decode_path = source_cache
                        .as_ref()
                        .map(|cache| sp_catalog::source_cache::prepare(cache, source))
                        .transpose()
                        .map_err(|e| e.to_string())?
                        .map_or_else(|| source.clone(), |prepared| prepared.decode_path);
                    let mut decoded =
                        sp_raw::decode_source(&decode_path).map_err(|e| e.to_string())?;
                    if index == options.reference {
                        if let Ok(Some(sidecar)) = sp_catalog::load_sidecar(&decode_path) {
                            inherited_metadata = sidecar.metadata;
                        }
                    }
                    let (width, height) = decoded.source.dimensions();
                    total_pixels += u64::from(width) * u64::from(height);
                    if u64::from(width) * u64::from(height) > 150_000_000
                        || total_pixels > 320_000_000
                    {
                        return Err("Merge exceeds the memory bound: 150 MP per source or 320 MP across the stack. Use fewer frames or smaller sources.".into());
                    }
                    let identity = (
                        decoded.source.is_raw(),
                        decoded.meta.camera_make.clone(),
                        decoded.meta.camera_model.clone(),
                    );
                    if let Some(expected) = &sensor_identity {
                        if *expected != identity {
                            return Err(
                                "Merge photographs from the same camera and source type".into()
                            );
                        }
                    } else {
                        sensor_identity = Some(identity);
                    }
                    if options.kind == MergeKind::PixelShift {
                        match decoded.source {
                            ImageSource::Raw(raw) => sensor_frames.push((index, raw, decoded.meta)),
                            ImageSource::Raster(_) => {
                                return Err(
                                    "Pixel shift requires four original Bayer RAW frames".into()
                                )
                            }
                        }
                        continue;
                    }
                    if options.kind == MergeKind::Astro {
                        let raw = decoded
                            .source
                            .as_raw()
                            .ok_or("Astro calibration requires original Bayer RAW light frames")?;
                        if calibration.is_none() {
                            let mut groups = masters.drain(..);
                            calibration = Some(sp_core::astro::Calibration::new(
                                raw,
                                &decoded.meta,
                                groups.next().flatten(),
                                groups.next().flatten(),
                                groups.next().flatten(),
                                groups.next().flatten(),
                            )?);
                            raw_wb = Some(raw.wb_coeffs);
                        }
                        let frame = calibration.as_ref().unwrap().apply(
                            raw,
                            &decoded.meta,
                            raw_wb.unwrap(),
                            &cancelled,
                        )?;
                        frames.push((index, frame, decoded.meta));
                        continue;
                    }
                    let mut edit = EditState::default();
                    edit.highlight_recovery.enabled = false;
                    edit.nr.hot_pixels = false;
                    edit.lens.enable = false;
                    if let ImageSource::Raw(raw) = &mut decoded.source {
                        if let Some(wb) = raw_wb {
                            Arc::make_mut(raw).wb_coeffs = wb;
                        } else {
                            raw_wb = Some(raw.wb_coeffs);
                        }
                        renderer.set_profile(Some(sp_core::profile::CameraProfile::standard(
                            &decoded.meta.camera_model,
                            raw.cam_to_xyz,
                        )));
                    } else {
                        renderer.set_profile(None);
                    }
                    renderer.set_capture_metadata(None);
                    renderer.set_lens_profile(None, None);
                    if decoded.source.is_raw() {
                        renderer
                            .open_source_with_edit(&decoded.source, &edit)
                            .map_err(|e| e.to_string())?;
                    }
                    let frame = match decoded.source {
                        ImageSource::Raster(raster) => {
                            Arc::try_unwrap(raster).unwrap_or_else(|raster| (*raster).clone())
                        }
                        ImageSource::Raw(_) => renderer
                            .render_scene_linear(&edit, &cancelled, |fraction| {
                                let _ = tx.send(Event::Progress(
                                    0.3 * (loaded as f32 + fraction) / sources.len() as f32,
                                    "Preparing linear sources…".into(),
                                ));
                            })
                            .map_err(|e| e.to_string())?,
                    };
                    frames.push((index, frame, decoded.meta));
                }
                frames.sort_by_key(|frame| frame.0);
                let mut options = options;
                if options.kind == MergeKind::Hdr {
                    options.exposure_ev = match ev {
                        Some(values) => values,
                        None => frames
                            .iter()
                            .map(|frame| exposure(&frame.2))
                            .collect::<Result<Vec<_>, _>>()?,
                    };
                    let min = options
                        .exposure_ev
                        .iter()
                        .copied()
                        .fold(f32::INFINITY, f32::min);
                    let max = options
                        .exposure_ev
                        .iter()
                        .copied()
                        .fold(f32::NEG_INFINITY, f32::max);
                    if max - min < 0.25 {
                        return Err("HDR needs differently exposed frames (at least 0.25 EV apart); use Burst average for matching exposures".into());
                    }
                } else if !matches!(
                    options.kind,
                    MergeKind::Panorama | MergeKind::Astro | MergeKind::PixelShift
                ) {
                    let exposure_values = frames
                        .iter()
                        .map(|frame| exposure(&frame.2))
                        .collect::<Result<Vec<_>, _>>();
                    if let Ok(values) = exposure_values {
                        if values.iter().copied().fold(f32::NEG_INFINITY, f32::max)
                            - values.iter().copied().fold(f32::INFINITY, f32::min)
                            > 0.2
                        {
                            return Err("Burst/focus stacks require matching exposures; choose an exposure bracket only for HDR".into());
                        }
                    }
                }
                let frames: Vec<_> = frames.into_iter().map(|(_, frame, _)| frame).collect();
                let progress = |fraction| {
                    let _ = tx.send(Event::Progress(0.3 + 0.65 * fraction, {
                        let __l10n_args = (&(options.kind.label()),);
                        crate::i18n::format(
                            "{0} · registering and combining sources…",
                            &[__l10n_args.0.to_string()],
                        )
                    }));
                    repaint.request_repaint();
                };
                let (master, provenance) = if options.kind == MergeKind::PixelShift {
                    sensor_frames.sort_by_key(|frame| frame.0);
                    let refs: Vec<_> = sensor_frames
                        .iter()
                        .map(|(_, raw, meta)| (raw.as_ref(), meta))
                        .collect();
                    let merged =
                        sp_core::pixel_shift::reconstruct(&refs, &pixel_shift, &cancelled)?;
                    let provenance = serde_json::json!({"version":1,"algorithm":"shopphoto-four-phase-bayer-v1","operation":options.kind,"sources":sources,"reference":0,"descriptor":pixel_shift,"crop":merged.crop,"inconsistent_pixels":merged.inconsistent_pixels,"green_rms":merged.green_rms});
                    (merged.image, provenance)
                } else if options.kind == MergeKind::Panorama {
                    let merged =
                        sp_core::panorama::stitch(frames, &panorama, &cancelled, progress)?;
                    let provenance = serde_json::json!({"version":1,"algorithm":"shopphoto-panorama-v1","operation":options.kind,"sources":sources,"reference":options.reference,"projection":panorama,"placements":merged.placements,"inliers":merged.inliers,"gains":merged.gains,"crop":merged.crop});
                    (merged.image, provenance)
                } else {
                    let merged =
                        sp_core::multiframe::merge(&frames, &options, &cancelled, progress)?;
                    if preview_only && options.kind == MergeKind::Focus {
                        let previews = frames
                            .iter()
                            .zip(&merged.registrations)
                            .map(|(frame, registration)| {
                                preview_image(frame, *registration, merged.crop)
                            })
                            .collect();
                        let rendered = preview_image(
                            &merged.image,
                            Default::default(),
                            [0, 0, merged.image.width, merged.image.height],
                        );
                        return Ok(Outcome::Preview {
                            merged: rendered,
                            sources: previews,
                            crop: merged.crop,
                            dimensions: [
                                frames[options.reference].width,
                                frames[options.reference].height,
                            ],
                        });
                    }
                    let provenance = serde_json::json!({"version":1,"algorithm":"shopphoto-linear-merge-v1","focus_repairs":options.focus_repairs,"align":options.align,"operation":options.kind,"sources":sources,"reference":options.reference,"exposure_ev":options.exposure_ev,"motion_threshold":options.motion_threshold,"registrations":merged.registrations,"crop":merged.crop,"rejected_samples":merged.rejected_samples,"astro_calibration":if options.kind==MergeKind::Astro {Some(&astro)}else{None}});
                    (merged.image, provenance)
                };
                drop(renderer);
                let destination = available_destination(&sources[options.reference], options.kind)?;
                let recipe =
                    serde_json::to_string_pretty(&provenance).map_err(|e| e.to_string())?;
                let _ = tx.send(Event::Progress(
                    0.96,
                    "Writing editable 32-bit linear TIFF…".into(),
                ));
                repaint.request_repaint();
                sp_export::write_linear_tiff(&master, &destination, &recipe, &cancelled)?;
                // Embedded TIFF description is authoritative if optional
                // recipe/catalog registration cannot be written afterward.
                let mut warnings = Vec::new();
                let recipe_result = (|| -> std::io::Result<()> {
                    use std::io::Write;
                    let mut file = std::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(destination.with_extension("spmerge.json"))?;
                    file.write_all(recipe.as_bytes())?;
                    file.sync_all()
                })();
                if let Err(error) = recipe_result {
                    warnings.push(crate::i18n::format(
                        "Recipe sidecar: {0}",
                        &[error.to_string()],
                    ));
                }
                let mut sidecar = sp_catalog::Sidecar {
                    version: sp_core::edit::EDIT_SCHEMA_VERSION,
                    metadata: inherited_metadata,
                    ..Default::default()
                };
                sidecar.edit.hdr.enabled = options.kind == MergeKind::Hdr;
                if let Err(error) = sp_catalog::save_sidecar(&destination, &sidecar) {
                    warnings.push(crate::i18n::format(
                        "Edit sidecar: {0}",
                        &[error.to_string()],
                    ));
                }
                if let Some(catalog) = catalog {
                    let registered = sp_catalog::CatalogDb::open(&catalog).and_then(|db| {
                        let mut parents = sources.clone();
                        if options.kind == MergeKind::Astro {
                            for path in astro.groups.iter().flatten() {
                                if !parents.contains(path) {
                                    parents.push(path.clone());
                                }
                            }
                        }
                        db.link_derived_sources(&destination, &parents, options.kind.label())
                    });
                    if let Err(error) = registered {
                        warnings.push(crate::i18n::format("Catalog: {0}", &[error.to_string()]));
                    }
                }
                Ok(Outcome::Saved(
                    destination,
                    (!warnings.is_empty()).then(|| {
                        let __l10n_args = (&(warnings.join("; ")),);
                        crate::i18n::format("Merge saved; {0}", std::slice::from_ref(__l10n_args.0))
                    }),
                ))
            })();
            let _ = tx.send(Event::Done(result));
            repaint.request_repaint();
        });
        self.merge_job = Some(rx);
        self.merge_cancel = Some(cancel);
        self.state.merge.busy = true;
        self.state.merge.progress = 0.0;
        self.state.merge.status = "Preparing sources…".into();
    }
}

fn focus_repair_ui(ui: &mut egui::Ui, state: &mut State) {
    ui.separator();
    ui.label(crate::i18n::text(
        "Repair focus transitions with a chosen registered source.",
    ));
    if ui
        .button(crate::i18n::text("Build / refresh focus preview"))
        .clicked()
    {
        state.preview_requested = true;
    }
    egui::ComboBox::from_id_salt("focus-repair-source")
        .selected_text({
            let __l10n_args = (&(state.focus_source + 1),);
            crate::i18n::format("Repair from source {0}", &[__l10n_args.0.to_string()])
        })
        .show_ui(ui, |ui| {
            for (index, path) in state.sources.iter().enumerate() {
                ui.selectable_value(
                    &mut state.focus_source,
                    index,
                    format!(
                        "{} · {}",
                        index + 1,
                        path.file_name().unwrap_or_default().to_string_lossy()
                    ),
                );
            }
        });
    ui.checkbox(
        &mut state.preview_source,
        crate::i18n::text("Show chosen source instead of merged preview"),
    );
    if let Some(preview) = &state.preview {
        let texture = if state.preview_source {
            preview
                .sources
                .get(state.focus_source)
                .unwrap_or(&preview.merged)
        } else {
            &preview.merged
        };
        let size = texture.size_vec2();
        let scale = (ui.available_width() / size.x).min(350.0 / size.y).min(1.0);
        let response = ui.add(
            egui::Image::new(texture)
                .fit_to_exact_size(size * scale)
                .sense(egui::Sense::click()),
        );
        let rect = response.rect;
        if response.clicked() && state.repairs.len() < 128 {
            if let Some(point) = response.interact_pointer_pos() {
                let uv = (point - rect.min) / rect.size();
                let center = [
                    (preview.crop[0] as f32 + uv.x * preview.crop[2] as f32)
                        / preview.dimensions[0] as f32,
                    (preview.crop[1] as f32 + uv.y * preview.crop[3] as f32)
                        / preview.dimensions[1] as f32,
                ];
                state.repairs.push(sp_core::multiframe::FocusRepair {
                    source: state
                        .focus_source
                        .min(state.sources.len().saturating_sub(1)),
                    center,
                    radii: [0.05, 0.05],
                    feather: 0.4,
                });
                state.selected_repair = Some(state.repairs.len() - 1);
            }
        }
        for (index, repair) in state.repairs.iter().enumerate() {
            let point = |x: f32, y: f32| {
                rect.min
                    + egui::vec2(
                        (x * preview.dimensions[0] as f32 - preview.crop[0] as f32)
                            / preview.crop[2] as f32
                            * rect.width(),
                        (y * preview.dimensions[1] as f32 - preview.crop[1] as f32)
                            / preview.crop[3] as f32
                            * rect.height(),
                    )
            };
            let outline = (0..=48)
                .map(|step| {
                    let angle = step as f32 / 48.0 * std::f32::consts::TAU;
                    point(
                        repair.center[0] + repair.radii[0] * angle.cos(),
                        repair.center[1] + repair.radii[1] * angle.sin(),
                    )
                })
                .collect();
            let color = if state.selected_repair == Some(index) {
                egui::Color32::YELLOW
            } else {
                egui::Color32::WHITE
            };
            ui.painter()
                .with_clip_rect(rect)
                .add(egui::Shape::line(outline, egui::Stroke::new(1.5, color)));
            ui.painter().text(
                point(repair.center[0], repair.center[1]),
                egui::Align2::CENTER_CENTER,
                format!("{}", index + 1),
                egui::FontId::proportional(14.0),
                color,
            );
        }
        ui.small(crate::i18n::text("Click preview to add a radial repair. Source and merged views share registered coordinates. Preview is SDR and limited to 1024 pixels; final processing uses full resolution."));
    }
    ui.horizontal_wrapped(|ui| {
        for index in 0..state.repairs.len() {
            ui.selectable_value(&mut state.selected_repair, Some(index), {
                let __l10n_args = (&(index + 1),);
                crate::i18n::format("Region {0}", &[__l10n_args.0.to_string()])
            });
        }
    });
    let mut remove = None;
    if let Some(index) = state.selected_repair {
        if let Some(repair) = state.repairs.get_mut(index) {
            ui.horizontal(|ui| {
                ui.label({
                    let __l10n_args = (&(index + 1), &(repair.source + 1));
                    crate::i18n::format(
                        "Region {0} · source {1}",
                        &[__l10n_args.0.to_string(), __l10n_args.1.to_string()],
                    )
                });
                if ui.button(crate::i18n::text("Use chosen source")).clicked() {
                    repair.source = state
                        .focus_source
                        .min(state.sources.len().saturating_sub(1));
                }
                if ui.button(crate::i18n::text("Remove region")).clicked() {
                    remove = Some(index);
                }
            });
            ui.add(
                egui::Slider::new(&mut repair.center[0], 0.0..=1.0)
                    .text(crate::i18n::text("Center X")),
            );
            ui.add(
                egui::Slider::new(&mut repair.center[1], 0.0..=1.0)
                    .text(crate::i18n::text("Center Y")),
            );
            ui.add(
                egui::Slider::new(&mut repair.radii[0], 0.002..=0.5)
                    .text(crate::i18n::text("Horizontal radius")),
            );
            ui.add(
                egui::Slider::new(&mut repair.radii[1], 0.002..=0.5)
                    .text(crate::i18n::text("Vertical radius")),
            );
            ui.add(
                egui::Slider::new(&mut repair.feather, 0.0..=1.0)
                    .text(crate::i18n::text("Feather")),
            );
        }
    }
    if let Some(index) = remove {
        state.repairs.remove(index);
        state.selected_repair = None;
    }
    ui.small(crate::i18n::text("Refresh to inspect changed repairs before saving. Later regions blend over earlier regions. Save creates a new master with all repair settings in its recipe."));
}
impl State {
    pub(crate) fn reset_focus(&mut self) {
        self.repairs.clear();
        self.selected_repair = None;
        self.preview = None;
        self.focus_source = 0;
        self.astro.equipment_confirmed = false;
        self.pixel_shift_confirmed = false;
    }
}

pub(crate) fn window(ctx: &egui::Context, state: &mut State) {
    if !state.open {
        return;
    }
    let mut open = state.open;
    egui::Window::new(crate::i18n::text("Merge photographs")).open(&mut open).default_width(560.0).default_height(640.0).vscroll(true).show(ctx,|ui|{
        ui.label(crate::i18n::text("Create an editable 32-bit linear TIFF from the selected originals."));
        ui.add_enabled_ui(!state.busy,|ui|{
            if ui.add_enabled(state.recipe_picker.is_none(),egui::Button::new(crate::i18n::text("Load focus repair recipe…"))).clicked(){
                let(tx,rx)=crossbeam_channel::bounded(1);state.recipe_picker=Some(rx);
                std::thread::spawn(move||{let result=rfd::FileDialog::new().set_title(crate::i18n::text("Load ShopPhoto focus merge recipe")).add_filter(crate::i18n::text("Merge recipe"),&["json"]).pick_file().map(|path|read_focus_recipe(&path)).transpose();let _=tx.send(result);});
            }
            let old_reference=state.reference;let old_align=state.align;
            ui.horizontal_wrapped(|ui|{for kind in [MergeKind::Hdr,MergeKind::Burst,MergeKind::Focus,MergeKind::Panorama,MergeKind::Astro,MergeKind::PixelShift] {ui.radio_value(&mut state.kind,kind,crate::i18n::text(kind.label()));}});
            ui.label(crate::i18n::text(if state.kind==MergeKind::PixelShift {"First source anchors sensor offsets and white balance."}else{"Reference frame sets alignment, white balance and the preferred position of moving objects."}));
            egui::ScrollArea::vertical().max_height(180.0).show(ui,|ui|{for (i,path) in state.sources.iter().enumerate() {ui.add_enabled_ui(state.kind!=MergeKind::PixelShift || i==0,|ui|{ui.radio_value(&mut state.reference,i,format!("{} · {}",i+1,path.file_name().unwrap_or_default().to_string_lossy()));});}});
            if state.kind == MergeKind::PixelShift {
                state.reference=0;
                ui.label(crate::i18n::text("Expert four-shot color reconstruction: offsets are sensor photosites before orientation, relative to the first source."));
                for index in 0..4 { ui.horizontal(|ui| {
                    ui.label(format!("{} {}",crate::i18n::text("Source"),index+1));
                    ui.add_enabled_ui(index!=0,|ui| {
                        ui.label(crate::i18n::text("dx")); if ui.add(egui::DragValue::new(&mut state.pixel_shift.offsets[index][0]).range(-2..=2)).changed(){state.pixel_shift_confirmed=false;}
                        ui.label(crate::i18n::text("dy")); if ui.add(egui::DragValue::new(&mut state.pixel_shift.offsets[index][1]).range(-2..=2)).changed(){state.pixel_shift_confirmed=false;}
                    });
                }); }
                ui.add(egui::Slider::new(&mut state.pixel_shift.green_tolerance,0.001..=0.1).text(crate::i18n::text("Duplicate green tolerance")));
                ui.checkbox(&mut state.pixel_shift_confirmed,crate::i18n::text("Four known Bayer phases, same camera/settings, static subject and steady light"));
                ui.small(crate::i18n::text("No camera presets or shift inference. Verify the actual sequence offsets. This mode improves color sampling, not pixel dimensions; moving scenes and unsupported multi-shot formats are rejected."));
            }
            if !matches!(state.kind,MergeKind::Panorama|MergeKind::PixelShift) {ui.checkbox(&mut state.align,crate::i18n::text("Align small camera shifts, rotations and focus breathing"));}
            else if state.kind==MergeKind::Panorama {
                use sp_core::panorama::{Projection,Boundary};
                ui.label(crate::i18n::text("Use adjacent overlapping frames in the displayed order, from a level camera rotated around its optical center."));
                ui.horizontal(|ui| {for (value,label) in [(Projection::Planar,"Planar"),(Projection::Cylindrical,"Cylindrical"),(Projection::Spherical,"Spherical")] {ui.radio_value(&mut state.panorama.projection,value,crate::i18n::text(label));}});
                if state.panorama.projection!=Projection::Planar {ui.add(egui::Slider::new(&mut state.panorama.horizontal_fov_degrees,15.0..=150.0).text(crate::i18n::text("Source horizontal field of view (degrees)")));}
                let mut automatic=state.panorama.manual_overlap.is_none();
                if ui.checkbox(&mut automatic,crate::i18n::text("Match overlap automatically")).changed() {state.panorama.manual_overlap=if automatic {None}else{Some(0.4)};}
                if let Some(overlap)=&mut state.panorama.manual_overlap {ui.add(egui::Slider::new(overlap,0.1..=0.9).text(crate::i18n::text("Manual overlap · left to right")));}
                ui.checkbox(&mut state.panorama.match_exposure,crate::i18n::text("Match exposure across overlaps"));
                ui.horizontal(|ui| {ui.radio_value(&mut state.panorama.boundary,Boundary::Crop,crate::i18n::text("Crop empty edges"));ui.radio_value(&mut state.panorama.boundary,Boundary::BlackCanvas,crate::i18n::text("Keep canvas with black edges"));});
                ui.small(crate::i18n::text("Feathered overlaps. Strong parallax, roll, moving subjects or incompatible lens fields of view require a specialist stitcher."));
            }
            if state.reference!=old_reference || state.align!=old_align {state.reset_focus();}
            if state.kind==MergeKind::Focus {focus_repair_ui(ui,state);}
            if state.kind==MergeKind::Astro {
                ui.label(crate::i18n::text("3–16 tracked Bayer RAW lights at the same exposure and ISO. Calibration happens on signed sensor samples before demosaic."));
                for (index,label) in ["Bias","Dark","Flat","Flat-dark"].iter().enumerate() {
                    ui.horizontal(|ui| {
                        ui.label({ let __l10n_args = (&(state.astro.groups[index].len()),); crate::i18n::format("{0}: {1} frames", &[label.to_string(), __l10n_args.0.to_string()]) });
                        if ui.add_enabled(state.calibration_picker.is_none(),egui::Button::new(crate::i18n::text("Choose RAWs…"))).clicked() {
                            let (tx,rx)=crossbeam_channel::bounded(1);state.calibration_picker=Some((index,rx));
                            let label=label.to_string();
                            std::thread::spawn(move||{let _=tx.send(rfd::FileDialog::new().set_title(crate::i18n::format("Choose {0} calibration frames", std::slice::from_ref(&label))).add_filter(crate::i18n::text("Camera RAW"),sp_core::raw::SUPPORTED_RAW_EXTENSIONS).pick_files());});
                        }
                        if !state.astro.groups[index].is_empty() && ui.small_button(crate::i18n::text("Clear")).clicked(){state.astro.groups[index].clear();}
                    });
                }
                ui.checkbox(&mut state.astro.equipment_confirmed,crate::i18n::text("Same physical camera and sensor temperature; flats match the optical setup"));
                ui.small(crate::i18n::text("Camera serial and sensor temperature are not recorded by this decoder. Darks must match light exposure; flat-darks must match flats. Flats require bias or flat-darks. Long flats require flat-darks."));
                ui.add(egui::Slider::new(&mut state.sigma,1.5..=6.0).text(crate::i18n::text("Outlier rejection · sigma")));
                ui.small(crate::i18n::text("Small tracked shifts/rotations only; no untracked wide-field sky distortion correction or dark scaling. Empty calibration groups perform an uncalibrated stack."));
            }
            if state.kind==MergeKind::Hdr {
                ui.label(crate::i18n::text("Exposure EV in displayed order, e.g. −2, 0, 2 (empty uses shutter/ISO/aperture EXIF)"));
                ui.text_edit_singleline(&mut state.exposure_values);
                ui.small(crate::i18n::text("Use RAW or linear TIFF brackets for radiometric HDR. Camera-rendered JPEG tone curves can reduce accuracy."));
            }
            if matches!(state.kind,MergeKind::Hdr|MergeKind::Burst) {ui.add(egui::Slider::new(&mut state.motion_threshold,0.03..=0.5).text(crate::i18n::text("Motion rejection tolerance")));}
            else if state.kind==MergeKind::Focus {ui.small(crate::i18n::text("Use a static scene and fixed exposure. Contrast-based blending may need retouching around overlapping fine edges."));}
            ui.small(crate::i18n::text("Common overlap is cropped. Output and provenance are saved beside the reference; originals and their edits stay available."));
            if ui.add_enabled((2..=16).contains(&state.sources.len()) && (state.kind!=MergeKind::Astro || (state.sources.len()>=3 && state.astro.equipment_confirmed)) && (state.kind!=MergeKind::PixelShift || (state.sources.len()==4 && state.pixel_shift_confirmed)),egui::Button::new(crate::i18n::text("Create merged TIFF"))).clicked() {state.requested=true;}
        });
        if state.busy {ui.add(egui::ProgressBar::new(state.progress).show_percentage());if ui.button(crate::i18n::text("Cancel merge")).clicked(){state.cancel_requested=true;}}
        if !state.status.is_empty(){ui.label(crate::i18n::text(&state.status));}
    });
    state.open = open;
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hdr_sources_merge_to_editable_float_master_with_all_catalog_parents() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let sources = vec![root.join("long.tif"), root.join("short.tif")];
        let cancelled = AtomicBool::new(false);
        for (index, path) in sources.iter().enumerate() {
            let pixels = (0..16 * 16)
                .map(|i| {
                    let light: f32 = if i % 16 > 7 { 2.0 } else { 0.2 };
                    let signal = (light * if index == 0 { 1.0 } else { 0.25 }).min(1.0);
                    [signal, signal, signal, 1.0]
                })
                .collect();
            let image = sp_core::source::RasterImage {
                width: 16,
                height: 16,
                linear_rec2020: pixels,
                bits_per_channel: 32,
                embedded_icc: None,
            };
            sp_export::write_linear_tiff(&image, path, "original", &cancelled).unwrap();
        }
        let original_bytes: Vec<_> = sources.iter().map(|p| std::fs::read(p).unwrap()).collect();
        let frames: Vec<_> = sources
            .iter()
            .map(|path| match sp_raw::decode_source(path).unwrap().source {
                ImageSource::Raster(raster) => Arc::try_unwrap(raster).unwrap(),
                _ => panic!("RGB source"),
            })
            .collect();
        let options = MergeOptions {
            kind: MergeKind::Hdr,
            reference: 0,
            exposure_ev: vec![0.0, -2.0],
            motion_threshold: 0.15,
            align: false,
            focus_repairs: Vec::new(),
        };
        let merged = sp_core::multiframe::merge(&frames, &options, &cancelled, |_| {}).unwrap();
        let output = available_destination(&sources[0], MergeKind::Hdr).unwrap();
        sp_export::write_linear_tiff(&merged.image, &output, "HDR recipe", &cancelled).unwrap();
        let ImageSource::Raster(reopened) = sp_raw::decode_source(&output).unwrap().source else {
            panic!("RGB master")
        };
        assert_eq!(reopened.bits_per_channel, 32);
        assert!((reopened.linear_rec2020[10][0] - 2.0).abs() < 0.002);
        let catalog = sp_catalog::CatalogDb::open_memory().unwrap();
        catalog
            .link_derived_sources(&output, &sources, "HDR")
            .unwrap();
        let photos = catalog
            .photos(
                &sp_catalog::LibraryFilter::default(),
                sp_catalog::LibrarySort::Name,
            )
            .unwrap();
        let master = photos.iter().find(|photo| photo.path == output).unwrap();
        assert_eq!(master.derived_sources, sources);
        for (path, bytes) in sources.iter().zip(original_bytes) {
            assert_eq!(std::fs::read(path).unwrap(), bytes);
        }
        assert_ne!(
            available_destination(&sources[0], MergeKind::Hdr).unwrap(),
            output
        );
    }

    #[test]
    fn exposure_values_are_explicit_and_preserve_stop_ratios() {
        assert_eq!(parse_ev("-2, 0, 2", 3).unwrap(), Some(vec![-2.0, 0.0, 2.0]));
        assert!(parse_ev("1,2", 3).is_err());
        assert!(parse_ev("NaN,1", 2).is_err());
        let mut meta = sp_core::ImageMeta {
            shutter_s: Some(0.01),
            iso: Some(100),
            aperture_f: Some(4.0),
            ..Default::default()
        };
        let ev = exposure(&meta).unwrap();
        meta.shutter_s = Some(0.04);
        assert!((exposure(&meta).unwrap() - ev - 2.0).abs() < 1e-5);
    }
}
