//! Background per-image Auto analysis. Results are committed only if the
//! corresponding edit has not changed while the GPU was working.
use super::*;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

pub(super) enum Event {
    Prepared {
        source: PathBuf,
        edit_source: PathBuf,
        before: Box<sp_catalog::Sidecar>,
        after: Box<sp_catalog::Sidecar>,
        preview: sp_raw::Preview,
    },
    Progress(usize, usize),
    Error(String),
    Done,
}

impl ShopPhotoApp {
    pub(super) fn start_auto_finish(&mut self, ctx: &egui::Context) {
        if self.state.auto_finish_busy
            || self.state.decode_in_flight
            || self.state.export_busy
            || self.state.ai_denoise_busy
            || self.state.enhancement_busy
        {
            return;
        }
        // A newly opened image can have ISO-seeded settings without a
        // sidecar yet. Snapshot those settings before the worker reads it.
        if self.state.open_image.is_some() {
            self.state.sidecar_dirty = true;
        }
        if !self.save_sidecar_if_due(true) {
            return;
        }
        if !self.jobs.flush_persist(std::time::Duration::from_secs(5)) {
            self.state.push_toast(
                "Could not flush edits before preparing the batch; try again",
                true,
            );
            return;
        }
        let Some(gpu) = &self.gpu else {
            return;
        };
        let mut sources: Vec<_> = self.state.selected_photos.iter().cloned().collect();
        if sources.is_empty() {
            sources.extend(self.state.open_image.clone());
        }
        sources.sort();
        if sources.is_empty() {
            return;
        }
        // A batch addresses primary edits. Do not silently overwrite a
        // virtual copy or apply its edit to the primary image.
        if self.state.virtual_copy.is_some() {
            self.state
                .push_toast("Select the primary edit before preparing a batch", false);
            return;
        }
        let gpu_ctx = GpuContext::new(
            gpu.render_state.device.clone(),
            gpu.render_state.queue.clone(),
        );
        let pipelines = gpu.renderer.shared_pipelines();
        let compute = self.state.raw_denoise_compute;
        let profiles = self.profiles.clone();
        let looks = self.custom_looks.clone();
        let lenses = self.lenses.clone();
        let source_cache = source_cache_dir();
        let cancel = Arc::new(AtomicBool::new(false));
        let cancelled = cancel.clone();
        let (tx, rx) = crossbeam_channel::bounded(2);
        let repaint = ctx.clone();
        let result = std::thread::Builder::new()
            .name("sp-auto-finish".into())
            .spawn(move || {
                let run = (|| -> Result<(), String> {
                    let mut renderer = GpuRenderer::with_pipelines(gpu_ctx, pipelines)
                        .map_err(|e| e.to_string())?;
                    let total = sources.len();
                    for (index, source) in sources.into_iter().enumerate() {
                        if cancelled.load(Ordering::Relaxed) {
                            break;
                        }
                        let item = (|| -> Result<(), String> {
                            let edit_source = source_cache
                                .as_ref()
                                .map(|cache| sp_catalog::source_cache::prepare(cache, &source))
                                .transpose()
                                .map_err(|e| e.to_string())?
                                .map_or_else(|| source.clone(), |p| p.decode_path);
                            let existing = sp_catalog::load_sidecar(&edit_source)
                                .map_err(|e| e.to_string())?;
                            let fresh = existing.is_none();
                            let before = existing.unwrap_or_default();
                            let decoded =
                                sp_raw::decode_source(&edit_source).map_err(|e| e.to_string())?;
                            let mut image_source = decoded.source.clone();
                            let mut ai_applied = false;
                            if image_source.is_raw()
                                && before.edit.nr.camera_denoise_mode
                                    != sp_core::edit::CameraDenoiseMode::Off
                            {
                                let raw = image_source.as_raw().expect("checked RAW source");
                                let root = sp_ai::model_root()
                                    .ok_or("Selected AI denoise model is unavailable")?;
                                let inferred =
                                    crate::inference_cache::infer_with_compute(raw, &root, compute)
                                        .map_err(|e| e.to_string())?;
                                image_source = ImageSource::Raw(Arc::new(sp_ai::blend(
                                    raw,
                                    &inferred,
                                    before.edit.nr.camera_denoise_mode.strength(decoded.meta.iso),
                                )));
                                ai_applied = true;
                            }
                            let mut analysis = before.edit.clone();
                            if fresh && image_source.is_raw() {
                                analysis.nr =
                                    sp_core::edit::NrParams::seeded_for_iso(decoded.meta.iso);
                            }
                            analysis.tone = Default::default();
                            renderer.set_capture_metadata(Some(&decoded.meta));
                            renderer.set_lens_profile(
                                resolve_lens(&lenses, &analysis.lens, &decoded.meta),
                                decoded.meta.aperture_f,
                            );
                            renderer
                                .open_source_with_edit(&image_source, &analysis)
                                .map_err(|e| e.to_string())?;
                            renderer.set_external_raw_denoise(ai_applied);
                            let profile = match &analysis.profile {
                                sp_core::edit::ProfileChoice::Named(name) => profiles
                                    .iter()
                                    .find(|p| {
                                        &p.name == name
                                            && camera_matches(
                                                &p.camera_model,
                                                &decoded.meta.camera_model,
                                            )
                                    })
                                    .cloned(),
                                _ => None,
                            };
                            renderer.set_profile(profile);
                            renderer.set_custom_look(
                                analysis.look.as_ref().and_then(|name| {
                                    looks.iter().find(|(n, _)| n == name).cloned()
                                }),
                            );
                            renderer.render(&analysis);
                            // Readback waits on this worker, never in the frame loop.
                            let _ = renderer.read_display_srgb8().map_err(|e| e.to_string())?;
                            let histogram = renderer
                                .poll_histogram()
                                .ok_or("Auto histogram was unavailable")?;
                            let stats =
                                sp_pipeline::auto_tone::HistogramStats::from_rgb_bins(&histogram)
                                    .ok_or("Empty Auto histogram")?;
                            let mut after = before.clone();
                            if fresh {
                                after.edit.nr = analysis.nr;
                                after.annotations = sp_catalog::xmp::load(&source)
                                    .ok()
                                    .flatten()
                                    .unwrap_or_default();
                            }
                            after.version = sp_core::edit::EDIT_SCHEMA_VERSION;
                            let now = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_secs() as i64;
                            after.history.push(sp_catalog::HistoryEntry {
                                label: "Before Auto Finish 0.3.0".into(),
                                created_s: now,
                                edit: before.edit.clone(),
                            });
                            if after.history.len() > 200 {
                                after.history.remove(0);
                            }
                            after.edit.tone = sp_pipeline::auto_tone::auto_tone(&stats);
                            let median = stats.percentile_encoded(0.5);
                            if !(0.08..=0.85).contains(&median)
                                || stats.percentile_encoded(0.995) - stats.percentile_encoded(0.005)
                                    < 0.12
                            {
                                let _ = tx.send(Event::Error({ let __l10n_args = (&(source.display()),); crate::i18n::format("{0}: extreme scene key or low contrast; inspect Auto Balance", &[__l10n_args.0.to_string()]) }));
                            }
                            renderer.render_without_histogram(&after.edit);
                            let (rgba, width, height) =
                                renderer.read_display_srgb8().map_err(|e| e.to_string())?;
                            if cancelled.load(Ordering::Relaxed) {
                                return Ok(());
                            }
                            tx.send(Event::Prepared {
                                source: source.clone(),
                                edit_source,
                                before: Box::new(before),
                                after: Box::new(after),
                                preview: sp_raw::Preview {
                                    rgba8: rgba,
                                    width,
                                    height,
                                },
                            })
                            .map_err(|e| e.to_string())?;
                            repaint.request_repaint();
                            Ok(())
                        })();
                        if let Err(error) = item {
                            let _ = tx.send(Event::Error(format!("{}: {error}", source.display())));
                        }
                        let _ = tx.send(Event::Progress(index + 1, total));
                        repaint.request_repaint();
                    }
                    Ok(())
                })();
                if let Err(error) = run {
                    let _ = tx.send(Event::Error(error));
                }
                let _ = tx.send(Event::Done);
                repaint.request_repaint();
            });
        match result {
            Ok(_) => {
                self.auto_finish_job = Some(rx);
                self.auto_finish_cancel = Some(cancel);
                self.state.auto_finish_busy = true;
                self.state.auto_finish_pending = 0;
                self.state.auto_finish_saved = 0;
                self.state.auto_finish_analyzed = false;
                self.state.auto_finish_progress = 0.0;
                self.state.auto_finish_errors.clear();
                self.state.auto_finish_status = "Analyzing selected photos…".into();
            }
            Err(error) => self.state.push_toast(
                crate::i18n::format("Could not start Auto Finish: {0}", &[error.to_string()]),
                true,
            ),
        }
    }

    pub(super) fn poll_auto_finish(&mut self, ctx: &egui::Context) {
        let events: Vec<_> = self
            .auto_finish_job
            .as_ref()
            .map(|rx| rx.try_iter().collect())
            .unwrap_or_default();
        for event in events {
            match event {
                Event::Prepared {
                    source,
                    edit_source,
                    before,
                    after,
                    preview,
                } => {
                    let active = self.state.open_image.as_ref() == Some(&source);
                    let changed = active
                        && (self.state.edit != before.edit || self.state.virtual_copy.is_some());
                    if changed {
                        self.state.auto_finish_errors.push({
                            let __l10n_args = (&(source.display()),);
                            crate::i18n::format(
                                "{0}: skipped because its edit changed",
                                &[__l10n_args.0.to_string()],
                            )
                        });
                        continue;
                    }
                    // Commit through the existing persistence lane, preserving
                    // annotations, snapshots and all denoise parameters.
                    self.state.auto_finish_pending += 1;
                    self.jobs.persist(PersistRequest::AutoFinish {
                        source,
                        edit_source,
                        before,
                        after,
                        preview,
                        thumbnail_cache: thumbnail_cache_dir(),
                    });
                }
                Event::Progress(done, total) => {
                    self.state.auto_finish_progress = done as f32 / total.max(1) as f32;
                    self.state.auto_finish_status = {
                        let __l10n_args = (&(self.state.auto_finish_errors.len()),);
                        crate::i18n::format(
                            "{0} / {1} analyzed · {2} review items",
                            &[
                                done.to_string(),
                                total.to_string(),
                                __l10n_args.0.to_string(),
                            ],
                        )
                    };
                }
                Event::Error(error) => self.state.auto_finish_errors.push(error),
                Event::Done => {
                    self.state.auto_finish_analyzed = true;
                    self.auto_finish_job = None;
                }
            }
        }
        if self.state.auto_finish_busy
            && self.state.auto_finish_analyzed
            && self.state.auto_finish_pending == 0
        {
            let cancelled = self
                .auto_finish_cancel
                .as_ref()
                .is_some_and(|c| c.load(Ordering::Relaxed));
            self.state.auto_finish_busy = false;
            self.state.auto_finish_status = {
                let __l10n_args = (
                    &(if cancelled {
                        "Preparation cancelled"
                    } else {
                        "Ready for review"
                    }),
                    &(self.state.auto_finish_saved),
                    &(self.state.auto_finish_errors.len()),
                );
                crate::i18n::format(
                    "{0} · {1} photos saved · {2} review items",
                    &[
                        __l10n_args.0.to_string(),
                        __l10n_args.1.to_string(),
                        __l10n_args.2.to_string(),
                    ],
                )
            };
            self.auto_finish_cancel = None;
            self.state
                .push_toast(self.state.auto_finish_status.clone(), false);
        }
        if self.state.auto_finish_busy {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
    }
}
