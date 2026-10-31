//! Export dialog pickers and the export worker.

use super::*;

pub(super) fn export_metadata(
    value: &sp_catalog::DescriptiveMetadata,
    annotations: sp_catalog::Annotations,
) -> sp_export::PhotoMetadata {
    sp_export::PhotoMetadata {
        rating: if annotations.flag == sp_catalog::Flag::Reject {
            Some(-1)
        } else {
            (annotations.rating > 0).then_some(annotations.rating.min(5) as i8)
        },
        label: match value.color_label {
            sp_catalog::ColorLabel::None => None,
            sp_catalog::ColorLabel::Red => Some("Red".into()),
            sp_catalog::ColorLabel::Yellow => Some("Yellow".into()),
            sp_catalog::ColorLabel::Green => Some("Green".into()),
            sp_catalog::ColorLabel::Blue => Some("Blue".into()),
            sp_catalog::ColorLabel::Purple => Some("Purple".into()),
        },
        caption: value.caption.clone(),
        keywords: value.keywords.clone(),
        creator: value.creator.clone(),
        copyright: value.copyright.clone(),
        rights: value.rights.clone(),
        contact: value.contact.clone(),
        gps: value.gps,
    }
}

pub(super) fn same_file(left: &std::path::Path, right: &std::path::Path) -> bool {
    left == right
        || std::fs::canonicalize(left)
            .ok()
            .zip(std::fs::canonicalize(right).ok())
            .is_some_and(|(left, right)| left == right)
}

// Exports have no explicit overwrite option. Preserve every existing file,
// including catalog originals outside the current batch and filesystem aliases.
fn available_export_path(destination: &std::path::Path) -> Result<PathBuf, String> {
    let mut candidate = destination.to_owned();
    let stem = destination.file_stem().unwrap_or_default();
    for number in 2u64.. {
        match std::fs::symlink_metadata(&candidate) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(candidate),
            Err(error) => return Err(format!("Cannot check export destination: {error}")),
            Ok(_) => {}
        }
        let mut name = stem.to_os_string();
        name.push(format!(" ({number})"));
        if let Some(extension) = destination.extension() {
            name.push(".");
            name.push(extension);
        }
        candidate = destination.with_file_name(name);
    }
    Err("No available export filename".into())
}

fn publish_export(
    mut staged: tempfile::TempPath,
    destination: &std::path::Path,
) -> Result<PathBuf, String> {
    loop {
        let candidate = available_export_path(destination)?;
        match staged.persist_noclobber(&candidate) {
            Ok(()) => return Ok(candidate),
            Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
                staged = error.path
            }
            Err(error) => return Err(format!("Cannot publish export: {}", error.error)),
        }
    }
}

impl ShopPhotoApp {
    pub(super) fn handle_export_profile(&mut self, ctx: &egui::Context) {
        if let Some(result) = self
            .export_profile_pick
            .as_ref()
            .and_then(|rx| rx.try_recv().ok())
        {
            self.export_profile_pick = None;
            match result {
                Ok(Some(path)) => {
                    self.state.export_settings.output_profile =
                        Some(sp_export::OutputProfile::Custom(path))
                }
                Ok(None) => {}
                Err(error) => self.state.push_toast(error, true),
            }
        }
        if std::mem::take(&mut self.state.export_profile_requested)
            && self.export_profile_pick.is_none()
        {
            let (tx, rx) = crossbeam_channel::bounded(1);
            let context = ctx.clone();
            std::thread::spawn(move || {
                let result = rfd::FileDialog::new()
                    .set_title(crate::i18n::text("RGB output ICC profile"))
                    .add_filter(crate::i18n::text("ICC profile"), &["icc", "icm"])
                    .pick_file()
                    .map(|p| {
                        sp_export::OutputProfile::Custom(p.clone())
                            .icc_bytes()
                            .map(|_| p)
                            .map_err(|e| e.to_string())
                    })
                    .transpose();
                let _ = tx.send(result);
                context.request_repaint();
            });
            self.export_profile_pick = Some(rx);
        }
    }
    pub(super) fn pick_export_folder(&mut self) {
        if self.export_folder_pick.is_some() {
            return;
        }
        let (tx, rx) = crossbeam_channel::bounded(1);
        std::thread::Builder::new()
            .name("sp-export-folder-dialog".into())
            .spawn(move || {
                let _ = tx.send(rfd::FileDialog::new().pick_folder());
            })
            .expect("spawn export folder dialog");
        self.export_folder_pick = Some(rx);
    }

    pub(super) fn pick_export_watermark(&mut self) {
        if self.export_watermark_pick.is_some() {
            return;
        }
        let (tx, rx) = crossbeam_channel::bounded(1);
        std::thread::Builder::new()
            .name("sp-export-watermark-dialog".into())
            .spawn(move || {
                let picked = rfd::FileDialog::new()
                    .add_filter(crate::i18n::text("PNG image"), &["png"])
                    .pick_file();
                let _ = tx.send(picked);
            })
            .expect("spawn export watermark dialog");
        self.export_watermark_pick = Some(rx);
    }

    pub(super) fn start_export(&mut self, ctx: &egui::Context) {
        self.start_export_request(ctx, None);
    }

    pub(super) fn start_export_request(
        &mut self,
        ctx: &egui::Context,
        queued: Option<sp_export::queue::ExportTask>,
    ) {
        if self.export_job.is_some()
            || self.state.export_busy
            || self.state.auto_finish_busy
            || self.state.enhancement_busy
        {
            return;
        }
        if queued.is_none() && self.state.decode_in_flight {
            self.state.push_toast(
                "Wait for the photo to finish loading before exporting",
                false,
            );
            return;
        }
        if self.state.ai_denoise_busy {
            self.state
                .push_toast("Wait for AI RAW denoise to finish before exporting", false);
            return;
        }
        let current = self.current.as_ref();
        if current.is_none() && queued.is_none() {
            self.state.push_toast("Open a photo before exporting", true);
            return;
        }
        let source = self.state.open_image.clone().unwrap_or_default();
        let Some(gpu) = self.gpu.as_ref() else {
            self.state
                .push_toast("A compatible GPU is required for export", true);
            return;
        };
        let print = self.print_request.take();
        let gallery = self.gallery_request.take();
        let current_source = source.clone();
        let current_ai_applied = self.ai_applied_mode
            == Some(self.state.edit.nr.camera_denoise_mode)
            && self.ai_applied_compute == Some(self.state.raw_denoise_compute)
            && self.ai_result_raw.is_some();
        let current_raw = if current_ai_applied {
            Some(sp_core::source::ImageSource::Raw(
                self.ai_result_raw.clone().expect("checked above"),
            ))
        } else {
            self.ai_source_raw
                .clone()
                .map(sp_core::source::ImageSource::Raw)
                .or_else(|| current.map(|c| c.source.clone()))
        };
        let current_meta = current.map(|c| c.meta.clone()).unwrap_or_default();
        let current_edit = self.state.edit.clone();
        let current_description =
            export_metadata(&self.state.descriptive_metadata, self.state.annotations);
        let mut sources = if let Some(task) = &queued {
            vec![task.source.clone()]
        } else if self.state.export_batch {
            self.state
                .selected_photos
                .iter()
                .cloned()
                .collect::<Vec<_>>()
        } else {
            vec![source]
        };
        sources.sort();
        if sources.is_empty() {
            sources.push(current_source.clone());
        }
        let mut settings = queued.as_ref().map_or_else(
            || self.state.export_settings.clone(),
            |task| task.settings.clone(),
        );
        if let Some(request) = &print {
            settings.format = sp_export::ExportFormat::Jpeg {
                quality: 98,
                chroma_444: true,
            };
            settings.color_space = sp_export::ExportColorSpace::Srgb;
            settings.output_profile = None;
            settings.file_size_limit = None;
            let (width, height) = request.layout.image_pixels();
            settings.size = if request.layout.fill {
                sp_export::SizeMode::Original
            } else {
                sp_export::SizeMode::Exact { width, height }
            };
        }
        if settings.output_profile.is_some() {
            settings.color_space = sp_export::ExportColorSpace::Rec2020;
        }
        if gallery.is_some() {
            settings.format = sp_export::ExportFormat::Jpeg {
                quality: 90,
                chroma_444: true,
            };
            settings.color_space = sp_export::ExportColorSpace::Srgb;
            settings.output_profile = None;
            settings.upscale_model = None;
            settings.file_size_limit = None;
            settings.size = sp_export::SizeMode::LongEdgePx(2048);
            settings.strip_gps = true;
            settings.copy_metadata = false;
        }
        if sources.len() > 1 && !settings.filename_pattern.contains("{seq}") {
            settings.filename_pattern.push_str("-{seq}");
        }
        let profiles = self.profiles.clone();
        let custom_looks = self.custom_looks.clone();
        let lenses = self.lenses.clone();
        let network_cache = source_cache_dir();
        let gpu_ctx = GpuContext::new(
            gpu.render_state.device.clone(),
            gpu.render_state.queue.clone(),
        );
        let export_pipes = gpu.renderer.shared_pipelines();
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let compute = self.state.raw_denoise_compute;
        let (tx, rx) = crossbeam_channel::unbounded();
        std::thread::Builder::new()
            .name("sp-export".into())
            .spawn(move || {
                let result = (|| -> Result<(PathBuf, usize), String> {
                    let mut renderer = GpuRenderer::with_pipelines(gpu_ctx, export_pipes)
                        .map_err(|e| e.to_string())?;
                    let total = sources.len();
                    if print.as_ref().is_some_and(|request|sources.iter().any(|source|same_file(&request.destination,source))){return Err("Print destination is an original photo; choose a PDF filename".into());}
                    let mut last_destination = PathBuf::new();
                    let mut proof_gallery=gallery.as_ref().map(|request|sp_export::gallery::Gallery::new(&request.destination,request.title.clone())).transpose()?;
                    let mut document=print.as_ref().map(|request|sp_export::print::PrintDocument::new(&request.destination,request.layout.clone(),sp_export::ExportColorSpace::Srgb)).transpose()?;
                    for (index, source) in sources.iter().enumerate() {
                        if worker_cancel.load(std::sync::atomic::Ordering::Relaxed) {
                            return Err("Export cancelled".into());
                        }
                        let (mut image_source, meta, mut edit, mut description, mut ai_applied) =
                            if queued.is_none() && source == &current_source && current_raw.is_some() {
                                (
                                    current_raw.clone().expect("checked above"),
                                    current_meta.clone(),
                                    current_edit.clone(),
                                    current_description.clone(),
                                    current_ai_applied,
                                )
                            } else {
                                let prepared = network_cache
                                    .as_deref()
                                    .map(|cache| sp_catalog::source_cache::prepare(cache, source))
                                    .transpose()
                                    .map_err(|e| e.to_string())?;
                                let working_source =
                                    prepared.as_ref().map_or(source.as_path(), |prepared| {
                                        prepared.decode_path.as_path()
                                    });
                                let decoded = sp_raw::decode_source(working_source)
                                    .map_err(|e| e.to_string())?;
                                let sidecar = sp_catalog::load_sidecar(working_source)
                                    .map_err(|e| e.to_string())?;
                                let (edit, description) = sidecar.map_or_else(
                                    || {
                                        (
                                            sp_core::EditState::default(),
                                            sp_export::PhotoMetadata::default(),
                                        )
                                    },
                                    |sidecar| (sidecar.edit, export_metadata(&sidecar.metadata, sidecar.annotations)),
                                );
                                (decoded.source, decoded.meta, edit, description, false)
                            };
                        if let Some(task)=&queued { edit=task.edit.clone();description=task.metadata.clone();ai_applied=false; }
                        if !ai_applied
                            && image_source.is_raw()
                            && edit.nr.camera_denoise_mode != sp_core::edit::CameraDenoiseMode::Off
                        {
                            let root = sp_ai::model_root()
                                .ok_or("Selected AI denoise model is unavailable")?;
                            let raw = image_source.as_raw().ok_or("RAW source unavailable")?;
                            let inferred = crate::inference_cache::infer_with_compute(raw, &root, compute)
                                .map_err(|e| e.to_string())?;
                            image_source = sp_core::source::ImageSource::Raw(std::sync::Arc::new(
                                sp_ai::blend(
                                    raw,
                                    &inferred,
                                    edit.nr.camera_denoise_mode.strength(meta.iso),
                                ),
                            ));
                            ai_applied = true;
                        }
                        let profile = match &edit.profile {
                            sp_core::edit::ProfileChoice::Named(name) => profiles
                                .iter()
                                .find(|profile| {
                                    &profile.name == name
                                        && camera_matches(&profile.camera_model, &meta.camera_model)
                                })
                                .cloned(),
                            sp_core::edit::ProfileChoice::Standard => None,
                        };
                        let custom_look = edit.look.as_ref().and_then(|name| {
                            custom_looks
                                .iter()
                                .find(|(candidate, _)| candidate == name)
                                .cloned()
                        });
                        // Shading is baked into the source by `open_image`,
                        // so it must be resolved first — an export that
                        // skipped this would differ from the preview.
                        let lens = resolve_lens(&lenses, &edit.lens, &meta);
                        renderer.set_lens_profile(lens, meta.aperture_f);
                        renderer.set_capture_metadata(Some(&meta));
                        renderer.set_external_raw_denoise(ai_applied);
                        renderer
                            .open_source_with_edit(&image_source, &edit)
                            .map_err(|e| e.to_string())?;
                        renderer.set_profile(profile);
                        renderer.set_custom_look(custom_look);
                        // Ultra HDR is the one format that needs BOTH
                        // renditions: a baseline SDR JPEG plus a gain map
                        // derived from the PQ render of the same edit.
                        let ultra_hdr =
                            matches!(settings.format, sp_export::ExportFormat::UltraHdr { .. });
                        let hdr_pq = edit.hdr.enabled
                            && edit.hdr.target == sp_core::edit::HdrTarget::Hdr
                            && matches!(
                                settings.format,
                                sp_export::ExportFormat::Avif { .. }
                                    | sp_export::ExportFormat::Png { sixteen_bit: true }
                                    | sp_export::ExportFormat::JpegXl { .. }
                            );
                        let high_precision = settings.format.supports_sixteen_bit()
                            || hdr_pq
                            || settings.output_profile.is_some()
                            || settings.upscale_model.is_some();
                        let gpu_output_space = if hdr_pq {
                            sp_export::ExportColorSpace::Rec2020
                        } else {
                            settings.color_space
                        };
                        let rendered = renderer
                            .render_export(
                                &edit,
                                hdr_pq,
                                high_precision,
                                gpu_output_space,
                                &worker_cancel,
                                |value| {
                                    let overall =
                                        (index as f32 + value * 0.8) / total.max(1) as f32;
                                    let _ = tx.send(ExportJobEvent::Progress(overall));
                                },
                            )
                            .map_err(|e| e.to_string())?;
                        let image = sp_export::ExportImage::from_buffers(
                            rendered.width, rendered.height, rendered.rgba8, rendered.rgba16)
                            .ok_or_else(|| "GPU returned an invalid image buffer".to_owned())?;
                        let mut encode_settings = settings.clone();
                        encode_settings.photo_metadata = description;
                        if hdr_pq {
                            encode_settings.color_space = sp_export::ExportColorSpace::Rec2020;
                        }
                        let prepared = enhance::prepare_export(
                            image,
                            edit.geometry,
                            &encode_settings,
                            hdr_pq,
                            &worker_cancel,
                        )?;
                        if let Some(document)=&mut document {
                            document.push(&prepared,&source.file_name().unwrap_or_default().to_string_lossy())?;
                            let _=tx.send(ExportJobEvent::Progress((index+1)as f32/total.max(1)as f32));
                            continue;
                        }
                        if let Some(proof_gallery)=&mut proof_gallery{
                            proof_gallery.push(&prepared,source,if gallery.as_ref().is_some_and(|request|request.caption){&encode_settings.photo_metadata.caption}else{""})?;
                            let _=tx.send(ExportJobEvent::Progress((index+1)as f32/total.max(1)as f32));continue;
                        }
                        last_destination =
                            sp_export::output_path(source, &encode_settings, Some(&meta), queued.as_ref().map_or(index,|task|task.sequence));
                        if same_file(&last_destination,source) {
                            return Err("Export destination is the original photo; choose a different name or folder".into());
                        }
                        last_destination = available_export_path(&last_destination)?;
                        let parent = last_destination.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or_else(|| std::path::Path::new("."));
                        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                        let staged = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?.into_temp_path();
                        if ultra_hdr {
                            // Second pass over the same edit, PQ-encoded, so
                            // the gain map can describe the headroom the SDR
                            // rendition rolled off.
                            let hdr_rendered = renderer
                                .render_export(
                                    &edit,
                                    true,
                                    true,
                                    sp_export::ExportColorSpace::Rec2020,
                                    &worker_cancel,
                                    |value| {
                                        let overall = (index as f32 + 0.5 + value * 0.4)
                                            / total.max(1) as f32;
                                        let _ = tx.send(ExportJobEvent::Progress(overall));
                                    },
                                )
                                .map_err(|e| e.to_string())?;
                            let hdr_image = sp_export::ExportImage::from_buffers(
                                hdr_rendered.width, hdr_rendered.height, hdr_rendered.rgba8, hdr_rendered.rgba16)
                                .ok_or_else(|| "GPU returned an invalid HDR image buffer".to_owned())?;
                            let mut hdr_settings = encode_settings.clone();
                            hdr_settings.color_space = sp_export::ExportColorSpace::Rec2020;
                            let hdr_prepared = enhance::prepare_export(
                                hdr_image,
                                edit.geometry,
                                &hdr_settings,
                                true,
                                &worker_cancel,
                            )?;
                            sp_export::encode_ultra_hdr(
                                &prepared,
                                &hdr_prepared,
                                &encode_settings,
                                &staged,
                                Some(&meta),
                            )
                            .map_err(|e| e.to_string())?;
                        } else {
                            sp_export::encode_file_with_metadata(
                                &prepared,
                                &encode_settings,
                                &staged,
                                hdr_pq,
                                Some(&meta),
                            )
                            .map_err(|e| e.to_string())?;
                        }
                        last_destination = publish_export(staged, &last_destination)?;
                        let _ = tx.send(ExportJobEvent::Progress(
                            (index + 1) as f32 / total.max(1) as f32,
                        ));
                    }
                    if worker_cancel.load(std::sync::atomic::Ordering::Relaxed){return Err("Export cancelled".into());}
                    if let Some(document)=document{last_destination=document.finish()?;}
                    if let Some(proof_gallery)=proof_gallery{last_destination=proof_gallery.finish(&gallery.as_ref().expect("gallery request").receipts)?;}
                    Ok((last_destination, total))
                })();
                let _ = match result {
                    Ok((path, count)) => tx.send(ExportJobEvent::Finished { path, count }),
                    Err(error) => tx.send(ExportJobEvent::Failed(error)),
                };
            })
            .expect("spawn export worker");
        self.export_job = Some(rx);
        self.export_cancel = Some(cancel);
        self.state.export_busy = true;
        self.state.export_progress = 0.0;
        ctx.request_repaint_after(std::time::Duration::from_millis(50));
    }
}

#[cfg(test)]
mod destination_tests {
    use super::*;

    #[test]
    fn export_collision_preserves_other_originals_and_previous_exports() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("B.png");
        std::fs::write(&destination, b"original B").unwrap();
        std::fs::write(directory.path().join("B (2).png"), b"previous export").unwrap();
        assert_eq!(
            available_export_path(&destination).unwrap(),
            directory.path().join("B (3).png")
        );
        assert_eq!(std::fs::read(&destination).unwrap(), b"original B");
        assert_eq!(
            available_export_path(&directory.path().join("new.png")).unwrap(),
            directory.path().join("new.png")
        );
    }

    #[test]
    fn publication_does_not_overwrite_a_file_created_after_path_selection() {
        let directory = tempfile::tempdir().unwrap();
        let destination = available_export_path(&directory.path().join("photo.png")).unwrap();
        let staged = tempfile::NamedTempFile::new_in(directory.path())
            .unwrap()
            .into_temp_path();
        std::fs::write(&staged, b"export").unwrap();
        std::fs::write(&destination, b"new original").unwrap();
        let published = publish_export(staged, &destination).unwrap();
        assert_eq!(published, directory.path().join("photo (2).png"));
        assert_eq!(std::fs::read(destination).unwrap(), b"new original");
        assert_eq!(std::fs::read(published).unwrap(), b"export");
    }

    #[cfg(unix)]
    #[test]
    fn export_collision_preserves_symlinks_to_sources() {
        let directory = tempfile::tempdir().unwrap();
        let original = directory.path().join("original.png");
        std::fs::write(&original, b"original").unwrap();
        let destination = directory.path().join("alias.png");
        std::os::unix::fs::symlink(&original, &destination).unwrap();
        assert_eq!(
            available_export_path(&destination).unwrap(),
            directory.path().join("alias (2).png")
        );
        assert_eq!(std::fs::read(original).unwrap(), b"original");
    }
}
