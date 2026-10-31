//! Library folders, card import and file moves.

use super::*;

impl ShopPhotoApp {
    /// Apply a rating, flag or color label to several Library photos: the
    /// open photo through its live sidecar state, the others by rewriting
    /// their sidecars (and XMP) on the persist worker.
    pub(super) fn annotate_paths(&mut self, change: crate::state::Annotate, paths: &[PathBuf]) {
        self.annotate_changes(Some(change), None, paths);
    }

    pub(super) fn annotate_choices(&mut self, path: &PathBuf, rating: Option<u8>, picked: bool) {
        self.annotate_changes(
            None,
            Some((rating, picked.then_some(sp_catalog::Flag::Pick))),
            std::slice::from_ref(path),
        );
    }

    fn annotate_changes(
        &mut self,
        change: Option<crate::state::Annotate>,
        choices: Option<(Option<u8>, Option<sp_catalog::Flag>)>,
        paths: &[PathBuf],
    ) {
        use crate::state::Annotate;
        if paths.is_empty() {
            return;
        }
        let clear_label = match change {
            Some(Annotate::Label(label)) => paths.iter().all(|path| {
                self.state
                    .catalog_photos
                    .iter()
                    .find(|photo| &photo.path == path)
                    .is_some_and(|photo| photo.color_label == label)
            }),
            _ => false,
        };
        let apply = |annotations: &mut sp_catalog::Annotations,
                     label: &mut sp_catalog::ColorLabel| {
            if let Some((rating, flag)) = choices {
                if let Some(rating) = rating {
                    annotations.rating = rating.min(5);
                }
                if let Some(flag) = flag {
                    annotations.flag = flag;
                }
                return;
            }
            match change {
                Some(Annotate::Rating(rating)) => annotations.rating = rating.min(5),
                Some(Annotate::Flag(flag)) => annotations.flag = flag,
                Some(Annotate::Label(new)) => {
                    *label = if clear_label {
                        sp_catalog::ColorLabel::None
                    } else {
                        new
                    }
                }
                None => {}
            }
        };
        for path in paths {
            let edit_source = if self.state.open_image.as_ref() == Some(path) {
                self.active_edit_source
                    .clone()
                    .unwrap_or_else(|| path.clone())
            } else {
                source_cache_dir()
                    .map(|cache| sp_catalog::source_cache::cached_path(&cache, path))
                    .filter(|cached| cached.exists())
                    .unwrap_or_else(|| path.clone())
            };
            let existing = match sp_catalog::load_sidecar(&edit_source) {
                Ok(sidecar) => sidecar,
                Err(error) => {
                    self.state.push_toast(
                        {
                            let __l10n_args =
                                (&(path.file_name().unwrap_or_default().to_string_lossy()),);
                            crate::i18n::format(
                                "Annotation skipped for {0}: {1}",
                                &[__l10n_args.0.to_string(), error.to_string()],
                            )
                        },
                        true,
                    );
                    continue;
                }
            };
            let foreign = if existing.is_none() {
                match sp_catalog::xmp::load_with_metadata(&edit_source) {
                    Ok(document) => document,
                    Err(error) => {
                        self.state.push_toast(
                            crate::i18n::format("Annotation skipped: {0}", &[error.to_string()]),
                            true,
                        );
                        continue;
                    }
                }
            } else {
                None
            };
            let Some(photo) = self
                .state
                .catalog_photos
                .iter()
                .find(|photo| &photo.path == path)
            else {
                continue;
            };
            let (mut annotations, mut color_label) = (photo.annotations, photo.color_label);
            apply(&mut annotations, &mut color_label);
            if self.state.open_image.as_ref() == Some(path) {
                if let Some(photo) = self
                    .state
                    .catalog_photos
                    .iter_mut()
                    .find(|photo| &photo.path == path)
                {
                    photo.annotations = annotations;
                    photo.color_label = color_label;
                }
                self.state.annotations = annotations;
                self.state.descriptive_metadata.color_label = color_label;
                self.state.sidecar_dirty = true;
                self.state.last_edit_change = Some(std::time::Instant::now());
                continue;
            }
            let before = existing.clone().map(Box::new);
            let mut sidecar = existing.unwrap_or_else(|| sp_catalog::Sidecar {
                version: sp_core::edit::EDIT_SCHEMA_VERSION,
                edit: EditState::default(),
                annotations,
                metadata: foreign
                    .map(|document| document.metadata)
                    .unwrap_or_default(),
                snapshots: Vec::new(),
                history: Vec::new(),
                virtual_copy: None,
                virtual_copy_name: None,
            });
            sidecar.annotations = annotations;
            sidecar.metadata.color_label = color_label;
            let metadata = sidecar.metadata.clone();
            self.jobs.persist(PersistRequest::CheckedSidecar {
                before,
                write: Box::new(SidecarWrite {
                    sidecar,
                    path: path.clone(),
                    edit_source,
                    virtual_copy: None,
                    write_xmp: self.state.write_xmp,
                    xmp_annotations_only: !matches!(change, Some(Annotate::Label(_))),
                    xmp_revision: None,
                    annotations,
                    metadata,
                    catalog: self.catalog_path.clone(),
                }),
            });
        }
    }

    pub(super) fn pick_library_folder(&mut self) {
        if self.library_folder_pick.is_some() {
            return;
        }
        let (tx, rx) = crossbeam_channel::bounded(1);
        std::thread::Builder::new()
            .name("sp-library-folder-dialog".into())
            .spawn(move || {
                let _ = tx.send(rfd::FileDialog::new().pick_folder());
            })
            .expect("spawn library folder dialog");
        self.library_folder_pick = Some(rx);
    }

    pub(super) fn selected_file_sources(&self) -> Vec<(PathBuf, Option<String>)> {
        let mut selected: Vec<_> = self
            .state
            .catalog_photos
            .iter()
            .filter(|photo| self.state.selected_photos.contains(&photo.path))
            .map(|photo| (photo.path.clone(), photo.captured_at.clone()))
            .collect();
        if selected.is_empty() {
            if let Some(path) = &self.state.open_image {
                let captured = self
                    .state
                    .meta
                    .as_ref()
                    .and_then(|meta| meta.captured_at.clone());
                selected.push((path.clone(), captured));
            }
        }
        selected.sort_by(|a, b| a.0.cmp(&b.0));
        selected
    }

    pub(super) fn pick_file_move_folder(&mut self) {
        if self.file_move_folder_pick.is_some() {
            return;
        }
        let (tx, rx) = crossbeam_channel::bounded(1);
        std::thread::Builder::new()
            .name("sp-file-move-dialog".into())
            .spawn(move || {
                let _ = tx.send(rfd::FileDialog::new().pick_folder());
            })
            .expect("spawn file move dialog");
        self.file_move_folder_pick = Some(rx);
    }

    pub(super) fn pick_import_source(&mut self) {
        if self.import_source_pick.is_none() {
            let (tx, rx) = crossbeam_channel::bounded(1);
            std::thread::Builder::new()
                .name("sp-import-source-dialog".into())
                .spawn(move || {
                    let _ = tx.send(rfd::FileDialog::new().pick_folder());
                })
                .expect("spawn import source dialog");
            self.import_source_pick = Some(rx);
        }
    }

    pub(super) fn pick_import_destination(&mut self) {
        if self.import_destination_pick.is_none() {
            let (tx, rx) = crossbeam_channel::bounded(1);
            std::thread::Builder::new()
                .name("sp-import-destination-dialog".into())
                .spawn(move || {
                    let _ = tx.send(rfd::FileDialog::new().pick_folder());
                })
                .expect("spawn import destination dialog");
            self.import_destination_pick = Some(rx);
        }
    }

    pub(super) fn pick_import_backup(&mut self) {
        if self.import_backup_pick.is_none() {
            let (tx, rx) = crossbeam_channel::bounded(1);
            std::thread::Builder::new()
                .name("sp-import-backup-dialog".into())
                .spawn(move || {
                    let _ = tx.send(rfd::FileDialog::new().pick_folder());
                })
                .expect("spawn import backup dialog");
            self.import_backup_pick = Some(rx);
        }
    }

    pub(super) fn start_import_review(&mut self, ctx: &egui::Context) {
        if self.import_review_job.is_some() || self.state.import_busy {
            return;
        }
        let Some(source) = self.state.import_source.clone() else {
            return;
        };
        self.state.import_review.clear();
        self.import_thumbs.clear();
        self.state.import_review_source = Some(source.clone());
        self.state.import_review_busy = true;
        let (tx, rx) = crossbeam_channel::bounded(8);
        std::thread::Builder::new()
            .name("sp-import-review".into())
            .spawn(move || {
                let result = sp_catalog::workflow::import_candidates(&source)
                    .map(|paths| {
                        for path in paths {
                            let (captured_at, preview, warning) =
                                match sp_raw::decode_source_preview(&path) {
                                    Ok(output) => {
                                        let preview = output.preview.and_then(|preview| {
                                            let pixels = image::RgbaImage::from_raw(
                                                preview.width,
                                                preview.height,
                                                preview.rgba8,
                                            )?;
                                            let pixels =
                                                image::imageops::thumbnail(&pixels, 96, 64);
                                            Some(sp_raw::Preview {
                                                width: pixels.width(),
                                                height: pixels.height(),
                                                rgba8: pixels.into_raw(),
                                            })
                                        });
                                        (output.meta.captured_at, preview, None)
                                    }
                                    Err(error) => (None, None, Some(error.to_string())),
                                };
                            let entry = crate::state::ImportReviewEntry {
                                path,
                                selected: true,
                                captured_at,
                                warning,
                            };
                            if tx
                                .send(ImportReviewEvent::Photo { entry, preview })
                                .is_err()
                            {
                                return;
                            }
                        }
                    })
                    .map_err(|error| error.to_string());
                let _ = tx.send(ImportReviewEvent::Finished(result));
            })
            .expect("spawn import review");
        self.import_review_job = Some(rx);
        ctx.request_repaint_after(std::time::Duration::from_millis(50));
    }

    pub(super) fn start_import(&mut self, ctx: &egui::Context) {
        if self.import_job.is_some() {
            return;
        }
        let (Some(source), Some(destination)) = (
            self.state.import_source.clone(),
            self.state.import_destination.clone(),
        ) else {
            self.state
                .push_toast("Choose an import source and destination", true);
            return;
        };
        let preset = self.state.import_preset_name.as_ref().and_then(|name| {
            self.state
                .user_presets
                .iter()
                .find(|preset| &preset.name == name)
                .cloned()
        });
        let reviewed = self.state.import_review_source.as_ref() == Some(&source);
        let mut metadata = self.state.import_metadata.clone();
        metadata.keywords = self
            .state
            .import_keywords_draft
            .split(',')
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_owned)
            .collect();
        let mut options = sp_catalog::workflow::ImportOptions {
            destination,
            backup: self.state.import_backup.clone(),
            filename_pattern: self.state.import_filename_pattern.clone(),
            skip_duplicates: self.state.import_skip_duplicates,
            preset,
            selection: reviewed.then(|| {
                self.state
                    .import_review
                    .iter()
                    .filter(|entry| entry.selected)
                    .map(|entry| entry.path.clone())
                    .collect()
            }),
            captured_at: self
                .state
                .import_review
                .iter()
                .filter_map(|entry| {
                    entry
                        .captured_at
                        .as_ref()
                        .map(|date| (entry.path.clone(), date.clone()))
                })
                .collect(),
            folder_layout: self.state.import_folder_layout,
            metadata,
        };
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let (tx, rx) = crossbeam_channel::unbounded();
        std::thread::Builder::new()
            .name("sp-card-import".into())
            .spawn(move || {
                if !reviewed
                    && (options.folder_layout
                        == sp_catalog::workflow::ImportFolderLayout::CaptureDate
                        || options.filename_pattern.contains("{date}"))
                {
                    if let Ok(paths) = sp_catalog::workflow::import_candidates(&source) {
                        for path in paths {
                            if worker_cancel.load(std::sync::atomic::Ordering::Relaxed) {
                                break;
                            }
                            if let Ok(output) = sp_raw::decode_source_preview(&path) {
                                if let Some(date) = output.meta.captured_at {
                                    options.captured_at.insert(path, date);
                                }
                            }
                        }
                    }
                }
                let result =
                    sp_catalog::workflow::import_card(&source, &options, |completed, total| {
                        let _ = tx.send(ImportJobEvent::Progress(
                            completed as f32 / total.max(1) as f32,
                        ));
                        !worker_cancel.load(std::sync::atomic::Ordering::Relaxed)
                    });
                let _ = match result {
                    Ok(report) => tx.send(ImportJobEvent::Finished(report)),
                    Err(error) => tx.send(ImportJobEvent::Failed(error.to_string())),
                };
            })
            .expect("spawn card import");
        self.import_job = Some(rx);
        self.import_cancel = Some(cancel);
        self.state.import_busy = true;
        self.state.import_progress = 0.0;
        ctx.request_repaint_after(std::time::Duration::from_millis(50));
    }

    pub(super) fn adopt_completed_file_moves(
        &mut self,
        batch: sp_catalog::workflow::FileMoveBatch,
    ) {
        if let Some(path) = &self.catalog_path {
            if let Err(error) =
                sp_catalog::CatalogDb::open(path).and_then(|db| db.apply_file_moves(batch.moves()))
            {
                let rollback = batch.undo();
                self.state.push_toast(
                    {
                        let __l10n_args = (&(rollback.err().map_or_else(String::new, |error| {
                            crate::i18n::format("; file rollback failed: {0}", &[error.to_string()])
                        })),);
                        crate::i18n::format(
                            "Could not update catalog paths: {0}{1}",
                            &[error.to_string(), __l10n_args.0.to_string()],
                        )
                    },
                    true,
                );
                return;
            }
        }
        let moved: std::collections::HashMap<_, _> = batch.moves().iter().cloned().collect();
        self.remap_open_paths(&moved);
        let count = batch.moves().len();
        self.last_file_move_batch = Some(batch);
        self.state.file_move_undo_available = true;
        self.state.push_toast_with_action(
            crate::i18n::format(
                "Moved {0} photo(s) with sidecars and virtual copies",
                &[count.to_string()],
            ),
            false,
            Some(crate::state::ToastAction::UndoFileMove),
        );
        self.refresh_catalog(Vec::new(), None);
    }

    fn remap_open_paths(&mut self, moved: &std::collections::HashMap<PathBuf, PathBuf>) {
        for task in &mut self.state.export_queue.tasks {
            if matches!(
                task.status,
                sp_export::queue::TaskStatus::Pending | sp_export::queue::TaskStatus::Failed(_)
            ) {
                if let Some(destination) = moved.get(&task.source) {
                    task.source = destination.clone();
                    self.state.export_queue_dirty = true;
                }
            }
        }
        self.state.selected_photos = self
            .state
            .selected_photos
            .iter()
            .map(|path| moved.get(path).cloned().unwrap_or_else(|| path.clone()))
            .collect();
        self.state.session = self
            .state
            .session
            .iter()
            .map(|path| moved.get(path).cloned().unwrap_or_else(|| path.clone()))
            .collect();
        self.state.grid_focus = self
            .state
            .grid_focus
            .as_ref()
            .map(|path| moved.get(path).cloned().unwrap_or_else(|| path.clone()));
        if let Some(open) = self.state.open_image.clone() {
            if let Some(destination) = moved.get(&open) {
                self.state.open_image = Some(destination.clone());
                if self.active_edit_source.as_ref() == Some(&open) {
                    self.active_edit_source = Some(destination.clone());
                }
            }
        }
        self.thumbs.clear();
        self.thumbnails_requested.clear();
        self.state.thumbnail_failures.clear();
    }

    pub(super) fn undo_completed_file_moves(&mut self) {
        if !self.save_sidecar_if_due(true) {
            return;
        }
        let Some(batch) = self.last_file_move_batch.take() else {
            return;
        };
        let inverse: Vec<_> = batch
            .moves()
            .iter()
            .map(|(old, new)| (new.clone(), old.clone()))
            .collect();
        if let Err(error) = batch.undo() {
            self.state.push_toast(
                crate::i18n::format("Could not undo move: {0}", &[error.to_string()]),
                true,
            );
            self.last_file_move_batch = Some(batch);
            return;
        }
        let updated = self
            .catalog_path
            .as_ref()
            .map(|path| {
                sp_catalog::CatalogDb::open(path).and_then(|db| db.apply_file_moves(&inverse))
            })
            .transpose();
        if let Err(error) = updated {
            let rollback = batch
                .moves()
                .iter()
                .try_for_each(|(old, new)| sp_catalog::workflow::move_photo(old, new));
            self.state.push_toast(
                {
                    let __l10n_args = (&(rollback.err().map_or_else(String::new, |error| {
                        crate::i18n::format("; rollback failed: {0}", &[error.to_string()])
                    })),);
                    crate::i18n::format(
                        "Could not undo catalog move: {0}{1}",
                        &[error.to_string(), __l10n_args.0.to_string()],
                    )
                },
                true,
            );
            self.last_file_move_batch = Some(batch);
            return;
        }
        self.remap_open_paths(&inverse.into_iter().collect());
        self.state.file_move_undo_available = false;
        self.state.push_toast("File move undone", false);
        self.refresh_catalog(Vec::new(), None);
    }

    pub(super) fn refresh_catalog(
        &mut self,
        scan_folders: Vec<PathBuf>,
        remove_folder: Option<i64>,
    ) {
        if self.catalog_job.is_some() || self.state.catalog_tool_busy {
            return;
        }
        let Some(db_path) = self.catalog_path.clone() else {
            return;
        };
        // Moving old watchers into the worker also keeps their potentially
        // blocking teardown off the UI thread.
        let retired_watchers = std::mem::take(&mut self.folder_watchers);
        let (tx, rx) = crossbeam_channel::unbounded();
        std::thread::Builder::new()
            .name("sp-catalog".into())
            .spawn(move || {
                drop(retired_watchers);
                let result = (|| -> Result<_, String> {
                    let mut db =
                        sp_catalog::CatalogDb::open(&db_path).map_err(|e| e.to_string())?;
                    let removed_folder = remove_folder
                        .map(|folder_id| db.remove_folder(folder_id))
                        .transpose()
                        .map_err(|e| e.to_string())?
                        .unwrap_or(false);
                    let attempted_folders = scan_folders;
                    let mut folder_errors = Vec::new();
                    for folder in &attempted_folders {
                        if let Err(error) = db.scan_folder(folder, |count| {
                            let _ = tx.send(CatalogJobEvent::Progress(count));
                        }) {
                            // One stale SMB mount must not prevent local and
                            // other network folders from loading. The catalog
                            // scan itself preserves that folder's old rows.
                            folder_errors.push((folder.clone(), error.to_string()));
                        }
                    }
                    let photos = db
                        .photos(
                            &sp_catalog::LibraryFilter::default(),
                            sp_catalog::LibrarySort::CaptureTime,
                        )
                        .map_err(|e| e.to_string())?;
                    let folders = db.folders().map_err(|e| e.to_string())?;
                    let collections = db.collections().map_err(|e| e.to_string())?;
                    let smart_collections =
                        db.smart_collections().map_err(|e| e.to_string())?;
                    // Recursive watcher registration can touch every level
                    // of a network mount. Keep it on this worker as part of
                    // the scan instead of stalling the render thread.
                    let mut fallback_rescan_folders = Vec::new();
                    let watchers = folders
                        .iter()
                        .filter_map(|folder| {
                            // Recursive native watcher setup can itself walk
                            // an SMB hierarchy or block on a disconnected
                            // mount. Network shares use the explicit Refresh
                            // action instead.
                            if folder_errors.iter().any(|(path, _)| path == &folder.path)
                                || sp_catalog::source_cache::is_network_path(&folder.path)
                            {
                                return None;
                            }
                            match sp_catalog::FolderWatcher::new(&folder.path) {
                                Ok(watcher) => Some(watcher),
                                Err(error) => {
                                    tracing::warn!(path = %folder.path.display(), "folder watch unavailable: {error}");
                                    if folder.path.try_exists().unwrap_or(false) {
                                        fallback_rescan_folders.push(folder.path.clone());
                                    }
                                    None
                                }
                            }
                        })
                        .collect();
                    Ok((
                        photos,
                        folders,
                        collections,
                        smart_collections,
                        watchers,
                        fallback_rescan_folders,
                        attempted_folders,
                        folder_errors,
                        removed_folder,
                    ))
                })();
                let _ = match result {
                    Ok((
                        photos,
                        folders,
                        collections,
                        smart_collections,
                        watchers,
                        fallback_rescan_folders,
                        attempted_folders,
                        folder_errors,
                        removed_folder,
                    )) => {
                        tx.send(CatalogJobEvent::Finished {
                            photos,
                            folders,
                            collections,
                            smart_collections,
                            watchers,
                            fallback_rescan_folders,
                            attempted_folders,
                            folder_errors,
                            removed_folder,
                        })
                    }
                    Err(error) => tx.send(CatalogJobEvent::Failed(error)),
                };
            })
            .expect("spawn catalog worker");
        self.catalog_job = Some(rx);
        self.state.catalog_scanning = true;
        self.state.catalog_scan_count = 0;
    }
}
