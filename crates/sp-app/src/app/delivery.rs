//! Persistent photo delivery recipes and a serial GPU queue.
use super::*;
use sp_export::queue::{ExportQueue, ExportTask, TaskStatus};
fn queue_path() -> Option<PathBuf> {
    dirs::data_local_dir().map(|p| p.join("shopphoto/export-queue.json"))
}

fn delivery_versions(
    edit_source: &std::path::Path,
    source: &std::path::Path,
    mode: crate::state::VersionExportMode,
    selected: &std::collections::HashSet<(PathBuf, String)>,
    current: Option<&str>,
) -> Result<Vec<(String, String)>, String> {
    use crate::state::VersionExportMode as Versions;
    if mode == Versions::Primary || (mode == Versions::Current && current.is_none_or(str::is_empty))
    {
        return Ok(vec![(String::new(), "Primary".into())]);
    }
    let mut versions = vec![(String::new(), "Primary".into())];
    versions.extend(
        sp_catalog::workflow::virtual_copy_records(edit_source)
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(|version| (version.id, version.name)),
    );
    if mode == Versions::Current {
        versions.retain(|(id, _)| Some(id.as_str()) == current);
    }
    if mode == Versions::Selected {
        versions.retain(|(id, _)| selected.contains(&(source.to_owned(), id.clone())));
    }
    Ok(versions)
}

fn version_suffix(id: &str, name: &str) -> String {
    if id.is_empty() {
        return "primary".into();
    }
    let safe = |value: &str| {
        value
            .chars()
            .take(48)
            .map(|c| if c.is_alphanumeric() { c } else { '-' })
            .collect::<String>()
    };
    format!("{}-{}", safe(name), safe(id))
}
fn saved_version_snapshot(
    path: &std::path::Path,
    id: &str,
) -> Result<(sp_core::EditState, sp_export::PhotoMetadata), String> {
    let sidecar = if id.is_empty() {
        sp_catalog::load_sidecar(path)
    } else {
        sp_catalog::load_sidecar_path(&sp_catalog::virtual_copy_sidecar_path(path, id))
    }
    .map_err(|e| e.to_string())?;
    match sidecar {
        Some(sidecar) => Ok((
            sidecar.edit,
            export::export_metadata(&sidecar.metadata, sidecar.annotations),
        )),
        None if id.is_empty() => {
            let metadata = sp_catalog::xmp::load_with_metadata(path)
                .map_err(|e| e.to_string())?
                .map_or_else(sp_export::PhotoMetadata::default, |document| {
                    export::export_metadata(
                        &document.metadata,
                        document.annotations.unwrap_or_default(),
                    )
                });
            Ok((sp_core::EditState::default(), metadata))
        }
        None => Err({
            let __l10n_args = (&(path.display()),);
            crate::i18n::format(
                "Version {0} is missing for {1}",
                &[id.to_string(), __l10n_args.0.to_string()],
            )
        }),
    }
}

impl ShopPhotoApp {
    pub(super) fn poll_delivery(&mut self, ctx: &egui::Context) {
        if !self.state.export_queue_loaded {
            self.state.export_queue_loaded = true;
            match queue_path()
                .ok_or_else(|| "Export queue storage is unavailable".to_owned())
                .and_then(|p| {
                    if self.export_queue_lock.is_none() {
                        self.export_queue_lock = Some(sp_export::queue::QueueLock::acquire(&p)?);
                    }
                    ExportQueue::load(&p)
                }) {
                Ok(queue) => {
                    self.state.export_queue = queue;
                    self.state.export_queue_error = None;
                }
                Err(error) => self.state.export_queue_error = Some(error),
            }
        }
        if let Some(result) = self
            .export_queue_prepare
            .as_ref()
            .and_then(|rx| rx.try_recv().ok())
        {
            self.export_queue_prepare = None;
            self.state.export_enqueue_busy = false;
            match result {
                Ok(tasks) => {
                    let count = tasks.len();
                    for task in tasks {
                        self.state.export_queue.add(task);
                    }
                    self.state.export_queue_dirty = true;
                    self.state.export_queue_open = true;
                    self.state.push_toast(
                        crate::i18n::format("Added {0} delivery tasks", &[count.to_string()]),
                        false,
                    );
                }
                Err(error) => self.state.push_toast(
                    crate::i18n::format(
                        "Could not prepare queue: {0}",
                        std::slice::from_ref(&error),
                    ),
                    true,
                ),
            }
        }
        if std::mem::take(&mut self.state.export_enqueue_requested) {
            self.prepare_delivery(ctx);
        }
        if self.state.export_queue_dirty && self.state.export_queue_error.is_none() {
            self.save_delivery();
        }
        if self.state.export_queue_error.is_some()
            || self.state.export_enqueue_busy
            || self.export_job.is_some()
            || self.state.export_busy
            || self.state.auto_finish_busy
            || self.state.enhancement_busy
            || self.state.ai_denoise_busy
            || self.state.export_start_requested
        {
            return;
        }
        if self.gpu.is_none() {
            return;
        }
        if let Some(task) = self.state.export_queue.start_next() {
            self.state.export_queue_dirty = true;
            self.save_delivery();
            if self.state.export_queue_error.is_some() {
                self.state
                    .export_queue
                    .finish(task.id, Err("Queue could not be saved".into()));
                return;
            }
            let id = task.id;
            self.start_export_request(ctx, Some(task));
            if self.export_job.is_some() {
                self.export_queue_active = Some(id);
            } else {
                self.state
                    .export_queue
                    .finish(id, Err("Export could not start".into()));
                self.state.export_queue_dirty = true;
            }
        }
    }
    pub(super) fn save_delivery(&mut self) {
        match queue_path()
            .ok_or_else(|| "Export queue storage is unavailable".to_owned())
            .and_then(|p| self.state.export_queue.save(&p))
        {
            Ok(()) => self.state.export_queue_dirty = false,
            Err(error) => {
                self.state.export_queue.paused = true;
                self.state.export_queue_error = Some(error.clone());
                self.state.push_toast(
                    crate::i18n::format(
                        "Could not save export queue: {0}",
                        std::slice::from_ref(&error),
                    ),
                    true,
                );
            }
        }
    }
    fn prepare_delivery(&mut self, ctx: &egui::Context) {
        if self.export_queue_prepare.is_some() || self.state.export_queue_error.is_some() {
            return;
        }
        if self.state.decode_in_flight {
            self.state.push_toast(
                "Wait for the photo to finish loading before adding delivery tasks",
                false,
            );
            return;
        }
        let Some(current) = self.state.open_image.clone() else {
            self.state
                .push_toast("Open a photo to add delivery tasks", false);
            return;
        };
        let current_edit = self.state.edit.clone();
        let current_description =
            export::export_metadata(&self.state.descriptive_metadata, self.state.annotations);
        let current_version = self.state.virtual_copy.clone().unwrap_or_default();
        let version_mode = self.state.export_version_mode;
        let selected_versions = self.state.export_version_choices.clone();
        let mut sources = if self.state.export_batch {
            self.state
                .selected_photos
                .iter()
                .cloned()
                .collect::<Vec<_>>()
        } else {
            vec![current.clone()]
        };
        sources.sort();
        sources.dedup();
        if sources.is_empty() {
            sources.push(current.clone());
        }
        let mut recipes = self
            .state
            .export_presets
            .iter()
            .filter(|p| self.state.export_recipe_names.contains(&p.name))
            .cloned()
            .collect::<Vec<_>>();
        if recipes.is_empty() {
            recipes.push(sp_export::ExportPreset {
                name: "Current settings".into(),
                settings: self.state.export_settings.clone(),
            });
        }
        let cache = source_cache_dir();
        let context = ctx.clone();
        let (tx, rx) = crossbeam_channel::bounded(1);
        std::thread::spawn(move || {
            let result = (|| -> Result<Vec<ExportTask>, String> {
                let mut tasks = Vec::new();
                for (sequence, source) in sources.iter().enumerate() {
                    let prepared = cache
                        .as_deref()
                        .map(|c| sp_catalog::source_cache::prepare(c, source))
                        .transpose()
                        .map_err(|e| e.to_string())?;
                    let path = prepared
                        .as_ref()
                        .map_or(source.as_path(), |p| p.decode_path.as_path());
                    use crate::state::VersionExportMode as Versions;
                    let versions = delivery_versions(
                        path,
                        source,
                        version_mode,
                        &selected_versions,
                        (source == &current).then_some(current_version.as_str()),
                    )?;
                    for (version_id, version_name) in versions {
                        let (edit, metadata) =
                            if source == &current && version_id == current_version {
                                (current_edit.clone(), current_description.clone())
                            } else {
                                saved_version_snapshot(path, &version_id)?
                            };
                        for recipe in &recipes {
                            let mut settings = recipe.settings.clone();
                            if !version_id.is_empty()
                                || matches!(version_mode, Versions::All | Versions::Selected)
                            {
                                settings.filename_pattern.push('-');
                                settings
                                    .filename_pattern
                                    .push_str(&version_suffix(&version_id, &version_name));
                            }
                            if sources.len() > 1 && !settings.filename_pattern.contains("{seq}") {
                                settings.filename_pattern.push_str("-{seq}");
                            }
                            if recipes.len() > 1 {
                                let name: String = recipe
                                    .name
                                    .chars()
                                    .map(|c| if c.is_alphanumeric() { c } else { '-' })
                                    .collect();
                                settings.filename_pattern.push('-');
                                settings.filename_pattern.push_str(&name);
                            }
                            tasks.push(ExportTask {
                                id: 0,
                                source: source.clone(),
                                recipe: format!("{} · {}", recipe.name, version_name),
                                sequence,
                                edit: edit.clone(),
                                metadata: metadata.clone(),
                                settings,
                                status: TaskStatus::Pending,
                                output: None,
                            });
                        }
                    }
                }
                if tasks.is_empty() {
                    return Err("Select at least one version to enqueue".into());
                }
                Ok(tasks)
            })();
            let _ = tx.send(result);
            context.request_repaint();
        });
        self.export_queue_prepare = Some(rx);
        self.state.export_enqueue_busy = true;
    }
}
pub(super) fn queue_window(ctx: &egui::Context, state: &mut AppState) {
    if !state.export_queue_open {
        return;
    }
    let mut open = true;
    egui::Window::new(crate::i18n::text("Export queue")).open(&mut open).default_width(620.0).show(ctx,|ui|{
        if let Some(error)=&state.export_queue_error{
            ui.colored_label(theme::ERROR,error);
            if ui.button(crate::i18n::text("Reload saved queue")).clicked(){state.export_queue_loaded=false;state.export_queue_dirty=false;}
            return;
        }
        ui.horizontal(|ui|{
            if ui.button(if state.export_queue.paused{"Resume queue"}else{"Pause after current photo"}).clicked(){state.export_queue.paused = !state.export_queue.paused;state.export_queue_dirty=true;}
            if ui.button(crate::i18n::text("Clear completed")).clicked(){state.export_queue.tasks.retain(|t|t.status!=TaskStatus::Finished);state.export_queue_dirty=true;}
        });
        ui.label(crate::i18n::text("Each task keeps the edit and metadata from when it was added. Restarted queues wait for Resume."));
        if state.export_enqueue_busy{ui.spinner();}
        let mut retry=None;let mut remove=None;
        egui::ScrollArea::vertical().max_height(420.0).show(ui,|ui|{
            for task in &state.export_queue.tasks{
                ui.separator();ui.horizontal(|ui|{
                    ui.label(format!("{} · {}",task.source.file_name().unwrap_or_default().to_string_lossy(),task.recipe));
                    match &task.status{
                        TaskStatus::Pending=>{ui.label(crate::i18n::text("Waiting"));},TaskStatus::Running=>{ui.spinner();ui.label(format!("{:.0}%",state.export_progress*100.0));},TaskStatus::Finished=>{ui.label(crate::i18n::text("Complete"));},
                        TaskStatus::Failed(error)=>{ui.colored_label(theme::ERROR,crate::i18n::text("Failed")).on_hover_text(error);if ui.button(crate::i18n::text("Retry")).clicked(){retry=Some(task.id);}},
                    }
                    if task.status!=TaskStatus::Running&&ui.small_button(crate::i18n::text("Remove")).clicked(){remove=Some(task.id);}
                });
                if let Some(path)=&task.output{ui.small(path.display().to_string());}
            }
        });
        if let Some(id)=retry{state.export_queue.retry(id);state.export_queue_dirty=true;}
        if let Some(id)=remove{state.export_queue.remove(id);state.export_queue_dirty=true;}
    });
    state.export_queue_open = open;
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn version_selection_captures_independent_immutable_edits_and_safe_names() {
        use crate::state::VersionExportMode as Mode;
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("frame.dng");
        std::fs::write(&source, b"raw").unwrap();
        let primary = sp_catalog::Sidecar::default();
        sp_catalog::save_sidecar(&source, &primary).unwrap();
        let mut edited = primary.clone();
        edited.edit.tone.exposure_ev = 2.0;
        edited.metadata.caption = "Portrait copy".into();
        let id = sp_catalog::workflow::create_virtual_copy(&source, &edited).unwrap();
        sp_catalog::workflow::rename_virtual_copy(&source, &id, "../Warm / portrait").unwrap();
        let empty = std::collections::HashSet::new();
        assert_eq!(
            delivery_versions(&source, &source, Mode::Primary, &empty, Some(&id)).unwrap()[0].0,
            ""
        );
        assert_eq!(
            delivery_versions(&source, &source, Mode::All, &empty, None)
                .unwrap()
                .len(),
            2
        );
        let chosen = std::collections::HashSet::from([(source.clone(), id.clone())]);
        assert_eq!(
            delivery_versions(&source, &source, Mode::Selected, &chosen, None)
                .unwrap()
                .len(),
            1
        );
        let (snapshot, metadata) = saved_version_snapshot(&source, &id).unwrap();
        assert_eq!(snapshot.tone.exposure_ev, 2.0);
        assert_eq!(metadata.caption, "Portrait copy");
        edited.edit.tone.exposure_ev = -1.0;
        sp_catalog::save_sidecar_path(
            &sp_catalog::virtual_copy_sidecar_path(&source, &id),
            &edited,
        )
        .unwrap();
        assert_eq!(snapshot.tone.exposure_ev, 2.0);
        assert_eq!(
            saved_version_snapshot(&source, "")
                .unwrap()
                .0
                .tone
                .exposure_ev,
            0.0
        );
        let suffix = version_suffix(&id, "../Warm / portrait");
        assert!(!suffix.contains('/'));
        assert_ne!(suffix, version_suffix("other", "../Warm / portrait"));
    }
}
