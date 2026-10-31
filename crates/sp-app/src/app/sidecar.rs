//! Sidecar persistence, virtual copies and opening files.

use super::*;

impl ShopPhotoApp {
    pub(super) fn refresh_virtual_copy_records(&mut self, edit_source: &std::path::Path) {
        let records = sp_catalog::workflow::virtual_copy_records(edit_source).unwrap_or_default();
        self.state.virtual_copies = records.iter().map(|copy| copy.id.clone()).collect();
        self.state.virtual_copy_names = records
            .iter()
            .map(|copy| (copy.id.clone(), copy.name.clone()))
            .collect();
        if let Some(original) = self.state.open_image.as_ref() {
            if let Some(photo) = self
                .state
                .catalog_photos
                .iter_mut()
                .find(|photo| &photo.path == original)
            {
                photo.virtual_copies.clone_from(&records);
            }
            if let Some(catalog) = self.catalog_path.as_ref() {
                if let Err(error) = sp_catalog::CatalogDb::open(catalog)
                    .and_then(|db| db.set_virtual_copies(original, &records))
                {
                    tracing::warn!("could not update catalog versions: {error}");
                }
            }
        }
    }

    pub(super) fn rename_active_virtual_copy(&mut self) {
        if !self.save_sidecar_if_due(true) {
            return;
        }
        let (Some(id), Some(edit_source)) = (
            self.state.virtual_copy.clone(),
            self.active_edit_source.clone(),
        ) else {
            return;
        };
        let name = self.state.virtual_copy_name_draft.trim().to_owned();
        match sp_catalog::workflow::rename_virtual_copy(&edit_source, &id, &name) {
            Ok(()) => {
                self.state.virtual_copy_name = Some(name.clone());
                self.state.virtual_copy_name_draft = name;
                self.state.sidecar_dirty = true;
                self.refresh_virtual_copy_records(&edit_source);
                self.state.push_toast("Version renamed", false);
            }
            Err(error) => self.state.push_toast(error.to_string(), true),
        }
    }

    pub(super) fn trash_active_virtual_copy(&mut self) {
        let (Some(id), Some(edit_source), Some(original)) = (
            self.state.virtual_copy.clone(),
            self.active_edit_source.clone(),
            self.state.open_image.clone(),
        ) else {
            return;
        };
        if self.state.source_offline {
            self.state
                .push_toast("Reconnect the original before removing this version", true);
            return;
        }
        if !self.save_sidecar_if_due(true) {
            return;
        }
        let result = if edit_source != original
            && sp_catalog::virtual_copy_sidecar_path(&original, &id).exists()
        {
            sp_catalog::workflow::trash_virtual_copy(&original, &id)
                .and_then(|()| sp_catalog::workflow::trash_virtual_copy(&edit_source, &id))
        } else {
            sp_catalog::workflow::trash_virtual_copy(&edit_source, &id)
        };
        match result {
            Ok(()) => {
                self.switch_virtual_copy(String::new());
                self.refresh_virtual_copy_records(&edit_source);
                self.state
                    .push_toast("Version moved to Trash; original photo retained", false);
            }
            Err(error) => self.state.push_toast(error.to_string(), true),
        }
    }

    pub(super) fn adopt_sidecar_document(&mut self, sidecar: sp_catalog::Sidecar) {
        self.state.annotations = sidecar.annotations;
        self.state.descriptive_metadata = sidecar.metadata;
        self.state.snapshots = sidecar.snapshots;
        self.state.persistent_history = sidecar.history;
        self.state.virtual_copy = sidecar.virtual_copy;
        self.state.virtual_copy_name = sidecar.virtual_copy_name;
        self.state.virtual_copy_name_draft =
            self.state.virtual_copy_name.clone().unwrap_or_default();
        let mut edit = sidecar.edit;
        let camera_denoise_supported = self.state.source_is_raw && self.state.meta.is_some();
        if !camera_denoise_supported {
            edit.nr.camera_denoise_mode = sp_core::edit::CameraDenoiseMode::Off;
        }
        self.state.adopt_edit(edit);
    }

    /// Load the sidecar (if any) for a newly opened image into the edit
    /// state; corrupt sidecars are quarantined, never deleted.
    pub(super) fn load_sidecar_for(&mut self, path: &std::path::Path) {
        self.refresh_virtual_copy_records(path);
        match sp_catalog::load_sidecar(path) {
            Ok(Some(sidecar)) => {
                self.adopt_sidecar_document(sidecar);
            }
            Ok(None) => {
                self.state.descriptive_metadata = Default::default();
                self.state.snapshots.clear();
                self.state.persistent_history.clear();
                self.state.virtual_copy = None;
                self.state.virtual_copy_name = None;
                // Fresh image: seed noise reduction from ISO (Phase 4).
                let mut edit = EditState::default();
                if sp_raw::is_supported(path) {
                    edit.nr = sp_core::edit::NrParams::seeded_for_iso(
                        self.state.meta.as_ref().and_then(|m| m.iso),
                    );
                }
                self.state.adopt_edit(edit);
                // No sidecar of ours, but another app may have rated this
                // photo already — adopt that rather than showing "unrated".
                match sp_catalog::xmp::load_with_metadata(path) {
                    Ok(Some(document)) => {
                        self.state.annotations = document.annotations.unwrap_or_default();
                        self.state.descriptive_metadata = document.metadata;
                    }
                    Ok(None) => self.state.annotations = Default::default(),
                    Err(error) => {
                        self.state.annotations = Default::default();
                        self.state.push_toast(
                            crate::i18n::format(
                                "Could not read XMP metadata: {0}",
                                &[error.to_string()],
                            ),
                            true,
                        );
                    }
                }
            }
            Err(e) => {
                let message = if matches!(e, sp_core::SpError::CorruptData { .. }) {
                    let sc_path = sp_catalog::sidecar_path(path);
                    let timestamp = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_nanos();
                    // Reserve a unique sibling so earlier quarantines cannot be replaced.
                    let mut sequence = 0u64;
                    let quarantine = loop {
                        let candidate = sc_path
                            .with_extension(format!("spedit.corrupt-{timestamp}-{sequence}"));
                        match std::fs::OpenOptions::new()
                            .write(true)
                            .create_new(true)
                            .open(&candidate)
                        {
                            Ok(_) => break Ok(candidate),
                            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                                sequence += 1
                            }
                            Err(error) => break Err(error),
                        }
                    };
                    let renamed = quarantine.and_then(|quarantine| {
                        std::fs::rename(&sc_path, &quarantine).inspect_err(|_| {
                            let _ = std::fs::remove_file(&quarantine);
                        })
                    });
                    match renamed {
                        Ok(()) => crate::i18n::format(
                            "Edit sidecar was corrupt and set aside ({0})",
                            &[e.to_string()],
                        ),
                        Err(error) => {
                            tracing::warn!(%error, path = %sc_path.display(), "could not quarantine corrupt sidecar");
                            format!("Could not set aside corrupt edit sidecar: {error} ({e})")
                        }
                    }
                } else {
                    tracing::warn!(error = %e, "could not load edit sidecar; file left in place");
                    format!("Could not load edit sidecar: {e}")
                };
                self.state.push_toast(message, true);
                self.state.adopt_edit(EditState::default());
                self.state.annotations = Default::default();
                self.state.descriptive_metadata = Default::default();
                self.state.snapshots.clear();
                self.state.persistent_history.clear();
                self.state.virtual_copy = None;
                self.state.virtual_copy_name = None;
            }
        }
    }

    pub(super) fn create_virtual_copy(&mut self) {
        if self.state.decode_in_flight || !self.save_sidecar_if_due(true) {
            return;
        }
        let Some(original) = self.state.open_image.clone() else {
            self.state.push_toast("Open a photo first", true);
            return;
        };
        let edit_source = self
            .active_edit_source
            .clone()
            .unwrap_or_else(|| original.clone());
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
        if !sp_catalog::sidecar_path(&edit_source).exists() {
            let mut primary = source.clone();
            primary.virtual_copy = None;
            primary.virtual_copy_name = None;
            if let Err(error) = sp_catalog::save_sidecar(&edit_source, &primary) {
                self.state.push_toast(
                    crate::i18n::format(
                        "Could not preserve primary edit: {0}",
                        &[error.to_string()],
                    ),
                    true,
                );
                return;
            }
        }
        match sp_catalog::workflow::create_virtual_copy(&edit_source, &source) {
            Ok(copy_id) => {
                let mut copy = source;
                copy.virtual_copy = Some(copy_id.clone());
                copy.virtual_copy_name = sp_catalog::load_sidecar_path(
                    &sp_catalog::virtual_copy_sidecar_path(&edit_source, &copy_id),
                )
                .ok()
                .flatten()
                .and_then(|copy| copy.virtual_copy_name);
                if edit_source != original {
                    if let Err(error) = sp_catalog::save_sidecar_path(
                        &sp_catalog::virtual_copy_sidecar_path(&original, &copy_id),
                        &copy,
                    ) {
                        self.state.push_toast(
                            crate::i18n::format(
                                "Virtual copy created locally but could not sync: {0}",
                                &[error.to_string()],
                            ),
                            true,
                        );
                    }
                }
                self.state.virtual_copy = Some(copy_id.clone());
                self.state.virtual_copy_name = copy.virtual_copy_name;
                self.state.virtual_copy_name_draft =
                    self.state.virtual_copy_name.clone().unwrap_or_default();
                self.refresh_virtual_copy_records(&edit_source);
                self.state.push_toast(
                    crate::i18n::format("Created virtual copy {0}", std::slice::from_ref(&copy_id)),
                    false,
                );
            }
            Err(error) => self.state.push_toast(
                crate::i18n::format("Could not create virtual copy: {0}", &[error.to_string()]),
                true,
            ),
        }
    }

    pub(super) fn switch_virtual_copy(&mut self, copy_id: String) {
        if !self.save_sidecar_if_due(true) {
            return;
        }
        let Some(edit_source) = self.active_edit_source.clone() else {
            return;
        };
        let loaded = if copy_id.is_empty() {
            sp_catalog::load_sidecar(&edit_source)
        } else {
            sp_catalog::load_sidecar_path(&sp_catalog::virtual_copy_sidecar_path(
                &edit_source,
                &copy_id,
            ))
        };
        match loaded {
            Ok(Some(sidecar)) => {
                self.adopt_sidecar_document(sidecar);
                self.invalidate_profile_render();
                self.state.push_toast(
                    if copy_id.is_empty() {
                        "Switched to primary edit".to_owned()
                    } else {
                        crate::i18n::format("Switched to {0}", std::slice::from_ref(&copy_id))
                    },
                    false,
                );
            }
            Ok(None) => self
                .state
                .push_toast("That edit version no longer exists", true),
            Err(error) => self.state.push_toast(
                crate::i18n::format("Could not load edit version: {0}", &[error.to_string()]),
                true,
            ),
        }
    }

    /// Debounced sidecar save: writes 500 ms after the last edit change.
    /// Queue the debounced sidecar write on the persist worker. `force`
    /// also verifies the latest local save succeeded, which callers need before
    /// reading the sidecar back from disk (image switch, virtual copies,
    /// device loss, exit).
    pub(super) fn save_sidecar_if_due(&mut self, force: bool) -> bool {
        // The selected path already names the incoming photo, while its
        // sidecar has not been loaded. Never save transient loading state.
        if self.state.decode_in_flight {
            return true;
        }
        // `SP_TEST_UI` applies throwaway edits for screenshots; never let
        // them reach the photographer's sidecars.
        if std::env::var_os("SP_TEST_UI").is_some() {
            self.state.sidecar_dirty = false;
            return true;
        }
        if force {
            // Closing or switching can precede the normal per-frame gesture
            // recorder, especially immediately after moving a slider.
            self.state.record_edit_gesture(std::time::Instant::now());
        }
        if self.state.sidecar_dirty {
            let due = force
                || self
                    .state
                    .last_edit_change
                    .is_none_or(|t| t.elapsed().as_millis() > 500);
            if due {
                if let Some(path) = self.state.open_image.clone() {
                    let edit_source = self
                        .active_edit_source
                        .clone()
                        .unwrap_or_else(|| path.clone());
                    let sidecar = sp_catalog::Sidecar {
                        version: sp_core::edit::EDIT_SCHEMA_VERSION,
                        edit: self.state.edit.clone(),
                        annotations: self.state.annotations,
                        metadata: self.state.descriptive_metadata.clone(),
                        snapshots: self.state.snapshots.clone(),
                        history: self.state.persistent_history.clone(),
                        virtual_copy: self.state.virtual_copy.clone(),
                        virtual_copy_name: self.state.virtual_copy_name.clone(),
                    };
                    self.jobs
                        .persist(PersistRequest::Sidecar(Box::new(SidecarWrite {
                            sidecar,
                            path,
                            edit_source,
                            virtual_copy: self.state.virtual_copy.clone(),
                            write_xmp: self.state.write_xmp,
                            xmp_annotations_only: false,
                            xmp_revision: self.state.xmp_revision.clone(),
                            annotations: self.state.annotations,
                            metadata: self.state.descriptive_metadata.clone(),
                            catalog: self.catalog_path.clone(),
                        })));
                    // A failed write comes back as PersistFailed and marks
                    // the edit dirty again.
                    self.state.sidecar_dirty = false;
                }
            }
        }
        if force {
            if let Some(path) = self.state.open_image.as_ref() {
                let edit_source = self.active_edit_source.as_ref().unwrap_or(path);
                if let Err(error) = self.jobs.confirm_sidecar(
                    edit_source,
                    self.state.virtual_copy.as_deref(),
                    std::time::Duration::from_secs(5),
                ) {
                    self.state.sidecar_dirty = true;
                    self.state.last_edit_change = Some(std::time::Instant::now());
                    self.state.push_toast(
                        format!("Could not save edits; the photo remains open: {error}"),
                        true,
                    );
                    return false;
                }
            }
        }
        true
    }

    pub(super) fn open_file_dialog(&mut self) {
        if self.file_pick.is_some() {
            return; // dialog already open
        }
        let (tx, rx) = crossbeam_channel::bounded(1);
        std::thread::Builder::new()
            .name("sp-file-dialog".into())
            .spawn(move || {
                let picked = rfd::FileDialog::new()
                    .add_filter(
                        "Photographs",
                        &sp_raw::SUPPORTED_EXTENSIONS
                            .iter()
                            .chain(sp_core::source::SUPPORTED_RASTER_EXTENSIONS.iter())
                            .copied()
                            .collect::<Vec<_>>(),
                    )
                    .pick_file();
                let _ = tx.send(picked);
            })
            .expect("spawn file dialog thread");
        self.file_pick = Some(rx);
    }

    pub(super) fn start_decode(&mut self, path: PathBuf, ctx: &egui::Context) {
        if !sp_raw::is_supported_image(&path) {
            if !cfg!(feature = "native-codecs")
                && path
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| {
                        matches!(
                            extension.to_ascii_lowercase().as_str(),
                            "avif" | "heif" | "heic"
                        )
                    })
            {
                self.state.push_toast("This build cannot open AVIF/HEIF. Install a build with native codecs, or convert to TIFF/PNG.", true);
                return;
            }
            self.state.push_toast(unsupported_file_message(&path), true);
            return;
        }
        if !self.save_sidecar_if_due(true) {
            return; // Keep the outgoing document available for a retry.
        }
        self.state.spot_ui.refine_request = None;
        self.state.spot_ui.refine_stroke_pending = false;
        self.state.mask_prompts.clear();
        self.state.mask_prompt_pick = None;
        self.state.mask_point_pick = false;
        self.state.begin_decode(path.clone());
        self.current = None;
        // A renderer for the outgoing photo may still be uploading.
        self.gpu_open_generation += 1;
        self.pending_gpu_open = None;
        self.pending_compare = None;
        self.state.spot_ui = Default::default();
        self.state.retouch_active_spot = None;
        self.state.source_offline = false;
        self.state.source_pinned = false;
        self.state.xmp_revision = None;
        self.state.pending_virtual_copy = None;
        // The inspector is still painted while the next RAW decodes. Clear
        // camera-specific state immediately so the next photo can never
        // inherit the previous photo's denoise mode for that interval.
        self.state.meta = None;
        self.state.source_is_raw = sp_raw::is_supported(&path);
        self.state.edit.nr.camera_denoise_mode = sp_core::edit::CameraDenoiseMode::Off;
        self.auto_readback = None;
        self.auto_analysis_edit = None;
        self.auto_retry_count = 0;
        self.state.auto_in_flight = false;
        self.state.active_preset = None;
        self.jobs.cancel_ai_denoise();
        self.ai_source_raw = None;
        self.ai_result_raw = None;
        self.ai_inferred_raw = None;
        self.ai_applied_mode = None;
        self.ai_generation = None;
        self.ai_requested_mode = sp_core::edit::CameraDenoiseMode::Off;
        self.state.ai_denoise_busy = false;
        self.state.ai_denoise_available = false;
        self.state.ai_denoise_status = None;
        self.state.ai_compare_original = false;
        self.state.ai_compare_split = false;
        self.active_edit_source = None;
        self.preview_tex = self.jobs.cached_preview(&path).map(|preview| {
            ctx.load_texture(
                "neighbor-preview",
                egui::ColorImage::from_rgba_unmultiplied(
                    [preview.width as usize, preview.height as usize],
                    &preview.rgba8,
                ),
                egui::TextureOptions::LINEAR,
            )
        });
        self.jobs.request_decode(path);
        ctx.request_repaint();
    }
}

/// "“notes.pdf” can't be opened: .PDF files aren't photos ShopPhoto reads."
/// Quotes the name so an empty or odd name stays visible, and names the
/// extension so the reason is clear without opening the details.
fn unsupported_file_message(path: &std::path::Path) -> String {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    match path.extension().and_then(|e| e.to_str()) {
        Some(ext) if !ext.is_empty() => crate::i18n::format(
            "“{0}” can't be opened: .{1} files are not a supported photo format",
            &[name.to_string(), ext.to_ascii_uppercase()],
        ),
        _ => crate::i18n::format(
            "“{0}” can't be opened: the file has no photo extension",
            &[name.to_string()],
        ),
    }
}
