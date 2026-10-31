//! Catalog maintenance and offline copies. Disk work stays off the UI thread.

use super::*;

#[derive(Debug, Clone, Copy)]
pub(crate) enum Action {
    Backup,
    Restore,
    CheckIntegrity,
    RelinkFolder(i64),
    PinSelected,
    UnpinSelected,
    ReloadXmp,
    SaveImportMetadata,
    LoadImportMetadata,
    StackSelected,
    DissolveStack(i64),
    SetStackCover(i64),
    MoveCollection(i64, Option<i64>),
    DeleteCollection(i64),
    PackageSelected,
    OpenProject,
    AnalyzeSelected,
    AnalyzeFacesSelected,
    PreviewAutoStacks,
    ApplyAutoStacks,
}

pub(super) enum Event {
    Progress(String),
    Finished(Result<Box<Outcome>, String>),
}

pub(super) struct Outcome {
    message: String,
    refresh: bool,
    relink: Option<(PathBuf, PathBuf)>,
    pinned: Option<(Vec<PathBuf>, bool)>,
    metadata: Option<(PathBuf, sp_catalog::xmp::XmpDocument)>,
    import_metadata: Option<sp_catalog::DescriptiveMetadata>,
    restored: bool,
    project_paths: Option<Vec<PathBuf>>,
    quality: Vec<(PathBuf, sp_catalog::quality::PreviewQuality)>,
    auto_stacks: Option<Vec<Vec<PathBuf>>>,
}

impl Outcome {
    fn message(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            refresh: false,
            relink: None,
            pinned: None,
            metadata: None,
            import_metadata: None,
            restored: false,
            project_paths: None,
            quality: Vec::new(),
            auto_stacks: None,
        }
    }
}

pub(super) fn auto_stack_window(ctx: &egui::Context, state: &mut AppState) {
    if !state.auto_stack_open {
        return;
    }
    egui::Window::new(crate::i18n::text("Review automatic photo stacks")).open(&mut state.auto_stack_open).resizable(true).default_width(580.0).show(ctx,|ui|{
        ui.label(crate::i18n::text("Uses selected catalog photos, or the whole catalog when nothing is selected. Existing stacks are excluded."));
        ui.add_enabled_ui(!state.catalog_tool_busy,|ui| {
            let old=state.auto_stack_mode;
            ui.horizontal(|ui| {
                ui.selectable_value(&mut state.auto_stack_mode,sp_catalog::workflow::AutoStackMode::CaptureTime,crate::i18n::text("Capture-time groups"));
                ui.selectable_value(&mut state.auto_stack_mode,sp_catalog::workflow::AutoStackMode::RawJpeg,crate::i18n::text("RAW + JPEG pairs"));
            });
            if old!=state.auto_stack_mode {state.auto_stack_suggestions.clear();}
            if state.auto_stack_mode==sp_catalog::workflow::AutoStackMode::CaptureTime {
                if ui.add(egui::Slider::new(&mut state.auto_stack_span_s,1..=60).text(crate::i18n::text("Maximum group span (seconds)"))).changed(){state.auto_stack_suggestions.clear();}
                ui.label(egui::RichText::new(crate::i18n::text("Matching folder, camera, lens and capture time. Review burst, bracket or focus sequences yourself; filenames and timestamps do not prove shooting intent.")).small().color(theme::TEXT_SECONDARY));
            }else{ui.label(crate::i18n::text("Same folder and filename stem, exactly one RAW plus its JPEG file(s). Originals stay separate."));}
            if ui.button(crate::i18n::text("Find proposed stacks")).clicked(){state.catalog_tool_requested=Some(Action::PreviewAutoStacks);}
            egui::ScrollArea::vertical().max_height(320.0).show(ui,|ui| {
                for (selected,paths) in &mut state.auto_stack_suggestions {
                    ui.checkbox(selected,{ let __l10n_args = (&(paths.len()),); crate::i18n::format("{0} photographs", &[__l10n_args.0.to_string()]) }).on_hover_text(paths.iter().map(|path|path.display().to_string()).collect::<Vec<_>>().join("\n"));
                    ui.label(egui::RichText::new(paths.iter().map(|path|path.file_name().unwrap_or_default().to_string_lossy()).collect::<Vec<_>>().join(" · ")).small());
                    ui.separator();
                }
            });
            if ui.add_enabled(state.auto_stack_suggestions.iter().any(|(selected,_)|*selected),egui::Button::new(crate::i18n::text("Create checked stacks"))).clicked(){state.catalog_tool_requested=Some(Action::ApplyAutoStacks);}
        });
        if state.catalog_tool_busy{ui.spinner();ui.label(crate::i18n::text(&state.catalog_tool_status));}
    });
}

impl ShopPhotoApp {
    pub(super) fn catalog_tools_upkeep(&mut self, ctx: &egui::Context) {
        let events = self
            .catalog_tool_job
            .as_ref()
            .map(|rx| rx.try_iter().collect::<Vec<_>>())
            .unwrap_or_default();
        for event in events {
            match event {
                Event::Progress(message) => self.state.catalog_tool_status = message,
                Event::Finished(result) => {
                    self.catalog_tool_job = None;
                    self.state.catalog_tool_busy = false;
                    self.state.catalog_tool_status.clear();
                    match result {
                        Ok(outcome) => {
                            if let Some(stacks) = outcome.auto_stacks {
                                self.state.auto_stack_suggestions =
                                    stacks.into_iter().map(|group| (true, group)).collect();
                            }
                            for (path, quality) in &outcome.quality {
                                if let Some(slot) = self
                                    .culling_slots
                                    .iter_mut()
                                    .find(|slot| slot.path == *path)
                                {
                                    slot.quality = Some(*quality);
                                }
                            }
                            self.state.preview_quality.extend(outcome.quality);
                            if let Some(paths) = outcome.project_paths {
                                self.state.selected_photos = paths.iter().cloned().collect();
                                self.state.session = paths;
                                self.state.library_collection = None;
                                self.state.mode = Mode::Library;
                            }
                            if outcome.restored {
                                self.folder_watchers.clear();
                                self.last_file_move_batch = None;
                                self.state.file_move_undo_available = false;
                                self.state.selected_photos.clear();
                                self.state.library_collection = None;
                            }
                            if let Some(metadata) = outcome.import_metadata {
                                self.state.import_keywords_draft = metadata.keywords.join(", ");
                                self.state.import_metadata = metadata;
                            }
                            if let Some((old, new)) = outcome.relink {
                                let remap = |path: &PathBuf| {
                                    sp_catalog::workflow::normalized_source_path(path)
                                        .strip_prefix(&old)
                                        .map_or_else(
                                            |_| path.clone(),
                                            |relative| new.join(relative),
                                        )
                                };
                                self.state.selected_photos =
                                    self.state.selected_photos.iter().map(remap).collect();
                                for task in &mut self.state.export_queue.tasks {
                                    if matches!(
                                        task.status,
                                        sp_export::queue::TaskStatus::Pending
                                            | sp_export::queue::TaskStatus::Failed(_)
                                    ) {
                                        let next = remap(&task.source);
                                        if next != task.source {
                                            task.source = next;
                                            self.state.export_queue_dirty = true;
                                        }
                                    }
                                }
                                self.state.session = self.state.session.iter().map(remap).collect();
                                if let Some(path) = self.state.open_image.clone() {
                                    let next = remap(&path);
                                    if next != path {
                                        self.state.open_image = Some(next.clone());
                                        if self.active_edit_source.as_ref() == Some(&path) {
                                            self.active_edit_source = Some(next);
                                        }
                                    }
                                }
                            }
                            if let Some((paths, pinned)) = outcome.pinned {
                                if self
                                    .state
                                    .open_image
                                    .as_ref()
                                    .is_some_and(|path| paths.contains(path))
                                {
                                    self.state.source_pinned = pinned;
                                    if pinned {
                                        if let (Some(cache), Some(path)) =
                                            (source_cache_dir(), self.state.open_image.as_ref())
                                        {
                                            self.active_edit_source = Some(
                                                sp_catalog::source_cache::cached_path(&cache, path),
                                            );
                                            // Edits made while the copy was preparing must land in
                                            // the new authoritative cache, not the earlier seed.
                                            self.state.sidecar_dirty = true;
                                        }
                                    }
                                }
                            }
                            if let Some((path, document)) = outcome.metadata {
                                if self.state.open_image.as_ref() == Some(&path) {
                                    if let Some(annotations) = document.annotations {
                                        self.state.annotations = annotations;
                                    }
                                    self.state.descriptive_metadata = document.metadata;
                                    self.state.xmp_revision = Some(std::sync::Arc::new(
                                        std::sync::Mutex::new(document.revision),
                                    ));
                                    self.state.sidecar_dirty = true;
                                }
                            }
                            if !outcome.message.is_empty() {
                                self.state.push_toast(outcome.message, false);
                            }
                            if outcome.refresh {
                                self.refresh_catalog(Vec::new(), None);
                            }
                        }
                        Err(error) => self.state.push_toast(error, true),
                    }
                }
            }
        }
        if self.catalog_tool_job.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
            return;
        }
        if self.catalog_job.is_some() {
            return;
        }
        let Some(action) = self.state.catalog_tool_requested.take() else {
            return;
        };
        if self.state.export_busy && matches!(action, Action::RelinkFolder(_) | Action::Restore) {
            self.state.push_toast(
                "Wait for the active export before changing catalog source paths",
                true,
            );
            return;
        }
        let write_xmp = self.state.write_xmp;
        if matches!(action, Action::ReloadXmp) {
            self.state.write_xmp = false;
        }
        let saved = self.save_sidecar_if_due(true);
        self.state.write_xmp = write_xmp;
        if !saved {
            return;
        }
        if matches!(action, Action::Restore | Action::PackageSelected)
            && !self.jobs.flush_persist(std::time::Duration::from_secs(5))
        {
            self.state.push_toast(
                "Pending edits have not finished saving; retry catalog restore shortly",
                true,
            );
            return;
        }
        let catalog = self.catalog_path.clone();
        let cache = source_cache_dir();
        let open = self.state.open_image.clone();
        let focus = self.state.grid_focus.clone();
        let auto_mode = self.state.auto_stack_mode;
        let auto_selected_only = !self.state.selected_photos.is_empty();
        let auto_span = self.state.auto_stack_span_s.clamp(1, 60);
        let auto_groups: Vec<_> = self
            .state
            .auto_stack_suggestions
            .iter()
            .filter(|(selected, _)| *selected)
            .map(|(_, paths)| paths.clone())
            .collect();
        let mut import_metadata = self.state.import_metadata.clone();
        import_metadata.keywords = self
            .state
            .import_keywords_draft
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .collect();
        let mut paths: Vec<_> = self.state.selected_photos.iter().cloned().collect();
        if paths.is_empty() {
            paths.extend(open.clone());
        }
        paths.sort();
        let old_folder = if let Action::RelinkFolder(id) = action {
            self.state
                .catalog_folders
                .iter()
                .find(|folder| folder.id == id)
                .map(|folder| folder.path.clone())
        } else {
            None
        };
        let (tx, rx) = crossbeam_channel::unbounded();
        let task = std::thread::Builder::new().name("sp-catalog-tools".into()).spawn(move || {
            let result = (|| -> Result<Outcome, String> {
                match action {
                    Action::AnalyzeSelected | Action::AnalyzeFacesSelected => {
                        let faces=matches!(action,Action::AnalyzeFacesSelected);
                        let mut quality=Vec::new();let mut failed=0;let mut face_failures=0;let mut face_error=None;
                        for (index,path) in paths.iter().enumerate() {
                            let _=tx.send(Event::Progress({ let __l10n_args = (&(index+1), &(paths.len()),); crate::i18n::format("Analyzing source previews · {0} of {1}", &[__l10n_args.0.to_string(), __l10n_args.1.to_string()]) }));
                            let local=cache.as_ref().and_then(|cache|sp_catalog::source_cache::prepare(cache,path).ok()).map_or_else(||path.clone(),|prepared|prepared.decode_path);
                            let decoded=sp_raw::decode_source_preview(&local).ok().and_then(|output|output.preview).and_then(|preview|sp_catalog::quality::analyze(preview.width,preview.height,&preview.rgba8).ok().map(|mut quality| {
                                if faces {match analyze_face_preview(preview.width,preview.height,&preview.rgba8){Ok(summary)=>quality.faces=Some(summary),Err(error)=>{face_failures+=1;face_error=Some(error);},}}
                                quality
                            }));
                            if let Some(value)=decoded {quality.push((path.clone(),value));}else{failed+=1;}
                        }
                        let message=if faces {{ let __l10n_args = (&(quality.len()), &(face_error.unwrap_or_default()),); crate::i18n::format("Analyzed {0} preview(s), {1} unavailable; {2} face checks unavailable. Face/eye detail is advisory; inspect at 100%. {3}", &[__l10n_args.0.to_string(), failed.to_string(), face_failures.to_string(), __l10n_args.1.to_string()]) }}else{{ let __l10n_args = (&(quality.len()),); crate::i18n::format("Analyzed {0} preview(s), {1} unavailable. Detail and similarity are advisory; inspect focus at 100%.", &[__l10n_args.0.to_string(), failed.to_string()]) }};
                        let mut outcome=Outcome::message(message);outcome.quality=quality;Ok(outcome)
                    }
                    Action::SaveImportMetadata => {
                        let Some(path) = rfd::FileDialog::new().set_title(crate::i18n::text("Save import metadata template")).set_file_name("Photography-metadata.json").add_filter(crate::i18n::text("Metadata template"), &["json"]).save_file() else { return Ok(Outcome::message("")); };
                        sp_catalog::workflow::save_metadata_template(&path, &import_metadata).map_err(|e| e.to_string())?;
                        Ok(Outcome::message("Metadata template saved"))
                    }
                    Action::LoadImportMetadata => {
                        let Some(path) = rfd::FileDialog::new().set_title(crate::i18n::text("Load import metadata template")).add_filter(crate::i18n::text("Metadata template"), &["json"]).pick_file() else { return Ok(Outcome::message("")); };
                        let template = sp_catalog::workflow::load_metadata_template(&path).map_err(|e| e.to_string())?;
                        let mut outcome = Outcome::message({ let __l10n_args = (&(template.name),); crate::i18n::format("Loaded metadata template {0}", std::slice::from_ref(__l10n_args.0)) });
                        outcome.import_metadata = Some(template.metadata);
                        Ok(outcome)
                    }
                    Action::PinSelected | Action::UnpinSelected => {
                        let cache = cache.ok_or_else(|| "Local cache directory unavailable".to_owned())?;
                        if paths.is_empty() { return Err("Select a photo first".into()); }
                        let pin = matches!(action, Action::PinSelected);
                        for (index, path) in paths.iter().enumerate() {
                            let _ = tx.send(Event::Progress({ let __l10n_args = (&(if pin { "Preparing" } else { "Releasing" }), &(index + 1), &(paths.len()),); crate::i18n::format("{0} offline copies · {1} of {2}", &[__l10n_args.0.to_string(), __l10n_args.1.to_string(), __l10n_args.2.to_string()]) }));
                            if pin { sp_catalog::source_cache::pin(&cache, path).map_err(|e| e.to_string())?; }
                            else { sp_catalog::source_cache::unpin(&cache, path).map_err(|e| e.to_string())?; }
                        }
                        let mut outcome = Outcome::message({ let __l10n_args = (&(paths.len()), &(if pin { "available offline" } else { "released from offline storage" }),); crate::i18n::format("{0} photo(s) {1}", &[__l10n_args.0.to_string(), __l10n_args.1.to_string()]) });
                        outcome.pinned = Some((paths, pin));
                        Ok(outcome)
                    }
                    Action::ReloadXmp => {
                        let path = open.ok_or_else(|| "Open a photo first".to_owned())?;
                        let document = sp_catalog::xmp::load_with_metadata(&path).map_err(|e| e.to_string())?.ok_or_else(|| "No XMP metadata file was found".to_owned())?;
                        let mut outcome = Outcome::message("Metadata reloaded from XMP; photo adjustments are unchanged");
                        outcome.metadata = Some((path, document));
                        Ok(outcome)
                    }
                    action => {
                        let catalog = catalog.ok_or_else(|| "Catalog storage unavailable".to_owned())?;
                        let mut db = sp_catalog::CatalogDb::open(&catalog).map_err(|e| e.to_string())?;
                        match action {
                            Action::PreviewAutoStacks=>{
                                let mut photos=db.photos(&sp_catalog::LibraryFilter::default(),sp_catalog::LibrarySort::Name).map_err(|e|e.to_string())?;
                                if auto_selected_only {photos.retain(|photo|paths.iter().any(|path|sp_catalog::workflow::normalized_source_path(path)==sp_catalog::workflow::normalized_source_path(&photo.path)));}
                                if auto_mode==sp_catalog::workflow::AutoStackMode::CaptureTime {
                                    for photo in &mut photos {
                                        if photo.captured_at.is_none() {
                                            let _=tx.send(Event::Progress({ let __l10n_args = (&(photo.path.file_name().unwrap_or_default().to_string_lossy()),); crate::i18n::format("Reading capture metadata for {0}", &[__l10n_args.0.to_string()]) }));
                                            if let Ok(decoded)=sp_raw::decode_source_preview(&photo.path){photo.captured_at=decoded.meta.captured_at.clone();photo.camera_model=decoded.meta.camera_model.clone();photo.lens_model=decoded.meta.lens_model.clone().unwrap_or_default();let _=db.set_metadata(&photo.path,&decoded.meta);}
                                        }
                                    }
                                }
                                let groups=sp_catalog::workflow::propose_stacks(&photos,auto_mode,auto_span);
                                let mut outcome=Outcome::message({ let __l10n_args = (&(groups.len()),); crate::i18n::format("Found {0} proposed stack(s); review before applying", &[__l10n_args.0.to_string()]) });outcome.auto_stacks=Some(groups);Ok(outcome)
                            }
                            Action::ApplyAutoStacks=>{
                                let mut created=Vec::new();
                                for paths in &auto_groups {
                                    match db.stack_photos(paths) {Ok(id)=>created.push(id),Err(error)=>{for id in created{let _=db.dissolve_stack(id);}return Err(error.to_string());}}
                                }
                                let mut outcome=Outcome::message({ let __l10n_args = (&(created.len()),); crate::i18n::format("Created {0} photo stack(s)", &[__l10n_args.0.to_string()]) });outcome.auto_stacks=Some(Vec::new());outcome.refresh=true;Ok(outcome)
                            }
                            Action::PackageSelected => {
                                let Some(parent)=rfd::FileDialog::new().set_title(crate::i18n::text("Choose a folder for the portable photo project")).pick_folder() else {return Ok(Outcome::message(""));};
                                let stamp=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs();
                                let destination=parent.join(format!("ShopPhoto-Project-{stamp}"));
                                let sources=paths.iter().map(|path| {
                                    let local=if let Some(cache)=&cache {sp_catalog::source_cache::prepare(cache,path).map_err(|e|e.to_string())?.decode_path} else {path.clone()};
                                    Ok((path.clone(),local))
                                }).collect::<Result<Vec<_>,String>>()?;
                                let manifest=sp_catalog::project::create(&db,&sources,&destination,|done,total|{let _=tx.send(Event::Progress(crate::i18n::format("Packaging verified originals · {0} of {1}", &[done.to_string(), total.to_string()])));}).map_err(|e|e.to_string())?;
                                Ok(Outcome::message({ let __l10n_args = (&(manifest.display()),); crate::i18n::format("Portable project saved to {0}", &[__l10n_args.0.to_string()]) }))
                            }
                            Action::OpenProject => {
                                let Some(manifest)=rfd::FileDialog::new().set_title(crate::i18n::text("Open portable photo project")).add_filter(crate::i18n::text("ShopPhoto project"), &["spproject"]).pick_file() else {return Ok(Outcome::message(""));};
                                let paths=sp_catalog::project::open(&mut db,&manifest).map_err(|e|e.to_string())?;
                                let mut outcome=Outcome::message({ let __l10n_args = (&(paths.len()),); crate::i18n::format("Opened {0} project photo(s); originals verified", &[__l10n_args.0.to_string()]) });
                                outcome.project_paths=Some(paths);outcome.refresh=true;Ok(outcome)
                            }
                            Action::MoveCollection(id,parent) => {
                                db.move_collection(id,parent).map_err(|e|e.to_string())?;
                                let mut outcome = Outcome::message("Collection moved"); outcome.refresh = true; Ok(outcome)
                            }
                            Action::DeleteCollection(id) => {
                                db.delete_collection(id).map_err(|e|e.to_string())?;
                                let mut outcome = Outcome::message("Collection removed; its photos remain in the library"); outcome.refresh = true; Ok(outcome)
                            }
                            Action::StackSelected => {
                                db.stack_photos(&paths).map_err(|e| e.to_string())?;
                                let mut outcome = Outcome::message({ let __l10n_args = (&(paths.len()),); crate::i18n::format("Grouped {0} photos into a stack", &[__l10n_args.0.to_string()]) });
                                outcome.refresh = true;
                                Ok(outcome)
                            }
                            Action::DissolveStack(id) => {
                                db.dissolve_stack(id).map_err(|e|e.to_string())?;
                                let mut outcome = Outcome::message("Stack removed; all photos retained");
                                outcome.refresh = true;
                                Ok(outcome)
                            }
                            Action::SetStackCover(id) => {
                                let photo = focus.ok_or_else(||"Select a stack photo first".to_owned())?;
                                db.set_stack_cover(id,&photo).map_err(|e|e.to_string())?;
                                let mut outcome = Outcome::message("Stack cover updated");
                                outcome.refresh = true;
                                Ok(outcome)
                            }
                            Action::Backup => {
                                let Some(destination) = rfd::FileDialog::new().set_title(crate::i18n::text("Save catalog backup")).set_file_name("ShopPhoto-catalog-backup.sqlite").add_filter(crate::i18n::text("SQLite catalog"), &["sqlite"]).save_file() else { return Ok(Outcome::message("")); };
                                db.backup_to(&destination).map_err(|e| e.to_string())?;
                                Ok(Outcome::message({ let __l10n_args = (&(destination.display()),); crate::i18n::format("Catalog backup saved to {0} · originals and sidecars remain in their folders", &[__l10n_args.0.to_string()]) }))
                            }
                            Action::Restore => {
                                let Some(source) = rfd::FileDialog::new().set_title(crate::i18n::text("Restore catalog from backup — current catalog will be preserved")).add_filter(crate::i18n::text("SQLite catalog"), &["sqlite", "db"]).pick_file() else { return Ok(Outcome::message("")); };
                                let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
                                let rescue = catalog.with_file_name(format!("catalog-before-restore-{stamp}.sqlite"));
                                db.restore_from(&source, &rescue).map_err(|e| e.to_string())?;
                                let mut outcome = Outcome::message({ let __l10n_args = (&(rescue.display()),); crate::i18n::format("Catalog restored; previous catalog saved at {0}. Originals and sidecars are retained.", &[__l10n_args.0.to_string()]) });
                                outcome.restored = true;
                                outcome.refresh = true;
                                Ok(outcome)
                            }
                            Action::CheckIntegrity => {
                                db.integrity_check().map_err(|e| e.to_string())?;
                                Ok(Outcome::message("Catalog integrity and photo relationships checked successfully"))
                            }
                            Action::RelinkFolder(id) => {
                                let Some(destination) = rfd::FileDialog::new().set_title(crate::i18n::text("Locate the moved photo folder")).pick_folder() else { return Ok(Outcome::message("")); };
                                let destination = sp_catalog::workflow::normalized_source_path(&destination);
                                let count = db.relink_folder(id, &destination).map_err(|e| e.to_string())?;
                                let mut outcome = Outcome::message(crate::i18n::format("Relinked {0} photo(s), preserving collections and companion links", &[count.to_string()]));
                                outcome.refresh = true;
                                outcome.relink = old_folder.map(|old| (old, destination));
                                Ok(outcome)
                            }
                            _ => Err("Unsupported catalog action".into()),
                        }
                    }
                }
            })();
            let _ = tx.send(Event::Finished(result.map(Box::new)));
        });
        match task {
            Ok(_) => {
                self.catalog_tool_job = Some(rx);
                self.state.catalog_tool_busy = true;
                self.state.catalog_tool_status = "Working on catalog…".into();
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
            }
            Err(error) => self.state.push_toast(
                crate::i18n::format("Could not start catalog task: {0}", &[error.to_string()]),
                true,
            ),
        }
    }
}

fn analyze_face_preview(
    width: u32,
    height: u32,
    rgba: &[u8],
) -> Result<sp_catalog::quality::FaceAnalysis, String> {
    let directory = tempfile::tempdir().map_err(|e| e.to_string())?;
    let input = directory.path().join("preview.png");
    let output = directory.path().join("faces.json");
    image::save_buffer(&input, rgba, width, height, image::ColorType::Rgba8)
        .map_err(|e| e.to_string())?;
    super::enhance_worker::run(
        directory.path(),
        serde_json::json!({"operation":"analyze_faces","input":input,"output":output}),
        &std::sync::atomic::AtomicBool::new(false),
    )?;
    let summary: sp_catalog::quality::FaceAnalysis =
        serde_json::from_slice(&std::fs::read(output).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    if !summary.valid() {
        return Err("Invalid face analysis result".into());
    }
    Ok(summary)
}
