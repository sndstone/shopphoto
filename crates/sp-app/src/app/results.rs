//! Draining background job results once per frame.

use super::*;

impl ShopPhotoApp {
    pub(super) fn poll_background(&mut self, ctx: &egui::Context) {
        // File-dialog result?
        if let Some(rx) = &self.file_pick {
            if let Ok(picked) = rx.try_recv() {
                self.file_pick = None;
                if let Some(path) = picked {
                    self.start_decode(path, ctx);
                }
            }
        }

        // DCP-import dialog result?
        if let Some(rx) = &self.dcp_pick {
            if let Ok(picked) = rx.try_recv() {
                self.dcp_pick = None;
                if let Some(path) = picked {
                    self.finish_dcp_import(&path);
                }
            }
        }
        if let Some(rx) = &self.cube_pick {
            if let Ok(picked) = rx.try_recv() {
                self.cube_pick = None;
                if let Some(path) = picked {
                    self.finish_cube_import(&path);
                }
            }
        }
        if let Some(rx) = &self.icc_pick {
            if let Ok(picked) = rx.try_recv() {
                self.icc_pick = None;
                if let Some(path) = picked {
                    self.finish_icc_import(&path);
                }
            }
        }

        if let Some(rx) = &self.user_preset_import_pick {
            if let Ok(picked) = rx.try_recv() {
                self.user_preset_import_pick = None;
                if let Some(path) = picked {
                    match sp_catalog::workflow::load_user_presets(&path) {
                        Ok(imported) => {
                            let count = imported.len();
                            for preset in imported {
                                self.state
                                    .user_presets
                                    .retain(|candidate| candidate.name != preset.name);
                                self.state.user_presets.push(preset);
                            }
                            self.state
                                .user_presets
                                .sort_by_key(|preset| preset.name.to_lowercase());
                            if let Some(path) = user_presets_path() {
                                let _ = sp_catalog::workflow::save_user_presets(
                                    &path,
                                    &self.state.user_presets,
                                );
                            }
                            self.state.push_toast(
                                crate::i18n::format(
                                    "Imported {0} user preset(s)",
                                    &[count.to_string()],
                                ),
                                false,
                            );
                        }
                        Err(error) => self.state.push_toast(
                            crate::i18n::format(
                                "Could not import presets: {0}",
                                &[error.to_string()],
                            ),
                            true,
                        ),
                    }
                }
            }
        }
        if let Some(rx) = &self.user_preset_export_pick {
            if let Ok(picked) = rx.try_recv() {
                self.user_preset_export_pick = None;
                if let Some(path) = picked {
                    match sp_catalog::workflow::save_user_presets(&path, &self.state.user_presets) {
                        Ok(()) => self.state.push_toast("User presets exported", false),
                        Err(error) => self.state.push_toast(
                            crate::i18n::format(
                                "Could not export presets: {0}",
                                &[error.to_string()],
                            ),
                            true,
                        ),
                    }
                }
            }
        }

        if let Some(rx) = &self.reference_raw_pick {
            if let Ok(paths) = rx.try_recv() {
                self.reference_raw_pick = None;
                if !paths.is_empty() {
                    self.state.reference_raws = paths;
                    self.state.reference_report = None;
                }
            }
        }
        if let Some(rx) = &self.reference_tiff_pick {
            if let Ok(path) = rx.try_recv() {
                self.reference_tiff_pick = None;
                if path.is_some() {
                    self.state.reference_tiff_dir = path;
                    self.state.reference_report = None;
                }
            }
        }
        if let Some(rx) = &self.reference_fit {
            if let Ok(result) = rx.try_recv() {
                self.reference_fit = None;
                self.finish_reference_fit(result);
                ctx.request_repaint();
            }
        }

        if let Some(rx) = &self.export_folder_pick {
            if let Ok(path) = rx.try_recv() {
                self.export_folder_pick = None;
                if let Some(path) = path {
                    self.state.export_settings.destination = path;
                }
            }
        }

        if let Some(rx) = &self.export_watermark_pick {
            if let Ok(path) = rx.try_recv() {
                self.export_watermark_pick = None;
                if let Some(png) = path {
                    self.state.export_settings.watermark = Some(sp_export::Watermark {
                        png,
                        position: sp_export::WatermarkPosition::BottomRight,
                        opacity: 0.8,
                        scale_percent: 20.0,
                    });
                }
            }
        }

        if let Some(rx) = &self.library_folder_pick {
            if let Ok(path) = rx.try_recv() {
                self.library_folder_pick = None;
                if let Some(path) = path {
                    self.refresh_catalog(vec![path], None);
                }
            }
        }
        if let Some(rx) = self.file_move_folder_pick.as_ref().filter(|_| {
            self.catalog_job.is_none() && !self.state.catalog_tool_busy && !self.state.export_busy
        }) {
            if let Ok(path) = rx.try_recv() {
                self.file_move_folder_pick = None;
                if let Some(destination) = path {
                    if !self.save_sidecar_if_due(true) {
                        return;
                    }
                    let sources = self.selected_file_sources();
                    let pattern = self.state.file_rename_pattern.clone();
                    match sp_catalog::workflow::move_photos(&sources, &destination, &pattern) {
                        Ok(batch) => self.adopt_completed_file_moves(batch),
                        Err(error) => self.state.push_toast(error.to_string(), true),
                    }
                }
            }
        }
        if let Some(rx) = &self.import_source_pick {
            if let Ok(path) = rx.try_recv() {
                self.import_source_pick = None;
                if path.is_some() {
                    self.state.import_source = path;
                }
            }
        }
        if let Some(rx) = &self.import_destination_pick {
            if let Ok(path) = rx.try_recv() {
                self.import_destination_pick = None;
                if path.is_some() {
                    self.state.import_destination = path;
                }
            }
        }
        if let Some(rx) = &self.import_backup_pick {
            if let Ok(path) = rx.try_recv() {
                self.import_backup_pick = None;
                if path.is_some() {
                    self.state.import_backup = path;
                }
            }
        }
        let review_events = self
            .import_review_job
            .as_ref()
            .map(|rx| rx.try_iter().collect::<Vec<_>>())
            .unwrap_or_default();
        for event in review_events {
            match event {
                ImportReviewEvent::Photo { entry, preview } => {
                    if let Some(preview) = preview {
                        let image = egui::ColorImage::from_rgba_unmultiplied(
                            [preview.width as usize, preview.height as usize],
                            &preview.rgba8,
                        );
                        self.import_thumbs.insert(
                            entry.path.clone(),
                            ctx.load_texture(
                                format!("import:{}", entry.path.display()),
                                image,
                                egui::TextureOptions::LINEAR,
                            ),
                        );
                    }
                    self.state.import_review.push(entry);
                }
                ImportReviewEvent::Finished(result) => {
                    self.import_review_job = None;
                    self.state.import_review_busy = false;
                    if let Err(error) = result {
                        self.state.push_toast(error, true);
                    }
                }
            }
        }
        if self.state.import_review_busy {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
        let import_events = self
            .import_job
            .as_ref()
            .map(|receiver| receiver.try_iter().collect::<Vec<_>>())
            .unwrap_or_default();
        for event in import_events {
            match event {
                ImportJobEvent::Progress(progress) => {
                    self.state.import_progress = progress;
                    ctx.request_repaint_after(std::time::Duration::from_millis(50));
                }
                ImportJobEvent::Finished(report) => {
                    let count = report.copied.len();
                    let skipped = report.skipped_duplicates;
                    let resumed = report.resumed;
                    self.import_job = None;
                    self.import_cancel = None;
                    self.state.import_busy = false;
                    self.state.import_progress = 1.0;
                    self.state.push_toast(
                        crate::i18n::format(
                            "Imported {0} photo(s) · resumed {1} · skipped {2} duplicate(s)",
                            &[count.to_string(), resumed.to_string(), skipped.to_string()],
                        ),
                        false,
                    );
                    if let Some(destination) = self.state.import_destination.clone() {
                        self.refresh_catalog(vec![destination], None);
                    }
                }
                ImportJobEvent::Failed(error) => {
                    self.import_job = None;
                    self.import_cancel = None;
                    self.state.import_busy = false;
                    let cancelled = error.contains("cancelled");
                    self.state.push_toast(
                        if cancelled {
                            "Import cancelled".into()
                        } else {
                            error
                        },
                        !cancelled,
                    );
                }
            }
        }

        let model_events = self
            .model_install
            .as_ref()
            .map(|receiver| receiver.try_iter().collect::<Vec<_>>())
            .unwrap_or_default();
        for event in model_events {
            match event {
                ModelInstallEvent::Progress(progress) => {
                    self.state.ai_model_install_progress = progress
                }
                ModelInstallEvent::Finished(version) => {
                    self.model_install = None;
                    self.state.ai_model_install_busy = false;
                    self.state.ai_model_install_progress = 1.0;
                    self.state.ai_denoise_available = self
                        .current
                        .as_ref()
                        .is_some_and(|output| output.source.is_raw() && sp_ai::model_installed());
                    // A mode chosen while the model was missing runs now.
                    if self.state.ai_denoise_available {
                        self.ai_requested_mode = sp_core::edit::CameraDenoiseMode::Off;
                    }
                    self.state.push_toast(
                        crate::i18n::format(
                            "Installed neural models · {0}",
                            std::slice::from_ref(&version),
                        ),
                        false,
                    );
                }
                ModelInstallEvent::Failed(error) => {
                    self.model_install = None;
                    self.state.ai_model_install_busy = false;
                    self.state.push_toast(
                        crate::i18n::format(
                            "Model download failed: {0}",
                            std::slice::from_ref(&error),
                        ),
                        true,
                    );
                }
            }
        }
        if let Some(rx) = &self.update_check {
            if let Ok(result) = rx.try_recv() {
                self.update_check = None;
                match result {
                    Ok(Some(info)) => {
                        self.state.update_available = Some((info.latest, info.url, info.notes));
                    }
                    Ok(None) => {}
                    Err(error) => tracing::debug!("update check failed: {error}"),
                }
            }
        }

        let catalog_events = self
            .catalog_job
            .as_ref()
            .map(|rx| rx.try_iter().collect::<Vec<_>>())
            .unwrap_or_default();
        for event in catalog_events {
            match event {
                CatalogJobEvent::Progress(count) => self.state.catalog_scan_count = count,
                CatalogJobEvent::Finished {
                    photos,
                    folders,
                    collections,
                    smart_collections,
                    watchers,
                    fallback_rescan_folders,
                    attempted_folders,
                    folder_errors,
                    removed_folder,
                } => {
                    let catalog_paths: std::collections::HashSet<_> =
                        photos.iter().map(|photo| photo.path.clone()).collect();
                    self.state
                        .selected_photos
                        .retain(|path| catalog_paths.contains(path));
                    self.thumbs.retain(|path, _| {
                        catalog_paths.contains(path) || self.state.session.contains(path)
                    });
                    self.state.catalog_photos = photos;
                    self.state.catalog_folders = folders;
                    self.state.catalog_collections = collections;
                    self.state.catalog_smart_collections = smart_collections;
                    self.state.catalog_scanning = false;
                    self.catalog_job = None;
                    self.folder_watchers = watchers;
                    self.fallback_rescan_folders = fallback_rescan_folders;
                    self.state
                        .catalog_folder_errors
                        .retain(|path, _| !attempted_folders.contains(path));
                    for (path, error) in &folder_errors {
                        self.state
                            .catalog_folder_errors
                            .insert(path.clone(), error.clone());
                    }
                    if !folder_errors.is_empty() {
                        let count = folder_errors.len();
                        self.state.push_toast(
                            {
                                let __l10n_args = (&(if count == 1 { " is" } else { "s are" }),);
                                crate::i18n::format(
                                    "{0} folder{1} unavailable; existing library entries were kept",
                                    &[count.to_string(), __l10n_args.0.to_string()],
                                )
                            },
                            true,
                        );
                    }
                    if removed_folder {
                        self.state
                            .push_toast("Folder removed from the library", false);
                    }
                    self.thumbnail_generation = self.jobs.begin_thumbnail_generation();
                    self.thumbnails_requested.clear();
                    self.state.thumbnail_failures.clear();
                }
                CatalogJobEvent::Failed(error) => {
                    self.state.catalog_scanning = false;
                    self.catalog_job = None;
                    self.state.push_toast(
                        crate::i18n::format(
                            "Catalog scan failed: {0}",
                            std::slice::from_ref(&error),
                        ),
                        true,
                    );
                }
            }
        }

        let export_events = self
            .export_job
            .as_ref()
            .map(|rx| rx.try_iter().collect::<Vec<_>>())
            .unwrap_or_default();
        for event in export_events {
            match event {
                ExportJobEvent::Progress(value) => self.state.export_progress = value,
                ExportJobEvent::Finished { path, count } => {
                    if std::mem::take(&mut self.print_open_pending) {
                        printer::open_pdf(&path);
                    }
                    if let Some(id) = self.export_queue_active.take() {
                        self.state.export_queue.finish(id, Ok(path.clone()));
                        self.state.export_queue_dirty = true;
                    }
                    self.state.export_busy = false;
                    self.state.export_progress = 1.0;
                    self.state.export_dialog_open = false;
                    self.export_job = None;
                    self.export_cancel = None;
                    if let Some(source) = self.external_edit_source.take() {
                        self.begin_external_monitor(source, path);
                    } else {
                        self.state.push_toast(
                            if count == 1 {
                                {
                                    let __l10n_args = (&(path.display()),);
                                    crate::i18n::format(
                                        "Exported {0}",
                                        &[__l10n_args.0.to_string()],
                                    )
                                }
                            } else {
                                crate::i18n::format("Exported {0} photos", &[count.to_string()])
                            },
                            false,
                        );
                    }
                }
                ExportJobEvent::Failed(error) => {
                    self.print_open_pending = false;
                    if let Some(id) = self.export_queue_active.take() {
                        self.state.export_queue.finish(id, Err(error.clone()));
                        self.state.export_queue_dirty = true;
                    }
                    self.state.export_busy = false;
                    self.export_job = None;
                    self.export_cancel = None;
                    self.external_edit_source = None;
                    self.state.push_toast(error, true);
                }
            }
        }

        // Decode results.
        for result in self.jobs.poll() {
            match result {
                JobResult::PreviewReady {
                    path,
                    preview,
                    generation,
                } => {
                    if self.state.open_image.as_ref() == Some(&path)
                        && self.jobs.preview_is_current(generation)
                        && self.state.decode_in_flight
                    {
                        self.preview_tex = Some(ctx.load_texture(
                            "quick-preview",
                            egui::ColorImage::from_rgba_unmultiplied(
                                [preview.width as usize, preview.height as usize],
                                &preview.rgba8,
                            ),
                            egui::TextureOptions::LINEAR,
                        ));
                        ctx.request_repaint();
                    }
                }
                JobResult::Decoded {
                    generation,
                    path,
                    decode_path,
                    network_cached,
                    offline,
                    pinned,
                    xmp_revision,
                    output,
                } => {
                    // Ignore stale results from a superseded open.
                    if self.state.open_image.as_deref() != Some(path.as_path())
                        || !self.jobs.preview_is_current(generation)
                    {
                        continue;
                    }
                    self.state.decode_in_flight = false;
                    self.state.meta = Some(output.meta.clone());
                    self.state.source_is_raw = output.source.is_raw();
                    self.state.source_offline = offline;
                    self.state.source_pinned = pinned;
                    self.state.xmp_revision = xmp_revision
                        .map(|revision| std::sync::Arc::new(std::sync::Mutex::new(revision)));
                    self.ai_source_raw = output.source.as_raw().cloned();
                    self.ai_result_raw = None;
                    self.ai_inferred_raw = None;
                    self.ai_applied_mode = None;
                    self.ai_requested_mode = sp_core::edit::CameraDenoiseMode::Off;
                    self.state.ai_denoise_available =
                        output.source.is_raw() && sp_ai::model_installed();
                    if let Some(db_path) = &self.catalog_path {
                        self.jobs.persist(PersistRequest::CatalogMetadata {
                            catalog: db_path.clone(),
                            path: path.clone(),
                            meta: output.meta.clone(),
                        });
                    }
                    self.refresh_profile_names();
                    self.active_edit_source = Some(decode_path.clone());
                    self.load_sidecar_for(&decode_path);
                    if let Some((requested, id)) = self.state.pending_virtual_copy.take() {
                        if requested == path {
                            self.switch_virtual_copy(id);
                        }
                    }
                    if offline {
                        self.state.push_toast(
                            "Original unavailable · editing a verified offline copy",
                            false,
                        );
                    } else if network_cached {
                        self.state
                            .push_toast("Working from a verified local copy of this photo", false);
                    }
                    self.state.active_size = Some(output.source.dimensions());
                    self.state.reset_view();
                    if !self.state.session.contains(&path) {
                        self.state.session.push(path.clone());
                    }
                    if let Some(p) = &output.preview {
                        self.thumbs
                            .entry(path.clone())
                            .or_insert_with(|| Self::make_thumb(ctx, p));
                    }
                    self.preview_tex = output.preview.as_ref().map(|p| {
                        ctx.load_texture(
                            "embedded-preview",
                            egui::ColorImage::from_rgba_unmultiplied(
                                [p.width as usize, p.height as usize],
                                &p.rgba8,
                            ),
                            egui::TextureOptions::LINEAR,
                        )
                    });
                    if output.preview.is_none() {
                        self.state.push_toast(
                            "File has no embedded preview; rendering directly from sensor data",
                            false,
                        );
                    }
                    self.gpu_open(&output, true, ctx);
                    self.current = Some(output);
                    let paths: Vec<_> = if self.state.catalog_photos.is_empty() {
                        self.state.session.clone()
                    } else {
                        self.state
                            .catalog_photos
                            .iter()
                            .map(|photo| photo.path.clone())
                            .collect()
                    };
                    if let Some(index) = paths.iter().position(|candidate| *candidate == path) {
                        self.jobs.prefetch_neighbors(
                            [
                                index.checked_sub(1).and_then(|i| paths.get(i)),
                                paths.get(index + 1),
                            ]
                            .into_iter()
                            .flatten()
                            .cloned(),
                        );
                    }
                    ctx.request_repaint();
                }
                JobResult::AiDenoised {
                    path,
                    raw,
                    inferred,
                    mode,
                    generation,
                    elapsed,
                    inferred_now,
                } => {
                    if self.state.open_image.as_ref() != Some(&path)
                        || self.ai_generation != Some(generation)
                        || self.state.edit.nr.camera_denoise_mode != mode
                    {
                        continue;
                    }
                    self.state.ai_denoise_busy = false;
                    self.ai_generation = None;
                    self.ai_applied_mode = Some(mode);
                    self.ai_applied_compute = Some(self.ai_requested_compute);
                    self.ai_result_raw = Some(raw.clone());
                    self.ai_inferred_raw = Some(inferred);
                    self.state.ai_compare_original = false;
                    self.state.ai_compare_split = false;
                    if std::env::var_os("SP_COMPARE_SPLIT").is_some() {
                        self.state.ai_compare_split = true;
                        self.state.panels.basic = false;
                        self.state.panels.detail = true;
                    }
                    // QA: show the untouched RAW through the same comparison
                    // renderer, so a before/after screenshot pair isolates
                    // the effect of the neural denoise alone.
                    if std::env::var_os("SP_COMPARE_BEFORE").is_some() {
                        self.state.ai_compare_original = true;
                        self.state.ai_compare_split = false;
                    }
                    if let Some(zoom) = std::env::var_os("SP_TEST_ZOOM")
                        .and_then(|value| value.to_string_lossy().parse::<f32>().ok())
                    {
                        self.state.zoom = Some(zoom.clamp(0.05, 6.0));
                    }
                    if let Some(destination) = std::env::var_os("SP_TEST_EXPORT") {
                        self.state.export_settings.destination = destination.into();
                        self.state.export_settings.filename_pattern = "qa-denoised".into();
                        self.state.export_settings.format = sp_export::ExportFormat::Jpeg {
                            quality: 92,
                            chroma_444: true,
                        };
                        self.state.export_start_requested = true;
                    }
                    self.state.ai_denoise_status = Some(if inferred_now {
                        {
                            let __l10n_args = (&(elapsed.as_secs_f32()),);
                            crate::i18n::format(
                                "AI RAW denoise applied · {0}s",
                                &[format!("{:.1}", __l10n_args.0)],
                            )
                        }
                    } else {
                        "AI RAW denoise applied · re-blended".into()
                    });
                    if let Some(mut output) = self.current.take() {
                        output.source = ImageSource::Raw(raw);
                        self.gpu_open(&output, false, ctx);
                        self.current = Some(output);
                    }
                    if inferred_now {
                        self.state.push_toast(
                            {
                                let __l10n_args = (&(elapsed.as_secs_f32()),);
                                crate::i18n::format(
                                    "AI RAW denoise complete · {0}s",
                                    &[format!("{:.1}", __l10n_args.0)],
                                )
                            },
                            false,
                        );
                    }
                    ctx.request_repaint();
                }
                JobResult::AiDenoiseFailed {
                    path,
                    mode,
                    generation,
                    error,
                } => {
                    if self.state.open_image.as_ref() != Some(&path)
                        || self.ai_generation != Some(generation)
                        || self.state.edit.nr.camera_denoise_mode != mode
                    {
                        continue;
                    }
                    self.state.ai_denoise_busy = false;
                    self.ai_generation = None;
                    self.ai_applied_mode = None;
                    self.ai_result_raw = None;
                    self.state.ai_compare_original = false;
                    self.state.ai_compare_split = false;
                    self.state.ai_denoise_status = Some("AI failed · using GPU fallback".into());
                    if let (Some(source), Some(mut output)) =
                        (self.ai_source_raw.clone(), self.current.take())
                    {
                        output.source = ImageSource::Raw(source);
                        self.gpu_open(&output, false, ctx);
                        self.current = Some(output);
                    }
                    self.state.push_toast(
                        crate::i18n::format(
                            "AI denoise failed; using GPU fallback: {0}",
                            std::slice::from_ref(&error),
                        ),
                        true,
                    );
                    ctx.request_repaint();
                }
                JobResult::CompareDecoded {
                    path,
                    decode_path,
                    output,
                } => {
                    self.on_compare_decoded(&path, decode_path, *output, ctx);
                }
                JobResult::CompareDecodeFailed { path, error } => {
                    self.on_compare_decode_failed(&path, error);
                }
                JobResult::DecodeFailed {
                    path,
                    error,
                    generation,
                } => {
                    if !self.jobs.preview_is_current(generation) {
                        continue;
                    }
                    if self.state.open_image.as_deref() == Some(path.as_path()) {
                        self.state.decode_in_flight = false;
                        self.state.open_image = None;
                        self.active_edit_source = None;
                    }
                    self.state.push_toast(error, true);
                    ctx.request_repaint();
                }
                JobResult::ThumbnailReady {
                    path,
                    preview,
                    meta,
                    generation,
                } => {
                    if generation == self.thumbnail_generation {
                        if let Some(meta) = meta {
                            if let Some(photo) = self
                                .state
                                .catalog_photos
                                .iter_mut()
                                .find(|photo| photo.path == path)
                            {
                                photo.camera_model.clone_from(&meta.camera_model);
                                photo.captured_at.clone_from(&meta.captured_at);
                            }
                            if let Some(db_path) = &self.catalog_path {
                                self.jobs.persist(PersistRequest::CatalogMetadata {
                                    catalog: db_path.clone(),
                                    path: path.clone(),
                                    meta,
                                });
                            }
                        }
                        self.thumbs
                            .entry(path)
                            .or_insert_with(|| Self::make_thumb(ctx, &preview));
                        ctx.request_repaint();
                    }
                }
                JobResult::ThumbnailFailed {
                    path,
                    error,
                    missing,
                    generation,
                } => {
                    if generation == self.thumbnail_generation {
                        tracing::debug!("thumbnail failed for {}: {error}", path.display());
                        self.state
                            .thumbnail_failures
                            .insert(path, crate::state::ThumbnailFailure { error, missing });
                        ctx.request_repaint();
                    }
                }
                JobResult::AutoFinishSaved {
                    path,
                    before,
                    after,
                    preview,
                    error,
                } => {
                    self.state.auto_finish_pending =
                        self.state.auto_finish_pending.saturating_sub(1);
                    if let Some(error) = error {
                        self.state
                            .auto_finish_errors
                            .push(format!("{}: {error}", path.display()));
                    } else {
                        self.state.auto_finish_saved += 1;
                        if self.state.open_image.as_ref() == Some(&path)
                            && self.state.virtual_copy.is_none()
                        {
                            if self.state.edit == before.edit
                                && self.state.annotations == before.annotations
                                && self.state.descriptive_metadata == before.metadata
                                && self.state.snapshots == before.snapshots
                                && self.state.persistent_history == before.history
                            {
                                self.state.apply_edit_step(after.edit.clone());
                                self.state.persistent_history = after.history.clone();
                                // This exact edit is already on disk. A second
                                // save would invalidate its prepared thumbnail.
                                self.state.sidecar_dirty = false;
                            } else {
                                // Keep newer in-memory edits and persist them on the normal lane.
                                self.state.sidecar_dirty = true;
                                self.state.last_edit_change = Some(std::time::Instant::now());
                                self.state.auto_finish_errors.push({
                                    let __l10n_args = (&(path.display()),);
                                    crate::i18n::format(
                                        "{0}: newer manual edit kept",
                                        &[__l10n_args.0.to_string()],
                                    )
                                });
                                continue;
                            }
                        }
                        self.thumbs.insert(path, Self::make_thumb(ctx, &preview));
                        ctx.request_repaint();
                    }
                }
                JobResult::SidecarSyncFailed { path, error } => {
                    tracing::warn!(path = %path.display(), "network sidecar sync failed: {error}");
                    self.state.push_toast(
                        "Edits are safe in the local cache; the network sidecar could not be updated yet",
                        true,
                    );
                }
                JobResult::Annotated {
                    path,
                    annotations,
                    color_label,
                } => {
                    if self.state.open_image.as_ref() == Some(&path) && !self.state.sidecar_dirty {
                        self.state.annotations = annotations;
                        self.state.descriptive_metadata.color_label = color_label;
                    }
                    if let Some(slot) = self.culling_slots.iter_mut().find(|slot| slot.path == path)
                    {
                        slot.annotations = annotations;
                        slot.metadata.color_label = color_label;
                    }
                    if let Some(photo) = self
                        .state
                        .catalog_photos
                        .iter_mut()
                        .find(|photo| photo.path == path)
                    {
                        photo.annotations = annotations;
                        photo.color_label = color_label;
                    }
                }
                JobResult::PersistFailed { path, error, retry } => {
                    if retry && self.state.open_image.as_ref() == Some(&path) {
                        // The .spedit did not land; keep the edit dirty so the
                        // next debounce tick retries.
                        self.state.sidecar_dirty = true;
                        self.state.last_edit_change = Some(std::time::Instant::now());
                        self.state.push_toast(
                            crate::i18n::format(
                                "Could not save edits: {0}",
                                std::slice::from_ref(&error),
                            ),
                            true,
                        );
                    } else {
                        self.state.push_toast(error, true);
                    }
                }
            }
        }

        // While work is pending, keep frames coming so results are seen
        // promptly without a busy repaint loop when idle.
        if self.state.decode_in_flight || self.file_pick.is_some() || self.state.export_busy {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
        // The countdown bars animate, so tick at ~20 fps while any toast
        // counts down; sticky errors alone need no repaints.
        let dt = (self.state.frame_ms / 1000.0).clamp(0.0, 0.5);
        if self.state.prune_toasts(dt) {
            let counting =
                !self.state.toasts_paused && self.state.toasts.iter().any(|toast| !toast.is_error);
            if counting {
                ctx.request_repaint_after(std::time::Duration::from_millis(50));
            }
        }
    }
}
