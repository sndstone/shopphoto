//! Reviewable exposure/WB continuity suggestions for selected sequences.
use super::*;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

pub(crate) struct Proposal {
    pub enabled: bool,
    pub source: PathBuf,
    edit_source: PathBuf,
    before: Box<sp_catalog::Sidecar>,
    after: Box<sp_catalog::Sidecar>,
    preview: sp_raw::Preview,
    samples: usize,
    spread: f32,
    correlation: f32,
    reference_source: PathBuf,
    reference_edit_source: PathBuf,
    reference_before: Box<sp_catalog::Sidecar>,
    texture: Option<egui::TextureHandle>,
}
impl std::fmt::Debug for Proposal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Proposal")
            .field("source", &self.source)
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
pub(crate) struct State {
    pub open: bool,
    pub analyze_requested: bool,
    pub apply_requested: bool,
    pub cancel_requested: bool,
    pub busy: bool,
    pub waiting: bool,
    pub sources: Vec<PathBuf>,
    pub reference: usize,
    pub copy_wb: bool,
    pub align: bool,
    pub proposals: Vec<Proposal>,
    pub errors: Vec<String>,
    pub status: String,
    pub progress: f32,
}
impl Default for State {
    fn default() -> Self {
        Self {
            open: false,
            analyze_requested: false,
            apply_requested: false,
            cancel_requested: false,
            busy: false,
            waiting: false,
            sources: Vec::new(),
            reference: 0,
            copy_wb: true,
            align: true,
            proposals: Vec::new(),
            errors: Vec::new(),
            status: String::new(),
            progress: 0.0,
        }
    }
}

pub(super) enum Event {
    Progress(f32, String),
    Problem(String),
    Done(Result<Vec<Proposal>, String>),
}

impl ShopPhotoApp {
    pub(super) fn sequence_upkeep(&mut self, ctx: &egui::Context) {
        if std::mem::take(&mut self.state.sequence.cancel_requested) {
            if let Some(cancel) = &self.sequence_cancel {
                cancel.store(true, Ordering::Relaxed);
            }
        }
        if self.state.sequence.waiting && !self.state.auto_finish_busy {
            self.state.sequence.waiting = false;
            self.state.sequence.busy = false;
            self.state.sequence.status = {
                let __l10n_args = (&(self.state.auto_finish_saved),);
                crate::i18n::format("{0} photographs updated", &[__l10n_args.0.to_string()])
            };
            self.state
                .sequence
                .errors
                .extend(self.state.auto_finish_errors.clone());
        }
        if let Some(receiver) = &self.sequence_job {
            let mut finished = None;
            loop {
                match receiver.try_recv() {
                    Ok(Event::Progress(progress, status)) => {
                        self.state.sequence.progress = progress;
                        self.state.sequence.status = status;
                    }
                    Ok(Event::Problem(error)) => self.state.sequence.errors.push(error),
                    Ok(Event::Done(result)) => {
                        finished = Some(result);
                        break;
                    }
                    Err(crossbeam_channel::TryRecvError::Empty) => break,
                    Err(crossbeam_channel::TryRecvError::Disconnected) => {
                        finished = Some(Err("Sequence analysis stopped unexpectedly".into()));
                        break;
                    }
                }
            }
            if let Some(result) = finished {
                self.sequence_job = None;
                self.sequence_cancel = None;
                self.state.sequence.busy = false;
                match result {
                    Ok(proposals) => {
                        self.state.sequence.status = {
                            let __l10n_args = (&(proposals.len()),);
                            crate::i18n::format(
                                "Review {0} proposed changes before applying",
                                &[__l10n_args.0.to_string()],
                            )
                        };
                        self.state.sequence.proposals = proposals;
                    }
                    Err(error) => self.state.sequence.status = error,
                }
            }
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        if std::mem::take(&mut self.state.sequence.analyze_requested) {
            self.analyze_sequence(ctx);
        }
        if std::mem::take(&mut self.state.sequence.apply_requested) {
            self.apply_sequence();
        }
    }

    fn analyze_sequence(&mut self, ctx: &egui::Context) {
        if self.state.sequence.busy
            || self.state.auto_finish_busy
            || self.state.export_busy
            || self.state.merge.busy
        {
            return;
        }
        if self.state.virtual_copy.is_some() {
            self.state.sequence.status = "Select primary edits before matching a sequence".into();
            return;
        }
        let sources = self.state.sequence.sources.clone();
        if !(2..=64).contains(&sources.len()) {
            self.state.sequence.status = "Select 2–64 sequence photographs".into();
            return;
        }
        if self.state.open_image.is_some() {
            self.state.sidecar_dirty = true;
        }
        if !self.save_sidecar_if_due(true) {
            return;
        }
        if !self.jobs.flush_persist(std::time::Duration::from_secs(5)) {
            self.state.sequence.status =
                "Could not flush current edits before analysis; retry".into();
            return;
        }
        let Some(gpu) = &self.gpu else {
            return;
        };
        let gpu_ctx = GpuContext::new(
            gpu.render_state.device.clone(),
            gpu.render_state.queue.clone(),
        );
        let pipelines = gpu.renderer.shared_pipelines();
        let profiles = self.profiles.clone();
        let looks = self.custom_looks.clone();
        let lenses = self.lenses.clone();
        let reference = self.state.sequence.reference.min(sources.len() - 1);
        let copy_wb = self.state.sequence.copy_wb;
        let align = self.state.sequence.align;
        let cache = source_cache_dir();
        let cancel = Arc::new(AtomicBool::new(false));
        let cancelled = cancel.clone();
        let (tx, rx) = crossbeam_channel::unbounded();
        let repaint = ctx.clone();
        std::thread::spawn(move || {
            let result = (|| -> Result<Vec<Proposal>, String> {
                let edit_path = |source: &std::path::Path| -> Result<PathBuf, String> {
                    cache
                        .as_ref()
                        .map(|cache| sp_catalog::source_cache::prepare(cache, source))
                        .transpose()
                        .map_err(|e| e.to_string())
                        .map(|prepared| {
                            prepared.map_or_else(|| source.to_owned(), |p| p.decode_path)
                        })
                };
                let path = edit_path(&sources[reference])?;
                let reference_edit = sp_catalog::load_sidecar(&path)
                    .map_err(|e| e.to_string())?
                    .unwrap_or_default();
                let decoded = sp_raw::decode_source(&path).map_err(|e| e.to_string())?;
                let reference_identity = (
                    decoded.source.is_raw(),
                    decoded.meta.camera_make.clone(),
                    decoded.meta.camera_model.clone(),
                );
                let wb = sp_core::sequence::reference_white_balance(
                    &decoded.source,
                    reference_edit.edit.wb,
                )?;
                let reference_pixels = sp_core::sequence::thumbnail(
                    &decoded.source,
                    if copy_wb { wb } else { reference_edit.edit.wb },
                    reference_edit.edit.tone.exposure_ev,
                    &cancelled,
                )?;
                drop(decoded);
                let mut renderer =
                    GpuRenderer::with_pipelines(gpu_ctx, pipelines).map_err(|e| e.to_string())?;
                renderer.set_proxy_long_edge(512);
                let mut proposals = Vec::new();
                for (index, source) in sources.iter().enumerate() {
                    if index == reference {
                        continue;
                    }
                    if cancelled.load(Ordering::Relaxed) {
                        return Err("Sequence matching cancelled".into());
                    }
                    let _ = tx.send(Event::Progress(index as f32 / sources.len() as f32, {
                        let __l10n_args =
                            (&(source.file_name().unwrap_or_default().to_string_lossy()),);
                        crate::i18n::format("Matching {0}", &[__l10n_args.0.to_string()])
                    }));
                    repaint.request_repaint();
                    let item = (|| -> Result<Proposal, String> {
                        let edit_source = edit_path(source)?;
                        let before = sp_catalog::load_sidecar(&edit_source)
                            .map_err(|e| e.to_string())?
                            .unwrap_or_default();
                        let decoded =
                            sp_raw::decode_source(&edit_source).map_err(|e| e.to_string())?;
                        if decoded.source.is_raw() != reference_identity.0
                            || (reference_identity.0
                                && (decoded.meta.camera_make != reference_identity.1
                                    || decoded.meta.camera_model != reference_identity.2))
                        {
                            return Err("Use the same RAW camera family or rendered RGB sources throughout a sequence".into());
                        }
                        let mut after = before.clone();
                        if copy_wb {
                            after.edit.wb = wb;
                        }
                        let measured = sp_core::sequence::thumbnail(
                            &decoded.source,
                            after.edit.wb,
                            before.edit.tone.exposure_ev,
                            &cancelled,
                        )?;
                        let matched = sp_core::sequence::match_exposure(
                            &reference_pixels,
                            &measured,
                            align,
                            &cancelled,
                        )?;
                        let exposure = before.edit.tone.exposure_ev + matched.delta_ev;
                        if !(-5.0..=5.0).contains(&exposure) {
                            return Err(crate::i18n::format(
                                "Required exposure {0} EV exceeds the develop slider range",
                                &[format!("{:+.2}", exposure)],
                            ));
                        }
                        after.edit.tone.exposure_ev = exposure;
                        after.version = sp_core::edit::EDIT_SCHEMA_VERSION;
                        after.history.push(sp_catalog::HistoryEntry {
                            label: "Before sequence exposure/WB match".into(),
                            created_s: std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_secs() as i64,
                            edit: before.edit.clone(),
                        });
                        if after.history.len() > 200 {
                            after.history.remove(0);
                        }
                        renderer.set_capture_metadata(Some(&decoded.meta));
                        renderer.set_lens_profile(
                            resolve_lens(&lenses, &after.edit.lens, &decoded.meta),
                            decoded.meta.aperture_f,
                        );
                        renderer
                            .open_source_with_edit(&decoded.source, &after.edit)
                            .map_err(|e| e.to_string())?;
                        renderer.set_profile(match &after.edit.profile {
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
                        });
                        renderer.set_custom_look(
                            after
                                .edit
                                .look
                                .as_ref()
                                .and_then(|name| looks.iter().find(|(n, _)| n == name).cloned()),
                        );
                        renderer.render_without_histogram(&after.edit);
                        let (rgba8, width, height) =
                            renderer.read_display_srgb8().map_err(|e| e.to_string())?;
                        Ok(Proposal {
                            enabled: true,
                            source: source.clone(),
                            edit_source,
                            before: Box::new(before),
                            after: Box::new(after),
                            preview: sp_raw::Preview {
                                rgba8,
                                width,
                                height,
                            },
                            samples: matched.samples,
                            spread: matched.spread_ev,
                            correlation: matched.correlation,
                            reference_source: sources[reference].clone(),
                            reference_edit_source: path.clone(),
                            reference_before: Box::new(reference_edit.clone()),
                            texture: None,
                        })
                    })();
                    match item {
                        Ok(proposal) => proposals.push(proposal),
                        Err(error) => {
                            let _ = tx.send(Event::Problem(format!(
                                "{}: {error}",
                                source.file_name().unwrap_or_default().to_string_lossy()
                            )));
                        }
                    }
                }
                if cancelled.load(Ordering::Relaxed) {
                    return Err("Sequence matching cancelled".into());
                }
                Ok(proposals)
            })();
            let _ = tx.send(Event::Done(result));
            repaint.request_repaint();
        });
        self.sequence_job = Some(rx);
        self.sequence_cancel = Some(cancel);
        self.state.sequence.busy = true;
        self.state.sequence.proposals.clear();
        self.state.sequence.errors.clear();
        self.state.sequence.progress = 0.0;
        self.state.sequence.status = "Analyzing overlapping sequence…".into();
    }

    fn apply_sequence(&mut self) {
        if self.state.sequence.busy || self.state.auto_finish_busy {
            return;
        }
        let Some(reference) = self.state.sequence.proposals.first() else {
            return;
        };
        let reference_changed =
            sp_catalog::load_sidecar(&reference.reference_edit_source).map_or(true, |current| {
                current.unwrap_or_default() != *reference.reference_before
            }) || (self.state.open_image.as_ref() == Some(&reference.reference_source)
                && (self.state.virtual_copy.is_some()
                    || self.state.edit != reference.reference_before.edit));
        if reference_changed {
            self.state.sequence.status =
                "The reference edit changed. Analyze again before applying the sequence match."
                    .into();
            return;
        }
        self.state.auto_finish_pending = 0;
        self.state.auto_finish_saved = 0;
        self.state.auto_finish_errors.clear();
        for proposal in std::mem::take(&mut self.state.sequence.proposals)
            .into_iter()
            .filter(|p| p.enabled)
        {
            if self.state.open_image.as_ref() == Some(&proposal.source)
                && (self.state.virtual_copy.is_some()
                    || self.state.edit != proposal.before.edit
                    || self.state.annotations != proposal.before.annotations
                    || self.state.descriptive_metadata != proposal.before.metadata
                    || self.state.snapshots != proposal.before.snapshots
                    || self.state.persistent_history != proposal.before.history)
            {
                self.state.sequence.errors.push({
                    let __l10n_args = (&(proposal.source.display()),);
                    crate::i18n::format(
                        "{0}: skipped because its edit changed",
                        &[__l10n_args.0.to_string()],
                    )
                });
                continue;
            }
            self.state.auto_finish_pending += 1;
            self.jobs.persist(PersistRequest::AutoFinish {
                source: proposal.source,
                edit_source: proposal.edit_source,
                before: proposal.before,
                after: proposal.after,
                preview: proposal.preview,
                thumbnail_cache: thumbnail_cache_dir(),
            });
        }
        self.state.auto_finish_analyzed = true;
        self.state.auto_finish_busy = true;
        self.state.auto_finish_status = "Saving sequence exposure/WB matches…".into();
        self.state.sequence.busy = true;
        self.state.sequence.waiting = true;
        self.state.sequence.status = "Saving reviewed changes; changed edits are skipped".into();
    }
}

pub(crate) fn window(ctx: &egui::Context, state: &mut State) {
    if !state.open {
        return;
    }
    egui::Window::new(crate::i18n::text("Match sequence exposure / WB")).open(&mut state.open).default_width(650.0).show(ctx,|ui|{
        ui.label(crate::i18n::text("Select a reference from the same scene. Exposure is measured from corresponding unclipped pixels."));
        ui.add_enabled_ui(!state.busy,|ui|{
            let mut changed=false;
            egui::ScrollArea::vertical().max_height(150.0).show(ui,|ui|{for (index,path) in state.sources.iter().enumerate(){changed|=ui.radio_value(&mut state.reference,index,path.file_name().unwrap_or_default().to_string_lossy()).changed();}});
            changed|=ui.checkbox(&mut state.copy_wb,crate::i18n::text("Use the reference's resolved white balance throughout the sequence")).changed();
            changed|=ui.checkbox(&mut state.align,crate::i18n::text("Align small camera movements before measuring exposure")).changed();
            if changed {state.proposals.clear();}
            if ui.button(crate::i18n::text("Analyze and preview changes")).clicked(){state.analyze_requested=true;}
        });
        if !state.proposals.is_empty(){
            ui.separator();ui.label(crate::i18n::text("Apply only the checked suggestions. Other develop settings stay intact."));
            egui::ScrollArea::vertical().max_height(240.0).show(ui,|ui|{for proposal in &mut state.proposals {
                let texture=proposal.texture.get_or_insert_with(||ctx.load_texture(format!("sequence-preview-{}",proposal.source.display()),egui::ColorImage::from_rgba_unmultiplied([proposal.preview.width as usize,proposal.preview.height as usize],&proposal.preview.rgba8),egui::TextureOptions::LINEAR));
                let size=egui::vec2(128.0,128.0*proposal.preview.height as f32/proposal.preview.width as f32);
                ui.image((texture.id(),size));
                let wb=proposal.after.edit.wb.temp_kelvin.map(|temp|{ let __l10n_args = (&(proposal.after.edit.wb.tint),); crate::i18n::format("{0} K / {1} tint", &[format!("{:.0}", temp), format!("{:+.1}", __l10n_args.0)]) }).unwrap_or_else(||"Original WB".into());
                ui.checkbox(&mut proposal.enabled,format!("{} · {:+.2} → {:+.2} EV · {wb}",proposal.source.file_name().unwrap_or_default().to_string_lossy(),proposal.before.edit.tone.exposure_ev,proposal.after.edit.tone.exposure_ev));
                ui.small({ let __l10n_args = (&(proposal.samples), &(proposal.spread), &(proposal.correlation),); crate::i18n::format("{0} paired samples · {1} EV spread · {2} alignment correlation", &[__l10n_args.0.to_string(), format!("{:.2}", __l10n_args.1), format!("{:.2}", __l10n_args.2)]) });
            }});
            if ui.add_enabled(!state.busy,egui::Button::new(crate::i18n::text("Apply checked changes"))).clicked(){state.apply_requested=true;}
        }
        if state.busy {ui.add(egui::ProgressBar::new(state.progress).show_percentage());if !state.waiting&&ui.button(crate::i18n::text("Cancel analysis")).clicked(){state.cancel_requested=true;}}
        if !state.status.is_empty(){ui.label(crate::i18n::text(&state.status));}
        for error in &state.errors {ui.colored_label(egui::Color32::LIGHT_RED,error);}
    });
}
