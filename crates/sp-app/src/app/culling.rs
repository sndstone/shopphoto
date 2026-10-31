//! Side-by-side culling (UX-01): two to four photos, each developed by its
//! own worker-built renderer, with locked pan and zoom and 1:1 tiles when
//! zoomed. Pick, reject and rating keys act on the focused cell and are
//! persisted to that photo's sidecar and catalog row.

use super::*;
use crate::state::CullingState;
use sp_pipeline::renderer::TileInfo;
use std::path::Path;

pub(super) const MAX_CULL_SLOTS: usize = 4;

pub(super) struct CullSlot {
    pub(super) path: PathBuf,
    /// Where the sidecar lives (the local cache copy for network sources).
    pub(super) edit_source: PathBuf,
    pub(super) edit: EditState,
    pub(super) active_size: Option<(u32, u32)>,
    pub(super) preview: Option<egui::TextureHandle>,
    pub(super) pending: Option<Receiver<Result<Box<GpuRenderer>, String>>>,
    pub(super) renderer: Option<GpuRenderer>,
    pub(super) tex_id: Option<egui::TextureId>,
    pub(super) out_size: egui::Vec2,
    pub(super) tile_id: Option<egui::TextureId>,
    pub(super) tile_info: Option<TileInfo>,
    pub(super) annotations: sp_catalog::Annotations,
    pub(super) metadata: sp_catalog::DescriptiveMetadata,
    pub(super) error: Option<String>,
    pub(super) quality: Option<sp_catalog::quality::PreviewQuality>,
}

/// What the viewport draws for one cell.
pub struct CullCell {
    pub name: String,
    pub hdr_proxy: bool,
    pub hdr_tile: bool,
    pub proxy: Option<(egui::TextureId, egui::Vec2)>,
    pub tile: Option<(egui::TextureId, TileInfo)>,
    pub preview: Option<(egui::TextureId, egui::Vec2)>,
    pub active: Option<(u32, u32)>,
    pub annotations: sp_catalog::Annotations,
    pub error: Option<String>,
    pub quality: Option<sp_catalog::quality::PreviewQuality>,
}

impl ShopPhotoApp {
    /// Order the selection the way the user sees it: catalog order first,
    /// then session order, then by path.
    fn ordered_selection(&self) -> Vec<PathBuf> {
        let mut paths: Vec<PathBuf> = self.state.selected_photos.iter().cloned().collect();
        let position = |path: &PathBuf| {
            self.state
                .catalog_photos
                .iter()
                .position(|photo| &photo.path == path)
                .or_else(|| {
                    self.state
                        .session
                        .iter()
                        .position(|p| p == path)
                        .map(|p| p + 1_000_000)
                })
                .unwrap_or(usize::MAX)
        };
        paths.sort_by(|a, b| position(a).cmp(&position(b)).then_with(|| a.cmp(b)));
        paths
    }

    pub(super) fn start_culling(&mut self, ctx: &egui::Context) {
        let paths = self.ordered_selection();
        if paths.len() < 2 {
            self.state.push_toast(
                "Select two to four photos (⌘-click in the filmstrip or grid) to compare",
                false,
            );
            return;
        }
        let paths: Vec<PathBuf> = paths.into_iter().take(MAX_CULL_SLOTS).collect();
        self.stop_culling();
        self.culling_slots = paths
            .iter()
            .map(|path| CullSlot {
                path: path.clone(),
                edit_source: path.clone(),
                edit: EditState::default(),
                active_size: None,
                preview: None,
                quality: None,
                pending: None,
                renderer: None,
                tex_id: None,
                out_size: egui::Vec2::ZERO,
                tile_id: None,
                tile_info: None,
                annotations: self
                    .state
                    .catalog_photos
                    .iter()
                    .find(|photo| &photo.path == path)
                    .map(|photo| photo.annotations)
                    .unwrap_or_default(),
                metadata: sp_catalog::DescriptiveMetadata::default(),
                error: None,
            })
            .collect();
        for path in &paths {
            self.jobs.request_compare_decode(path.clone());
        }
        self.state.culling = Some(CullingState::new(paths));
        self.state.mode = Mode::Develop;
        self.state.zoom = None;
        self.state.view_center = (0.5, 0.5);
        ctx.request_repaint();
    }

    pub(super) fn stop_culling(&mut self) {
        if self.culling_slots.is_empty() {
            self.state.culling = None;
            return;
        }
        if let Some(gpu) = &self.gpu {
            let mut er = gpu.render_state.renderer.write();
            for (index, slot) in self.culling_slots.iter_mut().enumerate() {
                if let Some(pack) = er
                    .callback_resources
                    .get_mut::<crate::hdr_view::HdrBlitPack>()
                {
                    pack.unbind(crate::hdr_view::HdrSource::CullProxy(index as u8));
                    pack.unbind(crate::hdr_view::HdrSource::CullTile(index as u8));
                }
                for id in [slot.tex_id.take(), slot.tile_id.take()]
                    .into_iter()
                    .flatten()
                {
                    er.free_texture(&id);
                }
            }
        }
        // Dropping the renderers frees their VRAM.
        self.culling_slots.clear();
        self.state.culling = None;
    }

    /// A decode for a culling cell landed: show the embedded preview at once
    /// and build the developed renderer on a worker.
    pub(super) fn on_compare_decoded(
        &mut self,
        path: &Path,
        decode_path: PathBuf,
        output: DecodeOutput,
        ctx: &egui::Context,
    ) {
        let Some(index) = self.culling_slots.iter().position(|slot| slot.path == path) else {
            return;
        };
        let sidecar = sp_catalog::load_sidecar(&decode_path).ok().flatten();
        let edit = sidecar.as_ref().map(|s| s.edit.clone()).unwrap_or_default();
        let setup = self.culling_setup(edit.clone(), &output.meta, ctx);
        let rx = setup
            .and_then(|setup| self.spawn_renderer_build(output.source.clone(), setup, false, ctx));
        let slot = &mut self.culling_slots[index];
        slot.edit_source = decode_path;
        slot.edit = edit;
        slot.active_size = Some(output.source.dimensions());
        if let Some(sidecar) = &sidecar {
            slot.annotations = sidecar.annotations;
            slot.metadata = sidecar.metadata.clone();
        }
        slot.quality = self.state.preview_quality.get(path).copied().or_else(|| {
            output.preview.as_ref().and_then(|preview| {
                sp_catalog::quality::analyze(preview.width, preview.height, &preview.rgba8).ok()
            })
        });
        slot.preview = output.preview.as_ref().map(|p| {
            ctx.load_texture(
                "cull-preview",
                egui::ColorImage::from_rgba_unmultiplied(
                    [p.width as usize, p.height as usize],
                    &p.rgba8,
                ),
                egui::TextureOptions::LINEAR,
            )
        });
        match rx {
            Some(rx) => slot.pending = Some(rx),
            None => slot.error = Some("GPU unavailable".into()),
        }
        ctx.request_repaint();
    }

    pub(super) fn on_compare_decode_failed(&mut self, path: &Path, error: String) {
        if let Some(slot) = self.culling_slots.iter_mut().find(|slot| slot.path == path) {
            slot.error = Some(error);
        }
    }

    /// Renderer settings for a cell: float HDR on an available HDR surface,
    /// otherwise SDR, proxy sized for the viewport, that photo's own edit
    /// and lens profile.
    fn culling_setup(
        &self,
        edit: EditState,
        meta: &sp_core::ImageMeta,
        egui_ctx: &egui::Context,
    ) -> Option<RendererSetup> {
        self.gpu.as_ref()?;
        let lens =
            resolve_lens(&self.lenses, &edit.lens, meta).map(|p| (p.clone(), meta.aperture_f));
        let viewport_edge = self.state.viewport_size.0.max(self.state.viewport_size.1);
        let proxy_long_edge = if viewport_edge >= 1.0 {
            (viewport_edge * egui_ctx.pixels_per_point()).ceil() as u32
        } else {
            2048
        };
        let view = ViewKey {
            proof_space: self.state.proof_space,
            custom_proof_generation: self.state.custom_proof_generation,
            gamut_warning: false,
            clipping_mode: 0,
            sharpen_mask_preview: false,
            mask_preview: None,
            analysis: false,
        };
        let profile = match &edit.profile {
            sp_core::edit::ProfileChoice::Standard => None,
            sp_core::edit::ProfileChoice::Named(name) => self
                .profiles
                .iter()
                .find(|p| &p.name == name && camera_matches(&p.camera_model, &meta.camera_model))
                .cloned(),
        };
        Some(RendererSetup {
            hdr_display: self.gpu.as_ref().is_some_and(|gpu| gpu.hdr_surface),
            proxy_long_edge,
            meta: Some(meta.clone()),
            external_raw_denoise: false,
            lens,
            profile,
            custom_look: self.custom_look_for(&edit),
            custom_proof: self.custom_proof_for_view(),
            view,
            edit,
        })
    }

    /// Adopt finished renderers and keep 1:1 tiles current for the regions
    /// the viewport reported last frame.
    pub(super) fn sync_culling(&mut self, ctx: &egui::Context) {
        let Some(culling) = self.state.culling.clone() else {
            return;
        };
        let Some(gpu) = &self.gpu else {
            return;
        };
        let device = gpu.render_state.device.clone();
        let renderer_lock = gpu.render_state.renderer.clone();
        let zoomed = self.state.zoom.is_some();
        for (index, slot) in self.culling_slots.iter_mut().enumerate() {
            if let Some(rx) = &slot.pending {
                match rx.try_recv() {
                    Ok(Ok(renderer)) => {
                        slot.pending = None;
                        if let (Some((_, w, h)), Some(view)) =
                            (renderer.output(), renderer.output_view())
                        {
                            let mut er = renderer_lock.write();
                            if let Some(old) = slot.tex_id.take() {
                                er.free_texture(&old);
                            }
                            slot.tex_id = Some(er.register_native_texture(
                                &device,
                                view,
                                wgpu::FilterMode::Linear,
                            ));
                            slot.out_size = egui::vec2(w as f32, h as f32);
                            if gpu.hdr_surface {
                                if let (Some(view), Some(pack)) = (
                                    renderer.hdr_display_view(),
                                    er.callback_resources
                                        .get_mut::<crate::hdr_view::HdrBlitPack>(),
                                ) {
                                    pack.bind(
                                        &device,
                                        crate::hdr_view::HdrSource::CullProxy(index as u8),
                                        view,
                                        None,
                                    );
                                }
                            }
                        }
                        slot.renderer = Some(*renderer);
                        ctx.request_repaint();
                    }
                    Ok(Err(error)) => {
                        slot.pending = None;
                        slot.error = Some(error);
                    }
                    Err(crossbeam_channel::TryRecvError::Empty) => {}
                    Err(crossbeam_channel::TryRecvError::Disconnected) => {
                        slot.pending = None;
                        slot.error = Some("upload worker exited".into());
                    }
                }
            }
            let region = culling.regions.get(index).copied().flatten();
            let Some(renderer) = &mut slot.renderer else {
                continue;
            };
            let Some(active) = slot.active_size else {
                continue;
            };
            match (zoomed, region) {
                (true, Some(region)) => {
                    let covered = slot.tile_info.is_some_and(|info| {
                        info.x <= region.0
                            && info.y <= region.1
                            && info.x + info.w >= region.0 + region.2
                            && info.y + info.h >= region.1 + region.3
                    });
                    if covered {
                        continue;
                    }
                    let (x, y, w, h) = overscanned_tile_region(region, active);
                    if let Some(info) = renderer.render_tile(&slot.edit, x, y, w, h) {
                        if let Some(view) = renderer.tile_view() {
                            let mut er = renderer_lock.write();
                            if let Some(old) = slot.tile_id.take() {
                                er.free_texture(&old);
                            }
                            slot.tile_id = Some(er.register_native_texture(
                                &device,
                                view,
                                wgpu::FilterMode::Nearest,
                            ));
                        }
                        if gpu.hdr_surface {
                            let mut er = renderer_lock.write();
                            if let (Some(view), Some(pack)) = (
                                renderer.tile_hdr_display_view(),
                                er.callback_resources
                                    .get_mut::<crate::hdr_view::HdrBlitPack>(),
                            ) {
                                pack.bind(
                                    &device,
                                    crate::hdr_view::HdrSource::CullTile(index as u8),
                                    view,
                                    None,
                                );
                            }
                        }
                        slot.tile_info = Some(info);
                        ctx.request_repaint();
                    }
                }
                _ => {
                    if let Some(old) = slot.tile_id.take() {
                        renderer_lock.write().free_texture(&old);
                    }
                    if let Some(pack) = renderer_lock
                        .write()
                        .callback_resources
                        .get_mut::<crate::hdr_view::HdrBlitPack>()
                    {
                        pack.unbind(crate::hdr_view::HdrSource::CullTile(index as u8));
                    }
                    slot.tile_info = None;
                }
            }
        }
    }

    pub(super) fn culling_cells(&self) -> Vec<CullCell> {
        self.culling_slots
            .iter()
            .map(|slot| CullCell {
                name: slot
                    .path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                hdr_proxy: self.gpu.as_ref().is_some_and(|gpu| gpu.hdr_surface)
                    && slot
                        .renderer
                        .as_ref()
                        .is_some_and(|r| r.hdr_display_view().is_some()),
                hdr_tile: self.gpu.as_ref().is_some_and(|gpu| gpu.hdr_surface)
                    && slot
                        .renderer
                        .as_ref()
                        .is_some_and(|r| r.tile_hdr_display_view().is_some()),
                proxy: slot.tex_id.map(|id| (id, slot.out_size)),
                tile: slot.tile_id.zip(slot.tile_info),
                preview: slot.preview.as_ref().map(|t| (t.id(), t.size_vec2())),
                active: slot.active_size,
                annotations: slot.annotations,
                error: slot.error.clone(),
                quality: slot.quality,
            })
            .collect()
    }

    /// Change the focused cell's flag or rating and persist it: the open
    /// image goes through the normal dirty-sidecar path, any other photo
    /// gets its sidecar and catalog row written by the persist worker.
    pub(super) fn annotate_culling_focus(
        &mut self,
        change: impl FnOnce(&mut sp_catalog::Annotations),
    ) {
        let Some(focused) = self.state.culling.as_ref().map(|c| c.focused) else {
            return;
        };
        let Some(slot) = self.culling_slots.get_mut(focused) else {
            return;
        };
        let existing = match sp_catalog::load_sidecar(&slot.edit_source) {
            Ok(sidecar) => sidecar,
            Err(error) => {
                self.state.push_toast(
                    crate::i18n::format("Annotation skipped: {0}", &[error.to_string()]),
                    true,
                );
                return;
            }
        };
        let foreign = if existing.is_none() {
            match sp_catalog::xmp::load_with_metadata(&slot.edit_source) {
                Ok(document) => document,
                Err(error) => {
                    self.state.push_toast(
                        crate::i18n::format("Annotation skipped: {0}", &[error.to_string()]),
                        true,
                    );
                    return;
                }
            }
        } else {
            None
        };
        let mut annotations = slot.annotations;
        change(&mut annotations);
        let path = slot.path.clone();
        let edit_source = slot.edit_source.clone();
        if self.state.open_image.as_ref() == Some(&path) {
            slot.annotations = annotations;
            if let Some(photo) = self
                .state
                .catalog_photos
                .iter_mut()
                .find(|photo| photo.path == path)
            {
                photo.annotations = annotations;
            }
            self.state.annotations = annotations;
            self.state.sidecar_dirty = true;
            self.state.last_edit_change = Some(std::time::Instant::now());
            return;
        }
        let before = existing.clone().map(Box::new);
        let metadata = existing
            .as_ref()
            .map(|s| s.metadata.clone())
            .unwrap_or_else(|| {
                foreign.map_or_else(|| slot.metadata.clone(), |document| document.metadata)
            });
        let sidecar = match existing {
            Some(mut sidecar) => {
                sidecar.annotations = annotations;
                sidecar
            }
            None => sp_catalog::Sidecar {
                version: sp_core::edit::EDIT_SCHEMA_VERSION,
                edit: slot.edit.clone(),
                annotations,
                metadata: metadata.clone(),
                snapshots: Vec::new(),
                history: Vec::new(),
                virtual_copy: None,
                virtual_copy_name: None,
            },
        };
        self.jobs.persist(PersistRequest::CheckedSidecar {
            before,
            write: Box::new(SidecarWrite {
                sidecar,
                path,
                edit_source,
                virtual_copy: None,
                write_xmp: self.state.write_xmp,
                xmp_annotations_only: true,
                xmp_revision: None,
                annotations,
                metadata,
                catalog: self.catalog_path.clone(),
            }),
        });
    }

    /// Per-frame requests from the top bar, viewport and shortcuts.
    pub(super) fn culling_upkeep(&mut self, ctx: &egui::Context) {
        // QA hook: `SP_TEST_CULL=a.3FR:b.3FR` compares the opened file with
        // the listed ones once the open has settled, for headless screenshots.
        if !self.culling_test_started
            && self.state.culling.is_none()
            && !self.state.decode_in_flight
        {
            if let (Some(list), Some(open)) = (
                std::env::var_os("SP_TEST_CULL"),
                self.state.open_image.clone(),
            ) {
                self.culling_test_started = true;
                self.state.selected_photos.clear();
                self.state.selected_photos.insert(open);
                for path in std::env::split_paths(&list) {
                    self.state.selected_photos.insert(path);
                }
                self.state.culling_requested = true;
            }
        }
        if std::mem::take(&mut self.state.culling_requested) {
            self.start_culling(ctx);
        }
        if std::mem::take(&mut self.state.culling_exit_requested) {
            self.stop_culling();
        }
        if let Some(path) = self.state.culling_open_requested.take() {
            self.stop_culling();
            self.start_decode(path, ctx);
        }
        if let Some(action) = self.state.culling_annotate.take() {
            match action {
                crate::state::CullingAnnotate::Flag(flag) => {
                    self.annotate_culling_focus(|a| a.flag = flag);
                }
                crate::state::CullingAnnotate::Rating(rating) => {
                    self.annotate_culling_focus(|a| a.rating = rating);
                }
            }
        }
        self.sync_culling(ctx);
    }
}
