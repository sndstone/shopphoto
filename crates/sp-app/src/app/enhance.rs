//! Isolated, cancellable image enhancement. The optional Python runtime is
//! independent of sp-ai and never opens or modifies denoise model files.
use super::*;
use std::{
    path::Path,
    sync::{atomic::AtomicBool, Arc},
};

pub(super) enum Event {
    Mask {
        source: PathBuf,
        source_generation: u64,
        before: Box<EditState>,
        masks: Vec<(String, sp_core::mask::AlphaMask)>,
        replace_mask: Option<usize>,
    },
    Candidates(Box<crate::state::MaskCandidates>),
    Model(PathBuf),
    Failed(String),
    Cancelled,
}

use super::enhance_worker::{python, run as run_worker};

fn read_mask(path: &Path, source: String) -> Result<sp_core::mask::AlphaMask, String> {
    let dimensions = image::ImageReader::open(path)
        .map_err(|e| e.to_string())?
        .into_dimensions()
        .map_err(|e| e.to_string())?;
    if u64::from(dimensions.0) * u64::from(dimensions.1) > 150_000_000 {
        return Err("Mask exceeds 150 MP".into());
    }
    let mut reader = image::ImageReader::open(path).map_err(|e| e.to_string())?;
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(600_000_000);
    reader.limits(limits);
    let decoded = reader.decode().map_err(|e| e.to_string())?;
    // An explicit transparency channel is the matte; grayscale PNGs store
    // coverage in luminance. Preserve all 16 bits in either representation.
    let image = if decoded.color().has_alpha() {
        let rgba = decoded.to_rgba16();
        image::ImageBuffer::from_fn(rgba.width(), rgba.height(), |x, y| {
            image::Luma([rgba.get_pixel(x, y)[3]])
        })
    } else {
        decoded.to_luma16()
    };
    let (width, height) = image.dimensions();
    let mask = sp_core::mask::AlphaMask {
        width,
        height,
        alpha: image.into_raw().into(),
        source,
        inverted: false,
        repairs: vec![],
    };
    if !mask.valid() {
        return Err("Mask is empty or exceeds 150 MP".into());
    }
    Ok(mask)
}

fn read_generated_mask(
    path: &Path,
    source: String,
    expected: (u32, u32),
) -> Result<sp_core::mask::AlphaMask, String> {
    let mask = read_mask(path, source)?;
    if (mask.width, mask.height) != expected {
        return Err("AI mask dimensions do not match the original photo".into());
    }
    if !mask.alpha.iter().any(|&value| value > 0) {
        return Err("AI returned an empty mask. Try an include point or another selection; no empty layer was created".into());
    }
    Ok(mask)
}

enum GeneratedSelection {
    Masks(Vec<(String, sp_core::mask::AlphaMask)>),
    Candidates {
        backend: String,
        items: Vec<crate::state::MaskCandidate>,
    },
}

fn parse_mask_request(request: &str) -> Result<(String, Option<Vec<u32>>), String> {
    let Some(selected) = request.strip_prefix("selected:") else {
        return Ok((request.into(), None));
    };
    let (kind, ids) = selected
        .split_once(':')
        .ok_or("Invalid element selection request")?;
    if !matches!(
        kind,
        "semantic_people" | "semantic_cars" | "semantic_motorcycles"
    ) {
        return Err("Invalid element category".into());
    }
    let ids: Vec<u32> = ids
        .split(',')
        .map(str::parse)
        .collect::<Result<_, _>>()
        .map_err(|_| "Invalid element IDs")?;
    if ids.is_empty()
        || ids.len() > 16
        || ids
            .iter()
            .enumerate()
            .any(|(index, id)| ids[..index].contains(id))
    {
        return Err("Choose 1–16 distinct elements".into());
    }
    Ok((kind.into(), Some(ids)))
}

#[derive(serde::Deserialize)]
struct InstanceManifest {
    version: u32,
    backend: String,
    width: u32,
    height: u32,
    candidates: Vec<InstanceEntry>,
}
#[derive(serde::Deserialize)]
struct InstanceEntry {
    id: u32,
    confidence: f32,
    bounds: [f32; 4],
    preview: Option<String>,
    mask: Option<String>,
}
fn instance_file(root: &Path, name: Option<&str>) -> Result<PathBuf, String> {
    let name = name.ok_or("Element result is missing its pixels")?;
    let path = Path::new(name);
    if path.components().count() != 1
        || !matches!(
            path.components().next(),
            Some(std::path::Component::Normal(_))
        )
    {
        return Err("Invalid element result path".into());
    }
    Ok(root.join(path))
}
fn read_instance_selection(
    path: &Path,
    expected: (u32, u32),
    selected: Option<&[u32]>,
    selected_backend: Option<&str>,
    layer_name: &str,
) -> Result<GeneratedSelection, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    if bytes.len() > 64 * 1024 {
        return Err("Element result is too large".into());
    }
    let manifest: InstanceManifest = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    if manifest.backend.is_empty()
        || manifest.backend.len() > 256
        || selected_backend.is_some_and(|expected| expected != manifest.backend)
    {
        return Err("Selection model changed; detect elements again before creating layers".into());
    }
    if manifest.version != 1
        || (manifest.width, manifest.height) != expected
        || manifest.candidates.is_empty()
        || manifest.candidates.len() > 16
    {
        return Err("Invalid or empty element detection result".into());
    }
    let ids: Vec<_> = manifest.candidates.iter().map(|item| item.id).collect();
    if ids
        .iter()
        .enumerate()
        .any(|(index, id)| ids[..index].contains(id))
        || selected.is_some_and(|selected| {
            selected.len() != ids.len() || !selected.iter().all(|id| ids.contains(id))
        })
    {
        return Err("Element results do not match selected IDs".into());
    }
    let root = path.parent().ok_or("Missing result directory")?;
    if selected.is_some()
        && u64::from(expected.0) * u64::from(expected.1) * 2 * ids.len() as u64 > 600_000_000
    {
        return Err(
            "Selected masks exceed the 600 MB selection budget; create fewer layers at a time"
                .into(),
        );
    }
    let mut masks = Vec::new();
    let mut candidates = Vec::new();
    for item in manifest.candidates {
        if !item.confidence.is_finite()
            || !(0.0..=1.0).contains(&item.confidence)
            || !item
                .bounds
                .iter()
                .all(|v| v.is_finite() && (0.0..=1.0).contains(v))
            || item.bounds[2] <= 0.0
            || item.bounds[3] <= 0.0
            || item.bounds[0] + item.bounds[2] > 1.00001
            || item.bounds[1] + item.bounds[3] > 1.00001
        {
            return Err("Invalid element confidence or bounds".into());
        }
        if selected.is_some() {
            let mask = read_generated_mask(
                &instance_file(root, item.mask.as_deref())?,
                format!("AI instance · {layer_name} · {}", item.id),
                expected,
            )?;
            masks.push((format!("{layer_name} {}", masks.len() + 1), mask));
        } else {
            let path = instance_file(root, item.preview.as_deref())?;
            let reader = image::ImageReader::open(&path).map_err(|e| e.to_string())?;
            let dimensions = reader.into_dimensions().map_err(|e| e.to_string())?;
            if dimensions.0 == 0 || dimensions.1 == 0 || dimensions.0 > 160 || dimensions.1 > 160 {
                return Err("Invalid element preview dimensions".into());
            }
            let image = image::open(path).map_err(|e| e.to_string())?.to_rgba8();
            let preview = egui::ColorImage::from_rgba_unmultiplied(
                [dimensions.0 as usize, dimensions.1 as usize],
                image.as_raw(),
            );
            candidates.push(crate::state::MaskCandidate {
                id: item.id,
                confidence: item.confidence,
                bounds: item.bounds,
                preview,
                texture: None,
                overlay: None,
                selected: false,
            });
        }
    }
    Ok(if selected.is_some() {
        GeneratedSelection::Masks(masks)
    } else {
        GeneratedSelection::Candidates {
            backend: manifest.backend,
            items: candidates,
        }
    })
}

fn edit_with_masks(
    before: &EditState,
    masks: Vec<(String, sp_core::mask::AlphaMask)>,
    replace_mask: Option<usize>,
) -> Result<(EditState, usize), String> {
    let mut edit = before.clone();
    if let Some(index) = replace_mask {
        if masks.len() != 1 {
            return Err("Refinement must return exactly one mask".into());
        }
        let layer = edit
            .local_adjustments
            .get_mut(index)
            .ok_or("Selected layer no longer exists")?;
        if !matches!(layer.mask, sp_core::edit::LocalMask::Alpha(_)) {
            return Err("Selected layer is not an alpha mask".into());
        }
        layer.mask = sp_core::edit::LocalMask::Alpha(masks.into_iter().next().unwrap().1);
        return Ok((edit, index));
    }
    if masks.is_empty() {
        return Err("AI returned no mask layers".into());
    }
    for (name, mask) in masks {
        edit.local_adjustments.push(sp_core::edit::LocalAdjustment {
            name,
            enabled: true,
            mask: sp_core::edit::LocalMask::Alpha(mask),
            tone: Default::default(),
            color: Default::default(),
            ..sp_core::edit::LocalAdjustment::default()
        });
    }
    let active = edit.local_adjustments.len() - 1;
    Ok((edit, active))
}

impl ShopPhotoApp {
    pub(super) fn pick_upscale_model(&mut self, ctx: &egui::Context) {
        if self.enhancement_job.is_some() {
            return;
        }
        let (tx, rx) = crossbeam_channel::bounded(1);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let event = rfd::FileDialog::new()
                .add_filter(
                    crate::i18n::text("RGB super-resolution model"),
                    &["pth", "safetensors"],
                )
                .pick_file()
                .map_or(Event::Cancelled, Event::Model);
            let _ = tx.send(event);
            ctx.request_repaint();
        });
        self.enhancement_job = Some(rx);
    }

    pub(super) fn import_mask(&mut self, ctx: &egui::Context) {
        if self.enhancement_job.is_some() {
            return;
        }
        let Some(source) = self.state.open_image.clone() else {
            return;
        };
        let before = Box::new(self.state.edit.clone());
        let source_generation = self.gpu_open_generation;
        let dimensions = self.gpu.as_ref().and_then(|gpu| gpu.renderer.active_size());
        let (tx, rx) = crossbeam_channel::bounded(1);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let event = match rfd::FileDialog::new()
                .add_filter(
                    crate::i18n::text("PNG mask (8/16-bit grayscale or alpha)"),
                    &["png"],
                )
                .pick_file()
            {
                Some(path) => match read_mask(&path, "Imported alpha matte".into()) {
                    Ok(mask) if dimensions == Some((mask.width, mask.height)) => Event::Mask {
                        source,
                        source_generation,
                        before,
                        masks: vec![("Imported mask".into(), mask)],
                        replace_mask: None,
                    },
                    Ok(_) => Event::Failed(crate::i18n::format(
                        "Import an uncropped mask matching the developed image dimensions: {0}",
                        &[format!("{:?}", dimensions)],
                    )),
                    Err(e) => Event::Failed(e),
                },
                None => Event::Cancelled,
            };
            let _ = tx.send(event);
            ctx.request_repaint();
        });
        self.enhancement_job = Some(rx);
    }

    pub(super) fn start_mask(&mut self, kind: String, ctx: &egui::Context) {
        if self.enhancement_job.is_some()
            || self.state.decode_in_flight
            || self.state.export_busy
            || self.state.auto_finish_busy
        {
            return;
        }
        let replace_mask = if kind == "refine_object" {
            let Some(index) = self.state.retouch_active_local.filter(|&index| {
                self.state
                    .edit
                    .local_adjustments
                    .get(index)
                    .is_some_and(|layer| matches!(layer.mask, sp_core::edit::LocalMask::Alpha(_)))
            }) else {
                self.state
                    .push_toast("Select an alpha mask layer before refining it", true);
                return;
            };
            Some(index)
        } else {
            None
        };
        let request_kind = if replace_mask.is_some() {
            "object"
        } else {
            &kind
        };
        let (kind, selected_ids) = match parse_mask_request(request_kind) {
            Ok(request) => request,
            Err(error) => {
                self.state.push_toast(error, true);
                return;
            }
        };
        if selected_ids.is_some()
            && !self
                .state
                .mask_candidates
                .as_ref()
                .is_some_and(|candidates| {
                    candidates.source == self.state.open_image.clone().unwrap_or_default()
                        && candidates.generation == self.gpu_open_generation
                        && candidates.before == self.state.edit
                        && candidates.kind == kind
                })
        {
            self.state
                .push_toast("Image or edit changed; detect elements again", true);
            return;
        }
        let selected_backend = selected_ids.as_ref().and_then(|_| {
            self.state
                .mask_candidates
                .as_ref()
                .map(|candidates| candidates.backend.clone())
        });
        let instances = matches!(
            kind.as_str(),
            "semantic_people" | "semantic_cars" | "semantic_motorcycles"
        );
        if !super::native_enhance::supports_mask(&kind) && !python().is_file() {
            self.state.push_toast("Install the separate enhancement runtime using scripts/setup-enhance.py (Python 3.11+)", true);
            return;
        }
        let (Some(current), Some(gpu), Some(source)) =
            (&self.current, &self.gpu, self.state.open_image.clone())
        else {
            return;
        };
        if self
            .enhancement_worker
            .as_ref()
            .is_some_and(|worker| worker.is_stopped())
        {
            self.enhancement_worker.take();
        }
        if self.enhancement_worker.is_none() {
            match super::enhance_worker::InteractiveWorker::new() {
                Ok(worker) => self.enhancement_worker = Some(worker),
                Err(error) => {
                    self.state.push_toast(error, true);
                    return;
                }
            }
        }
        let ai_applied = self.ai_applied_mode == Some(self.state.edit.nr.camera_denoise_mode)
            && self.ai_result_raw.is_some();
        let image_source = if ai_applied {
            ImageSource::Raw(self.ai_result_raw.as_ref().unwrap().clone())
        } else {
            current.source.clone()
        };
        let meta = current.meta.clone();
        let mut edit = self.state.edit.clone();
        let before = Box::new(edit.clone());
        edit.geometry = Default::default();
        edit.resolution.mode = sp_core::edit::ResolutionMode::Full;
        edit.local_adjustments.clear();
        edit.sample_points.clear();
        let profile = self.profile_for(&edit);
        let look = self.custom_look_for(&edit);
        let lens = resolve_lens(&self.lenses, &edit.lens, &meta).cloned();
        let gpu_ctx = GpuContext::new(
            gpu.render_state.device.clone(),
            gpu.render_state.queue.clone(),
        );
        let pipes = gpu.renderer.shared_pipelines();
        let point = self.state.mask_point;
        let prompts = self.state.mask_prompts.clone();
        let text_prompt = self.state.mask_text_prompt.trim().to_owned();
        let layer_name = crate::state::MaskSubject::ALL
            .into_iter()
            .find(|subject| subject.request_kind() == kind)
            .map(|subject| crate::i18n::text(subject.label()).to_owned())
            .unwrap_or_else(|| {
                if kind == "text" {
                    text_prompt.clone()
                } else if kind == "depth" {
                    "Depth selection".into()
                } else {
                    "Object selection".into()
                }
            });
        if matches!(kind.as_str(), "object" | "sky")
            && !prompts.is_empty()
            && !prompts.iter().any(|(_, include)| *include)
        {
            self.state.push_toast(
                "Add at least one foreground point before building a selection",
                true,
            );
            return;
        }
        let (points, labels): (Vec<_>, Vec<_>) = if prompts.is_empty() {
            (vec![point], vec![1])
        } else {
            prompts
                .into_iter()
                .map(|(point, include)| (point, u8::from(include)))
                .unzip()
        };
        let near = self.state.mask_depth_min.min(self.state.mask_depth_max);
        let far = self.state.mask_depth_min.max(self.state.mask_depth_max);
        let source_generation = self.gpu_open_generation;
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let (tx, rx) = crossbeam_channel::bounded(1);
        let ctx = ctx.clone();
        let Some(worker) = &self.enhancement_worker else {
            return;
        };
        let task = worker.submit(cancel.clone(), move || {
            let result = (|| -> Result<GeneratedSelection, String> {
                if worker_cancel.load(std::sync::atomic::Ordering::Relaxed) {
                    return Err("Enhancement cancelled".into());
                }
                let dir = tempfile::tempdir().map_err(|e| e.to_string())?;
                let key = super::enhance_worker::CanonicalKey::new(
                    source_generation, &image_source, edit.clone(), &meta,
                    profile.clone(), look.clone(), lens.clone().map(|lens| (lens, meta.aperture_f)), ai_applied,
                )?;
                let input = super::enhance_worker::canonical_input(key, &worker_cancel, || {
                    let mut renderer = GpuRenderer::with_pipelines(gpu_ctx, pipes).map_err(|e| e.to_string())?;
                    renderer.set_canonical_mask_input(true);
                    renderer.set_capture_metadata(Some(&meta));
                    renderer.set_lens_profile(lens.as_ref(), meta.aperture_f);
                    renderer.open_source_with_edit(&image_source, &edit).map_err(|e| e.to_string())?;
                    renderer.set_external_raw_denoise(ai_applied);
                    renderer.set_profile(profile);
                    renderer.set_custom_look(look);
                    renderer.render_export(&edit, false, false, sp_export::ExportColorSpace::Srgb,
                        &worker_cancel, |_| {}).map_err(|e| e.to_string())
                })?;
                // Only the mapped input file survives between requests; decoded
                // pixels and temporary GPU resources are released before inference.
                drop(image_source);
                let output = dir.path().join(if instances { "instances.json" } else { "mask.png" });
                let (width, height) = (input.width, input.height);
                run_worker(
                    dir.path(),
                    serde_json::json!({"operation":if instances { "mask_instances" } else { "mask" }, "selected_ids":selected_ids, "selected_backend":selected_backend, "kind":kind, "input":input.path, "input_format":"rgba8", "width":width, "height":height, "output":output, "points":points, "labels":labels, "prompt":text_prompt, "near":near, "far":far}),
                    &worker_cancel,
                )?;
                if instances {
                    read_instance_selection(&output, (width, height), selected_ids.as_deref(), selected_backend.as_deref(), &layer_name)
                } else {
                read_generated_mask(
                    &output,
                    if let Some(source) = super::native_enhance::mask_source(&kind) {
                        source
                    } else if kind == "depth" {
                        "Depth Anything V2 Small · refined depth".into()
                    } else if kind == "text" {
                        format!("CLIPSeg + ViTMatte · {}", text_prompt)
                    } else if kind.starts_with("semantic_") {
                        format!("MaskFormer + ViTMatte · {kind}")
                    } else {
                        format!("SlimSAM + ViTMatte · {kind}")
                    },
                    (width, height),
                ).map(|mask| GeneratedSelection::Masks(vec![(layer_name.clone(), mask)]))
                }
            })();
            if result.as_ref().is_err_and(|error| !super::enhance_worker::is_no_match(error)) { super::enhance_worker::clear_input_cache(); }
            let event = match result {
                Ok(GeneratedSelection::Masks(masks)) => Event::Mask {
                    source, source_generation, before, masks, replace_mask,
                },
                Ok(GeneratedSelection::Candidates {backend,items}) => Event::Candidates(Box::new(crate::state::MaskCandidates {
                    source, generation: source_generation, before: *before, kind, backend, items,
                })),
                Err(e) => Event::Failed(e),
            };
            let _ = tx.send(event);
            ctx.request_repaint();
        });
        if let Err(error) = task {
            self.enhancement_worker.take();
            self.state.push_toast(error, true);
            return;
        }
        self.enhancement_job = Some(rx);
        self.enhancement_cancel = Some(cancel);
        self.state.enhancement_busy = true;
        self.state.enhancement_status = "Generating full-resolution mask…".into();
    }

    pub(super) fn poll_enhancements(&mut self, ctx: &egui::Context) {
        let event = self
            .enhancement_job
            .as_ref()
            .and_then(|rx| match rx.try_recv() {
                Ok(event) => Some(event),
                Err(crossbeam_channel::TryRecvError::Disconnected) => Some(Event::Failed(
                    "Enhancement worker stopped unexpectedly; retry or select a smaller image"
                        .into(),
                )),
                Err(crossbeam_channel::TryRecvError::Empty) => None,
            });
        let Some(event) = event else {
            return;
        };
        self.enhancement_job = None;
        self.enhancement_cancel = None;
        self.state.enhancement_busy = false;
        match event {
            Event::Model(path) => self.state.export_settings.upscale_model = Some(path),
            Event::Mask {
                source,
                source_generation,
                before,
                masks,
                replace_mask,
            } => {
                if self.state.open_image.as_ref() != Some(&source)
                    || !self.jobs.preview_is_current(source_generation)
                    || self.state.edit != *before
                {
                    self.state.push_toast(
                        "Mask discarded because the image or edit changed; try again",
                        false,
                    );
                    return;
                }
                let (edit, active) = match edit_with_masks(&self.state.edit, masks, replace_mask) {
                    Ok(result) => result,
                    Err(error) => {
                        self.state.push_toast(error, true);
                        return;
                    }
                };
                self.state.mask_candidates = None;
                self.state.retouch_active_local = Some(active);
                if replace_mask.is_some() {
                    self.state.mask_prompts.clear();
                }
                self.state.mask_show_overlay = true;
                self.state.apply_edit_step(edit);
                self.state.panels.local_retouch = true;
                self.state.inspector_query.clear();
                self.state.inspector_jump = None;
                self.state.inspector_focus_card = Some("Local & retouch".into());
                self.state.enhancement_status = "Mask ready · adjust it in Local & retouch".into();
                self.state
                    .push_toast(self.state.enhancement_status.clone(), false);
            }
            Event::Candidates(candidates) => {
                if self.state.open_image.as_ref() != Some(&candidates.source)
                    || !self.jobs.preview_is_current(candidates.generation)
                    || self.state.edit != candidates.before
                {
                    self.state.push_toast(
                        "Detections discarded because the image or edit changed; try again",
                        false,
                    );
                    return;
                }
                self.state.enhancement_status = format!(
                    "{} elements detected · choose previews before creating layers",
                    candidates.items.len()
                );
                self.state.mask_candidates = Some(*candidates);
                self.state
                    .push_toast(self.state.enhancement_status.clone(), false);
            }
            Event::Failed(error) => {
                self.state.enhancement_status = error.clone();
                self.state.push_toast(error, true);
            }
            Event::Cancelled => self.state.enhancement_status.clear(),
        }
        ctx.request_repaint();
    }
}

/// Runs after geometry/crop and before output sharpening or decorations.
pub(super) fn upscale_export(
    image: sp_export::ExportImage,
    settings: &sp_export::ExportSettings,
    hdr: bool,
    cancel: &AtomicBool,
) -> Result<sp_export::ExportImage, String> {
    let Some(model) = &settings.upscale_model else {
        return Ok(image);
    };
    if !hdr && settings.color_space != sp_export::ExportColorSpace::Srgb {
        return Err("AI RGB upscale currently requires sRGB SDR or Rec.2020 PQ HDR output".into());
    }
    if let Some(result) = super::native_enhance::try_upscale(&image, model, hdr, cancel) {
        return result;
    }
    let dir = tempfile::tempdir().map_err(|e| e.to_string())?;
    let input = dir.path().join("input.png");
    let output = dir.path().join("output.png");
    if let Some(pixels) = &image.rgba16 {
        let buffer = image::ImageBuffer::<image::Rgba<u16>, _>::from_raw(
            image.width,
            image.height,
            pixels.clone(),
        )
        .ok_or("Invalid 16-bit upscale input")?;
        buffer.save(&input).map_err(|e| e.to_string())?;
    } else {
        image::save_buffer(
            &input,
            &image.rgba8,
            image.width,
            image.height,
            image::ColorType::Rgba8,
        )
        .map_err(|e| e.to_string())?;
    }
    run_worker(
        dir.path(),
        serde_json::json!({"operation":"upscale", "input":input, "output":output, "model":model, "hdr":hdr}),
        cancel,
    )?;
    let (w, h) = image::ImageReader::open(&output)
        .map_err(|e| e.to_string())?
        .into_dimensions()
        .map_err(|e| e.to_string())?;
    if ![2, 4].into_iter().any(|scale| {
        image.width.checked_mul(scale) == Some(w) && image.height.checked_mul(scale) == Some(h)
    }) {
        return Err("Upscale returned invalid dimensions".into());
    }
    if u64::from(w) * u64::from(h) > 150_000_000 {
        return Err("Upscale exceeds 150 MP".into());
    }
    let mut reader = image::ImageReader::open(&output).map_err(|e| e.to_string())?;
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(1_500_000_000);
    reader.limits(limits);
    let out = reader.decode().map_err(|e| e.to_string())?.to_rgba16();
    sp_export::ExportImage::new_16(w, h, out.into_raw()).ok_or("Invalid upscale result".into())
}

pub(super) fn prepare_export(
    image: sp_export::ExportImage,
    geometry: sp_core::edit::GeometryParams,
    settings: &sp_export::ExportSettings,
    hdr: bool,
    cancel: &AtomicBool,
) -> Result<sp_export::ExportImage, String> {
    let prepare =
        |image: sp_export::ExportImage, geometry, settings: &sp_export::ExportSettings| {
            if hdr {
                sp_export::prepare_pq_image_owned(image, geometry, settings)
            } else {
                sp_export::prepare_image_already_converted_owned(image, geometry, settings)
            }
        };
    if settings.upscale_model.is_none() {
        return Ok(prepare(image, geometry, settings));
    }
    let mut first = settings.clone();
    // Super-resolution must see the developed crop at working resolution.
    // SizeMode describes the delivered file, so apply it after inference.
    first.size = sp_export::SizeMode::Original;
    first.output_sharpening = sp_export::OutputSharpening::None;
    first.watermark = None;
    first.text_watermark = None;
    first.border = None;
    let prepared = prepare(image, geometry, &first);
    let enhanced = upscale_export(prepared, settings, hdr, cancel)?;
    Ok(prepare(enhanced, Default::default(), settings))
}

#[cfg(test)]
mod mask_import_tests {
    use super::*;

    #[test]
    fn point_refinement_replaces_only_selected_alpha_preserves_edits_and_undoes() {
        let alpha = |pixels: Vec<u16>| sp_core::mask::AlphaMask {
            width: 3,
            height: 1,
            alpha: pixels.into(),
            source: "test".into(),
            inverted: false,
            repairs: vec![],
        };
        let mut layer = sp_core::edit::LocalAdjustment {
            name: "Motorcycle".into(),
            mask: sp_core::edit::LocalMask::Alpha(alpha(vec![65535, 65535, 65535])),
            opacity: 0.6,
            temperature: 12.,
            sharpness: 35.,
            ..Default::default()
        };
        layer.tone.exposure_ev = 1.2;
        layer.color.saturation = 17.;
        let mut before = EditState::default();
        before
            .local_adjustments
            .push(sp_core::edit::LocalAdjustment::default());
        before.local_adjustments.push(layer.clone());
        let refined = alpha(vec![65535, 0, 65535]); // Excluded foreground railing.
        let (next, active) = edit_with_masks(
            &before,
            vec![("Object selection".into(), refined.clone())],
            Some(1),
        )
        .unwrap();
        assert_eq!(active, 1);
        assert_eq!(next.local_adjustments.len(), 2);
        let mut expected = before.clone();
        expected.local_adjustments[1].mask = sp_core::edit::LocalMask::Alpha(refined);
        assert_eq!(
            next, expected,
            "all layer and other document edits must survive refinement"
        );
        let mut state = crate::state::AppState {
            edit: before.clone(),
            ..Default::default()
        };
        state.apply_edit_step(next.clone());
        state.undo();
        assert_eq!(state.edit, before);
        state.redo();
        assert_eq!(state.edit, next);
        assert!(edit_with_masks(&before, vec![], Some(1)).is_err());
        assert!(edit_with_masks(
            &before,
            vec![("test".into(), alpha(vec![1, 0, 1]))],
            Some(0)
        )
        .is_err());
        assert!(edit_with_masks(
            &before,
            vec![("test".into(), alpha(vec![1, 0, 1]))],
            Some(9)
        )
        .is_err());
    }

    #[test]
    fn instance_requests_validate_distinct_supported_selections() {
        assert_eq!(
            parse_mask_request("selected:semantic_people:4,7").unwrap(),
            ("semantic_people".into(), Some(vec![4, 7]))
        );
        assert_eq!(
            parse_mask_request("semantic_sky").unwrap(),
            ("semantic_sky".into(), None)
        );
        for request in [
            "selected:semantic_sky:0",
            "selected:semantic_people:",
            "selected:semantic_cars:2,2",
            "selected:semantic_motorcycles:no",
        ] {
            assert!(parse_mask_request(request).is_err(), "{request}");
        }
    }

    #[test]
    fn instance_manifest_validates_ids_bounds_paths_and_selected_full_resolution() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("instances.json");
        let preview = image::RgbaImage::from_pixel(3, 2, image::Rgba([20, 30, 40, 255]));
        preview.save(dir.path().join("candidate.png")).unwrap();
        let mask = image::ImageBuffer::<image::Luma<u16>, Vec<u16>>::from_raw(
            3,
            2,
            vec![0, 1234, 65535, 0, 0, 65535],
        )
        .unwrap();
        mask.save(dir.path().join("mask.png")).unwrap();
        let manifest = serde_json::json!({"version":1,"backend":"test-v1","width":3,"height":2,"candidates":[{"id":4,"confidence":0.9,"bounds":[0.0,0.0,1.0,1.0],"preview":"candidate.png","mask":"mask.png"}]});
        let write = |value: &serde_json::Value| {
            std::fs::write(&path, serde_json::to_vec(value).unwrap()).unwrap()
        };
        write(&manifest);
        let GeneratedSelection::Candidates {
            items: candidates, ..
        } = read_instance_selection(&path, (3, 2), None, None, "People").unwrap()
        else {
            panic!("expected previews")
        };
        assert_eq!(candidates.len(), 1);
        assert!(!candidates[0].selected);
        let GeneratedSelection::Masks(masks) =
            read_instance_selection(&path, (3, 2), Some(&[4]), Some("test-v1"), "People").unwrap()
        else {
            panic!("expected masks")
        };
        assert_eq!(&*masks[0].1.alpha, &[0, 1234, 65535, 0, 0, 65535]);
        assert!(read_instance_selection(
            &path,
            (3, 2),
            Some(&[4]),
            Some("different-model"),
            "People"
        )
        .is_err());
        assert!(
            read_instance_selection(&path, (3, 2), Some(&[5]), Some("test-v1"), "People").is_err()
        );
        assert!(
            read_instance_selection(&path, (2, 3), Some(&[4]), Some("test-v1"), "People").is_err()
        );
        for (field, value) in [
            ("preview", serde_json::json!("../other.png")),
            ("bounds", serde_json::json!([0.8, 0.0, 0.5, 1.0])),
            ("confidence", serde_json::json!(2.0)),
        ] {
            let mut invalid = manifest.clone();
            invalid["candidates"][0][field] = value;
            write(&invalid);
            assert!(
                read_instance_selection(&path, (3, 2), None, None, "People").is_err(),
                "{field}"
            );
        }
        let mut duplicate = manifest.clone();
        duplicate["candidates"]
            .as_array_mut()
            .unwrap()
            .push(manifest["candidates"][0].clone());
        write(&duplicate);
        assert!(read_instance_selection(&path, (3, 2), None, None, "People").is_err());
        write(&manifest);
        image::ImageBuffer::<image::Luma<u16>, Vec<u16>>::from_pixel(3, 2, image::Luma([0]))
            .save(dir.path().join("mask.png"))
            .unwrap();
        assert!(
            read_instance_selection(&path, (3, 2), Some(&[4]), Some("test-v1"), "People").is_err()
        );
    }

    #[test]
    fn generated_masks_reject_blank_or_misaligned_results_without_restricting_imports() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mask.png");
        image::GrayImage::new(3, 2).save(&path).unwrap();
        assert!(read_mask(&path, "Imported mask".into()).is_ok());
        assert!(read_generated_mask(&path, "Sky".into(), (3, 2))
            .unwrap_err()
            .contains("empty"));
        image::GrayImage::from_pixel(3, 2, image::Luma([255]))
            .save(&path)
            .unwrap();
        assert!(read_generated_mask(&path, "Sky".into(), (2, 3))
            .unwrap_err()
            .contains("dimensions"));
        assert!(read_generated_mask(&path, "Sky".into(), (3, 2)).is_ok());
    }

    #[test]
    #[ignore = "requires optional runtime and SHOPPHOTO_TEST_SR; SHOPPHOTO_TEST_SR4 additionally exercises real 4x"]
    fn actual_ai_upscale_runs_after_crop_without_upsampling_the_full_frame() {
        let mut models = vec![(
            2,
            std::env::var_os("SHOPPHOTO_TEST_SR").expect("SwinIR 2x checkpoint"),
        )];
        if let Some(model) = std::env::var_os("SHOPPHOTO_TEST_SR4") {
            models.push((4, model));
        }
        for (scale, model) in models {
            for size in [
                sp_export::SizeMode::Original,
                sp_export::SizeMode::Exact {
                    width: 192,
                    height: 144,
                },
            ] {
                let pixels = (0..256 * 192)
                    .flat_map(|index| {
                        let (x, y) = (index % 256, index / 256);
                        if (64..192).contains(&x) && (48..144).contains(&y) {
                            [16384, 32768, 49152, 65535]
                        } else {
                            [65535, 0, 0, 65535]
                        }
                    })
                    .collect();
                let source = sp_export::ExportImage::new_16(256, 192, pixels).unwrap();
                let settings = sp_export::ExportSettings {
                    upscale_model: Some(model.clone().into()),
                    size,
                    ..Default::default()
                };
                let geometry = sp_core::edit::GeometryParams {
                    crop: [0.25, 0.25, 0.75, 0.75],
                    ..Default::default()
                };
                let output =
                    prepare_export(source, geometry, &settings, false, &AtomicBool::new(false))
                        .unwrap();
                let expected = size.output_dims(128 * scale, 96 * scale);
                assert_eq!(
                    (output.width, output.height),
                    expected,
                    "crop first, then native {scale}x AI scale, then final resize"
                );
                let pixels = output.rgba16.unwrap();
                let center = &pixels
                    [((output.height / 2 * output.width + output.width / 2) * 4) as usize..][..3];
                assert!(
                    center[2] > center[1] && center[1] > center[0],
                    "the red area outside the crop must not enter inference: {center:?}"
                );
            }
        }
    }

    #[test]
    fn imports_transparency_or_grayscale_without_losing_precision() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mask.png");
        let rgba: image::ImageBuffer<image::Rgba<u16>, Vec<u16>> =
            image::ImageBuffer::from_raw(2, 1, vec![65535, 65535, 65535, 1234, 0, 0, 0, 54321])
                .unwrap();
        rgba.save(&path).unwrap();
        assert_eq!(
            &*read_mask(&path, "test".into()).unwrap().alpha,
            &[1234, 54321]
        );
        let gray: image::ImageBuffer<image::Luma<u16>, Vec<u16>> =
            image::ImageBuffer::from_raw(2, 1, vec![1234, 54321]).unwrap();
        gray.save(&path).unwrap();
        assert_eq!(
            &*read_mask(&path, "test".into()).unwrap().alpha,
            &[1234, 54321]
        );
        let rgba8: image::ImageBuffer<image::Rgba<u8>, Vec<u8>> =
            image::ImageBuffer::from_raw(2, 1, vec![255, 255, 255, 0, 0, 0, 0, 128]).unwrap();
        rgba8.save(&path).unwrap();
        assert_eq!(
            &*read_mask(&path, "test".into()).unwrap().alpha,
            &[0, 32896]
        );
    }
}

#[cfg(test)]
mod runtime_acceptance {
    use super::*;
    #[test]
    #[ignore = "requires installed optional Python/models and explicit portrait/sky fixtures"]
    fn actual_optional_runtime_masks_faces_and_sidecar_roundtrip() {
        let person =
            PathBuf::from(std::env::var_os("SHOPPHOTO_TEST_FACE_PHOTO").expect("portrait fixture"));
        let sky =
            PathBuf::from(std::env::var_os("SHOPPHOTO_TEST_SEMANTIC_SCENE").expect("sky fixture"));
        let directory = tempfile::tempdir().unwrap();
        let cancelled = AtomicBool::new(false);
        for (kind, input) in [("semantic_people", &person), ("semantic_sky", &sky)] {
            let output = directory.path().join(format!("{kind}.png"));
            run_worker(
                directory.path(),
                serde_json::json!({"operation":"mask","kind":kind,"input":input,"output":output}),
                &cancelled,
            )
            .unwrap();
            let mask = read_mask(&output, format!("MaskFormer + ViTMatte · {kind}")).unwrap();
            let dimensions = image::image_dimensions(input).unwrap();
            assert_eq!((mask.width, mask.height), dimensions);
            assert!(mask.alpha.iter().max().unwrap() > mask.alpha.iter().min().unwrap());
            let mut sidecar = sp_catalog::Sidecar::default();
            sidecar
                .edit
                .local_adjustments
                .push(sp_core::edit::LocalAdjustment {
                    mask: sp_core::edit::LocalMask::Alpha(mask),
                    ..Default::default()
                });
            let encoded = serde_json::to_vec(&sidecar).unwrap();
            let restored: sp_catalog::Sidecar = serde_json::from_slice(&encoded).unwrap();
            assert_eq!(
                restored.edit, sidecar.edit,
                "Generated alpha must survive saved edits"
            );
        }
        let output = directory.path().join("faces.json");
        run_worker(
            directory.path(),
            serde_json::json!({"operation":"analyze_faces","input":person,"output":output}),
            &cancelled,
        )
        .unwrap();
        let faces: sp_catalog::quality::FaceAnalysis =
            serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        assert!(faces.valid() && faces.faces >= 1 && faces.eye_regions >= 2);
    }
}
