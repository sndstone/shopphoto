//! GPU develop renderer: worker-side construction, adoption, and the
//! per-frame re-render and 1:1 tile scheduling.

use super::*;

pub(super) fn overscanned_tile_region(
    visible: (u32, u32, u32, u32),
    active: (u32, u32),
) -> (u32, u32, u32, u32) {
    const OVERSCAN: u32 = 256;
    let (x, y, w, h) = visible;
    let (aw, ah) = active;
    let x0 = x.saturating_sub(OVERSCAN);
    let y0 = y.saturating_sub(OVERSCAN);
    let x1 = x.saturating_add(w).saturating_add(OVERSCAN).min(aw);
    let y1 = y.saturating_add(h).saturating_add(OVERSCAN).min(ah);
    (
        x0,
        y0,
        x1.saturating_sub(x0).max(1),
        y1.saturating_sub(y0).max(1),
    )
}

/// Controls update full-resolution pixels even while the pointer is held down.
/// Gestures that only move the viewport may defer new tile coverage until idle.
pub(super) fn live_tile_update_due(interaction_active: bool, image_changed: bool) -> bool {
    !interaction_active || image_changed
}

/// A renderer under construction on a worker thread.
pub(super) type RendererBuild = Receiver<Result<Box<GpuRenderer>, String>>;

/// Snapshot of the app state a worker needs to build a develop renderer.
pub(super) struct RendererSetup {
    pub(super) hdr_display: bool,
    pub(super) proxy_long_edge: u32,
    pub(super) meta: Option<sp_core::ImageMeta>,
    pub(super) external_raw_denoise: bool,
    pub(super) lens: Option<(sp_core::lens::LensProfile, Option<f32>)>,
    pub(super) profile: Option<sp_core::profile::CameraProfile>,
    pub(super) custom_look: Option<(String, sp_core::cube::CubeLut)>,
    pub(super) custom_proof: Option<sp_pipeline::proof::CustomProof>,
    pub(super) view: ViewKey,
    pub(super) edit: EditState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ViewKey {
    pub(super) proof_space: sp_core::edit::ProofSpace,
    pub(super) custom_proof_generation: u64,
    pub(super) gamut_warning: bool,
    pub(super) clipping_mode: u32,
    pub(super) sharpen_mask_preview: bool,
    pub(super) mask_preview: Option<usize>,
    pub(super) analysis: bool,
}

/// A second develop pipeline used only while the denoise split comparison is
/// visible. Keeping it resident means both halves update at the same zoom and
/// pan position without repeatedly re-uploading a 50/100 MP RAW.
/// What the comparison renderer shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CompareKind {
    /// The untouched sensor data, developed with the current edit.
    Denoise,
    /// The current source, developed with the "before" edit.
    Edit,
}

pub(super) struct CompareGpuState {
    pub(super) kind: CompareKind,
    pub(super) renderer: GpuRenderer,
    pub(super) tex_id: egui::TextureId,
    pub(super) out_size: egui::Vec2,
    pub(super) last_rendered: Option<(EditState, bool)>,
    pub(super) last_view: Option<ViewKey>,
    pub(super) tile_id: Option<egui::TextureId>,
    pub(super) tile_info: Option<sp_pipeline::renderer::TileInfo>,
    pub(super) tile_key: Option<TileKey>,
    pub(super) last_lens: Option<Option<(String, u32)>>,
}

/// GPU develop pipeline + the egui handle to its output texture.
pub(super) struct GpuState {
    pub(super) renderer: GpuRenderer,
    pub(super) render_state: egui_wgpu::RenderState,
    /// egui id of the registered output texture (freed/re-registered per
    /// opened image; render() updates the texture in place).
    pub(super) tex_id: Option<egui::TextureId>,
    pub(super) out_size: egui::Vec2,
    /// (edit, show_original) the current output texture was rendered with.
    pub(super) last_rendered: Option<(EditState, bool)>,
    pub(super) last_view: Option<ViewKey>,
    /// The swapchain is Rgba16Float (Metal EDR path active).
    pub(super) hdr_surface: bool,
    /// egui id + geometry of the current 1:1 tile, when zoomed in.
    pub(super) tile_id: Option<egui::TextureId>,
    pub(super) tile_info: Option<sp_pipeline::renderer::TileInfo>,
    /// Key of the last tile render: (region, edit, show_original).
    pub(super) tile_key: Option<TileKey>,
    /// Noise-loupe texture (1:1 crop shown in the Detail panel while the
    /// viewport is in fit view; shares the tile machinery).
    pub(super) loupe_id: Option<egui::TextureId>,
    pub(super) loupe_info: Option<sp_pipeline::renderer::TileInfo>,
    pub(super) loupe_key: Option<TileKey>,
    /// (profile name, aperture bits) of the lens grid currently uploaded.
    /// Uploading a grid means re-baking the proxy source, so it only
    /// happens when the resolved profile actually changes.
    pub(super) last_lens: Option<Option<(String, u32)>>,
    /// Untouched-RAW renderer for the denoise split comparison.
    pub(super) compare: Option<CompareGpuState>,
}

impl ShopPhotoApp {
    /// Resolve the edit's profile choice against loaded profiles.
    /// Missing named profiles fall back to Tier-1 (embedded matrix).
    pub(super) fn active_profile(&self) -> Option<sp_core::profile::CameraProfile> {
        self.profile_for(&self.state.edit)
    }

    pub(super) fn profile_for(&self, edit: &EditState) -> Option<sp_core::profile::CameraProfile> {
        if !self.state.source_is_raw {
            return None;
        }
        match &edit.profile {
            sp_core::edit::ProfileChoice::Standard => None,
            sp_core::edit::ProfileChoice::Named(name) => {
                let model = self.state.meta.as_ref().map(|m| m.camera_model.as_str())?;
                self.profiles
                    .iter()
                    .find(|p| &p.name == name && camera_matches(&p.camera_model, model))
                    .cloned()
            }
        }
    }

    pub(super) fn custom_look_for(
        &self,
        edit: &EditState,
    ) -> Option<(String, sp_core::cube::CubeLut)> {
        let name = edit.look.as_ref()?;
        self.custom_looks
            .iter()
            .find(|(candidate, _)| candidate == name)
            .cloned()
    }

    /// Identity of the lens grid the current image and edit call for.
    /// Cheap to compute every frame; the grid itself is only uploaded when
    /// this changes.
    pub(super) fn lens_key(&self) -> Option<(String, u32)> {
        if !self.state.source_is_raw {
            return None;
        }
        let meta = self.state.meta.as_ref()?;
        let profile = resolve_lens(&self.lenses, &self.state.edit.lens, meta)?;
        Some((
            profile.name.clone(),
            meta.aperture_f.unwrap_or_default().to_bits(),
        ))
    }

    /// Push the matching lens shading grid to the renderer if it differs
    /// from what is already loaded. Returns whether anything changed.
    pub(super) fn sync_lens_profile(&mut self) -> bool {
        let key = self.lens_key();
        if self.gpu.as_ref().map(|g| &g.last_lens) == Some(&Some(key.clone())) {
            return false;
        }
        let resolved = self
            .state
            .meta
            .as_ref()
            .filter(|_| self.state.source_is_raw)
            .and_then(|meta| {
                resolve_lens(&self.lenses, &self.state.edit.lens, meta)
                    .map(|p| (p.clone(), meta.aperture_f))
            });
        let Some(gpu) = &mut self.gpu else {
            return false;
        };
        match &resolved {
            Some((profile, aperture)) => gpu.renderer.set_lens_profile(Some(profile), *aperture),
            None => gpu.renderer.set_lens_profile(None, None),
        }
        gpu.last_lens = Some(key);
        true
    }

    pub(super) fn custom_proof_for_view(&self) -> Option<sp_pipeline::proof::CustomProof> {
        let name = self.state.selected_custom_proof.as_deref()?;
        self.custom_proofs
            .iter()
            .find(|proof| proof.name == name)
            .cloned()
    }

    /// Downscale an embedded preview to a filmstrip thumbnail texture.
    pub(super) fn make_thumb(
        ctx: &egui::Context,
        preview: &sp_raw::Preview,
    ) -> egui::TextureHandle {
        const THUMB_W: u32 = 192;
        let step = (preview.width / THUMB_W).max(1);
        let (tw, th) = (preview.width / step, preview.height / step);
        let mut px = Vec::with_capacity((tw * th * 4) as usize);
        for y in 0..th {
            for x in 0..tw {
                let src = (((y * step) * preview.width + x * step) * 4) as usize;
                px.extend_from_slice(&preview.rgba8[src..src + 4]);
            }
        }
        ctx.load_texture(
            "thumb",
            egui::ColorImage::from_rgba_unmultiplied([tw as usize, th as usize], &px),
            egui::TextureOptions::LINEAR,
        )
    }

    /// Open the decoded image on the GPU, render it with the current edit,
    /// and (re)register the output texture with egui.
    /// Everything a worker needs to build a develop renderer for `raw`
    /// without touching app state.
    pub(super) fn renderer_setup(
        &self,
        edit: EditState,
        external_raw_denoise: bool,
        egui_ctx: &egui::Context,
    ) -> Option<RendererSetup> {
        let gpu = self.gpu.as_ref()?;
        let lens = self.state.meta.as_ref().and_then(|meta| {
            resolve_lens(&self.lenses, &self.state.edit.lens, meta)
                .map(|p| (p.clone(), meta.aperture_f))
        });
        let viewport_edge = self.state.viewport_size.0.max(self.state.viewport_size.1);
        let proxy_long_edge = if viewport_edge >= 1.0 {
            (viewport_edge * egui_ctx.pixels_per_point() * 2.0).ceil() as u32
        } else {
            2048
        };
        let view = ViewKey {
            proof_space: self.state.proof_space,
            custom_proof_generation: self.state.custom_proof_generation,
            gamut_warning: self.state.gamut_warning,
            clipping_mode: if self.state.alt_clipping {
                3
            } else {
                self.state.clipping_mode
            },
            sharpen_mask_preview: self.state.sharpen_mask_preview,
            analysis: self.state.scopes_enabled || !self.state.edit.sample_points.is_empty(),
            mask_preview: self
                .state
                .mask_show_overlay
                .then_some(self.state.retouch_active_local)
                .flatten(),
        };
        Some(RendererSetup {
            hdr_display: gpu.hdr_surface,
            proxy_long_edge,
            meta: self.state.meta.clone(),
            external_raw_denoise,
            lens,
            profile: self.active_profile(),
            custom_look: self.custom_look_for(&edit),
            custom_proof: self.custom_proof_for_view(),
            view,
            edit,
        })
    }

    /// Upload `raw` and render it on a worker thread. The 200 MB CFA
    /// upload, the texture allocation and the first render used to run
    /// inside the frame, which is one of the main-thread stalls behind the
    /// macOS beach ball. The worker shares this renderer's compiled
    /// pipelines, so it does not compile shaders either.
    pub(super) fn spawn_renderer_build(
        &self,
        source: ImageSource,
        setup: RendererSetup,
        want_histogram: bool,
        egui_ctx: &egui::Context,
    ) -> Option<RendererBuild> {
        let gpu = self.gpu.as_ref()?;
        let gpu_ctx = GpuContext::new(
            gpu.render_state.device.clone(),
            gpu.render_state.queue.clone(),
        );
        let pipes = gpu.renderer.shared_pipelines();
        let (tx, rx) = crossbeam_channel::bounded(1);
        let wake = egui_ctx.clone();
        std::thread::Builder::new()
            .name("sp-gpu-open".into())
            .spawn(move || {
                let result = (|| -> Result<Box<GpuRenderer>, String> {
                    let mut renderer =
                        GpuRenderer::with_pipelines(gpu_ctx, pipes).map_err(|e| e.to_string())?;
                    renderer.set_hdr_display(setup.hdr_display);
                    renderer.set_proxy_long_edge(setup.proxy_long_edge);
                    renderer.set_capture_metadata(setup.meta.as_ref());
                    renderer.set_external_raw_denoise(setup.external_raw_denoise);
                    match &setup.lens {
                        Some((profile, aperture)) => {
                            renderer.set_lens_profile(Some(profile), *aperture)
                        }
                        None => renderer.set_lens_profile(None, None),
                    }
                    renderer
                        .open_source_with_edit(&source, &setup.edit)
                        .map_err(|e| e.to_string())?;
                    renderer.set_profile(setup.profile);
                    renderer.set_custom_look(setup.custom_look);
                    renderer.set_view_options(
                        setup.view.proof_space,
                        setup.view.gamut_warning,
                        setup.view.clipping_mode,
                    );
                    renderer.set_custom_proof(setup.custom_proof);
                    renderer.set_sharpen_mask_preview(setup.view.sharpen_mask_preview);
                    if want_histogram {
                        let _ = renderer.render(&setup.edit);
                    } else {
                        renderer.render_without_histogram(&setup.edit);
                    }
                    Ok(Box::new(renderer))
                })();
                let _ = tx.send(result);
                wake.request_repaint();
            })
            .ok()?;
        Some(rx)
    }

    /// Open the decoded image on the GPU, rendered with the current edit.
    /// Returns immediately; the renderer is adopted in a later frame by
    /// [`Self::adopt_ready_renderers`]. `new_file` drops the previous
    /// output so the viewport shows the embedded preview meanwhile instead
    /// of the previous photo; a same-image swap (AI denoise) keeps it.
    pub(super) fn gpu_open(
        &mut self,
        output: &DecodeOutput,
        new_file: bool,
        egui_ctx: &egui::Context,
    ) {
        let Some(setup) = self.renderer_setup(
            self.state.edit.clone(),
            self.ai_applied_mode == Some(self.state.edit.nr.camera_denoise_mode),
            egui_ctx,
        ) else {
            return;
        };
        self.gpu_open_generation += 1;
        self.pending_compare = None;
        let generation = self.gpu_open_generation;
        if new_file {
            self.clear_ai_split_renderer();
            if let Some(gpu) = &mut self.gpu {
                let mut er = gpu.render_state.renderer.write();
                for id in [gpu.tex_id.take(), gpu.tile_id.take(), gpu.loupe_id.take()]
                    .into_iter()
                    .flatten()
                {
                    er.free_texture(&id);
                }
                drop(er);
                gpu.tile_info = None;
                gpu.tile_key = None;
                gpu.loupe_info = None;
                gpu.loupe_key = None;
                gpu.last_rendered = None;
            }
        }
        match self.spawn_renderer_build(output.source.clone(), setup, true, egui_ctx) {
            Some(rx) => self.pending_gpu_open = Some((generation, rx)),
            None => self
                .state
                .push_toast("GPU render failed: could not start the upload worker", true),
        }
    }

    /// Adopt renderers finished by worker threads: swap the develop
    /// renderer, or install the comparison renderer, and register their
    /// output textures with egui. Cheap when nothing is pending.
    pub(super) fn adopt_ready_renderers(&mut self, ctx: &egui::Context) {
        if let Some((generation, rx)) = &self.pending_gpu_open {
            match rx.try_recv() {
                Ok(result) => {
                    let current = *generation == self.gpu_open_generation;
                    self.pending_gpu_open = None;
                    match result {
                        Ok(renderer) if current => self.adopt_renderer(*renderer, ctx),
                        Ok(_) => {}
                        Err(error) => self.state.push_toast(
                            crate::i18n::format(
                                "GPU render failed: {0}",
                                std::slice::from_ref(&error),
                            ),
                            true,
                        ),
                    }
                }
                Err(crossbeam_channel::TryRecvError::Empty) => {}
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    self.pending_gpu_open = None;
                    self.state
                        .push_toast("GPU render failed: upload worker exited", true);
                }
            }
        }
        if let Some((generation, kind, rx)) = &self.pending_compare {
            match rx.try_recv() {
                Ok(result) => {
                    let kind = *kind;
                    let current = *generation == self.gpu_open_generation
                        && self
                            .compare_request()
                            .is_some_and(|request| request.kind == kind);
                    self.pending_compare = None;
                    match result {
                        Ok(renderer) if current => {
                            self.adopt_compare_renderer(*renderer, kind, ctx)
                        }
                        Ok(_) => {}
                        Err(error) => {
                            self.state.ai_compare_original = false;
                            self.state.ai_compare_split = false;
                            self.state.compare_mode = crate::state::CompareMode::Off;
                            self.state.push_toast(
                                crate::i18n::format(
                                    "Could not start the comparison: {0}",
                                    std::slice::from_ref(&error),
                                ),
                                true,
                            );
                        }
                    }
                }
                Err(crossbeam_channel::TryRecvError::Empty) => {}
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    self.pending_compare = None;
                }
            }
        }
    }

    pub(super) fn adopt_renderer(&mut self, renderer: GpuRenderer, ctx: &egui::Context) {
        let lens_key = self.lens_key();
        let Some(gpu) = &mut self.gpu else {
            return;
        };
        // Dropping the previous renderer frees its VRAM.
        gpu.renderer = renderer;
        self.state.analysis_frame = None;
        self.state.spot_ui = crate::state::SpotUi::default();
        self.state.analysis_dirty = true;
        gpu.last_lens = Some(lens_key);
        // The worker rendered a snapshot of the edit; leaving the keys
        // empty makes the next frame re-render only if the edit or view
        // changed meanwhile.
        gpu.last_rendered = None;
        gpu.last_view = None;
        gpu.tile_key = None;
        gpu.tile_info = None;
        gpu.loupe_key = None;
        gpu.loupe_info = None;
        if gpu.hdr_surface {
            if let Some(pack) = gpu
                .render_state
                .renderer
                .write()
                .callback_resources
                .get_mut::<crate::hdr_view::HdrBlitPack>()
            {
                pack.unbind(crate::hdr_view::HdrSource::Tile);
                if let Some(view) = gpu.renderer.hdr_display_view() {
                    pack.bind(
                        &gpu.render_state.device,
                        crate::hdr_view::HdrSource::Proxy,
                        view,
                        None,
                    );
                }
            }
        }
        let (_, w, h) = gpu.renderer.output().expect("image just opened");
        gpu.out_size = egui::vec2(w as f32, h as f32);
        let view = gpu.renderer.output_view().expect("image just opened");
        let mut er = gpu.render_state.renderer.write();
        for id in [gpu.tex_id.take(), gpu.tile_id.take(), gpu.loupe_id.take()]
            .into_iter()
            .flatten()
        {
            er.free_texture(&id);
        }
        gpu.tex_id = Some(er.register_native_texture(
            &gpu.render_state.device,
            view,
            wgpu::FilterMode::Linear,
        ));
        drop(er);
        self.state.render_count += 1;
        ctx.request_repaint();
    }

    pub(super) fn adopt_compare_renderer(
        &mut self,
        renderer: GpuRenderer,
        kind: CompareKind,
        ctx: &egui::Context,
    ) {
        let Some(gpu) = &mut self.gpu else {
            return;
        };
        let Some((_, width, height)) = renderer.output() else {
            return;
        };
        let Some(view) = renderer.output_view() else {
            return;
        };
        let tex_id = gpu.render_state.renderer.write().register_native_texture(
            &gpu.render_state.device,
            view,
            wgpu::FilterMode::Linear,
        );
        let out_size = egui::vec2(width as f32, height as f32);
        // Empty keys: the lockstep sync re-applies lens, view and edit on
        // the next frame, so the comparison can never show a stale edit.
        gpu.compare = Some(CompareGpuState {
            kind,
            renderer,
            tex_id,
            out_size,
            last_rendered: None,
            last_view: None,
            tile_id: None,
            tile_info: None,
            tile_key: None,
            last_lens: None,
        });
        ctx.request_repaint();
    }

    /// Re-render when the edit changed since the last render. The output
    /// texture is updated in place, so only a repaint is needed.
    pub(super) fn gpu_rerender_if_dirty(&mut self, ctx: &egui::Context) {
        if self.state.decode_in_flight {
            return;
        }
        let interaction_active = ctx.input(|input| {
            input.pointer.primary_down()
                || (input.zoom_delta() - 1.0).abs() > 1.0e-4
                || input.smooth_scroll_delta.length_sq() > 0.0
        });
        let mut preview_edit = self.display_edit();
        preview_edit.sample_points.clear();
        let key = (preview_edit.clone(), self.state.show_original);
        let view_key = ViewKey {
            proof_space: self.state.proof_space,
            custom_proof_generation: self.state.custom_proof_generation,
            gamut_warning: self.state.gamut_warning,
            clipping_mode: if self.state.alt_clipping {
                3
            } else {
                self.state.clipping_mode
            },
            sharpen_mask_preview: self.state.sharpen_mask_preview,
            analysis: self.state.scopes_enabled || !self.state.edit.sample_points.is_empty(),
            mask_preview: self
                .state
                .mask_show_overlay
                .then_some(self.state.retouch_active_local)
                .flatten(),
        };
        let needs_rerender = self.gpu.as_ref().is_some_and(|g| {
            g.tex_id.is_some()
                && (g.last_rendered.as_ref() != Some(&key) || g.last_view != Some(view_key))
        });
        let profile_override = if needs_rerender && !self.state.show_original {
            self.active_profile()
        } else {
            None
        };
        let custom_look_override = if needs_rerender && !self.state.show_original {
            self.custom_look_for(&preview_edit)
        } else {
            None
        };
        let custom_proof = needs_rerender
            .then(|| self.custom_proof_for_view())
            .flatten();
        if needs_rerender {
            self.sync_lens_profile();
        }
        let Some(gpu) = &mut self.gpu else {
            return;
        };
        if gpu.tex_id.is_none() {
            return;
        }
        let edit_for_render = if self.state.show_original {
            EditState::default()
        } else {
            preview_edit
        };
        if needs_rerender {
            gpu.renderer.set_view_options(
                view_key.proof_space,
                view_key.gamut_warning,
                view_key.clipping_mode,
            );
            gpu.renderer.set_custom_proof(custom_proof);
            gpu.renderer
                .set_sharpen_mask_preview(view_key.sharpen_mask_preview);
            gpu.renderer.set_mask_preview(view_key.mask_preview);
            gpu.renderer.set_analysis_enabled(view_key.analysis);
            self.state.analysis_dirty = view_key.analysis;
            self.state.lens_coordinate_map = gpu.renderer.lens_coordinate_map(&edit_for_render);
            gpu.renderer.set_profile(profile_override);
            gpu.renderer.set_custom_look(custom_look_override);
            if self.state.auto_in_flight {
                // Keep the Auto histogram owned by the explicit readback
                // state machine. A dirty viewport render must neither consume
                // it nor queue a competing live histogram.
                gpu.renderer.render_without_histogram(&edit_for_render);
            } else if let Some(histogram) = gpu.renderer.render(&edit_for_render) {
                self.state.histogram = Some(histogram);
            }
            {
                let mut renderer = gpu.render_state.renderer.write();
                if let Some(old) = self.state.mask_overlay_texture.take() {
                    renderer.free_texture(&old);
                }
                self.state.mask_overlay_texture = gpu.renderer.mask_preview_view().map(|view| {
                    renderer.register_native_texture(
                        &gpu.render_state.device,
                        view,
                        wgpu::FilterMode::Linear,
                    )
                });
            }
            let edit_used = &edit_for_render;
            if gpu.hdr_surface && edit_used.hdr.enabled {
                if let Some(view) = gpu.renderer.hdr_display_view() {
                    if let Some(pack) = gpu
                        .render_state
                        .renderer
                        .write()
                        .callback_resources
                        .get_mut::<crate::hdr_view::HdrBlitPack>()
                    {
                        pack.bind(
                            &gpu.render_state.device,
                            crate::hdr_view::HdrSource::Proxy,
                            view,
                            None,
                        );
                    }
                }
            }
            self.state.render_count += 1;
            gpu.last_rendered = Some(key);
            if gpu.last_view != Some(view_key) {
                gpu.tile_key = None;
                gpu.loupe_key = None;
            }
            gpu.last_view = Some(view_key);
            ctx.request_repaint();
        }

        if !view_key.analysis {
            self.state.analysis_frame = None;
            self.state.analysis_dirty = false;
        } else if self.state.analysis_dirty && !interaction_active {
            match gpu.renderer.read_analysis_frame() {
                Ok(frame) => {
                    self.state.analysis_frame = frame.map(std::sync::Arc::new);
                    self.state.analysis_dirty = false;
                }
                Err(error) => {
                    self.state.analysis_dirty = false;
                    tracing::warn!("Scope readback failed: {error}");
                }
            }
        }

        // 1:1 tile path: when zoomed beyond the proxy's pixel density,
        // render the visible region at full resolution (Malvar).
        let wants_tile = !self.state.crop_mode
            && self
                .state
                .effective_zoom()
                .zip(self.state.active_size)
                .is_some_and(|((z, _), (aw, _))| {
                    let proxy_density = gpu.out_size.x / aw as f32;
                    z > proxy_density * 1.2
                });
        if wants_tile && live_tile_update_due(interaction_active, needs_rerender) {
            if let Some(region) = self.state.visible_region() {
                let active = self
                    .state
                    .active_size
                    .expect("tile requires an active image");
                let cached_region = gpu
                    .tile_info
                    .filter(|info| {
                        info.x <= region.0
                            && info.y <= region.1
                            && info.x + info.w >= region.0 + region.2
                            && info.y + info.h >= region.1 + region.3
                    })
                    .map_or_else(
                        || overscanned_tile_region(region, active),
                        |info| (info.x, info.y, info.w, info.h),
                    );
                let contains_visible = gpu.tile_info.is_some_and(|info| {
                    info.x <= region.0
                        && info.y <= region.1
                        && info.x + info.w >= region.0 + region.2
                        && info.y + info.h >= region.1 + region.3
                });
                let tkey = (
                    cached_region,
                    edit_for_render.clone(),
                    self.state.show_original,
                );
                if !contains_visible
                    || gpu
                        .tile_key
                        .as_ref()
                        .is_none_or(|key| key.1 != tkey.1 || key.2 != tkey.2)
                {
                    let (x, y, w, h) = cached_region;
                    if let Some(info) = gpu.renderer.render_tile(&edit_for_render, x, y, w, h) {
                        gpu.tile_info = Some(info);
                        gpu.tile_key = Some(tkey);
                        // The loupe shares the tile texture; its cache is
                        // now stale.
                        gpu.loupe_key = None;
                        let view = gpu.renderer.tile_view().expect("tile rendered");
                        let mut er = gpu.render_state.renderer.write();
                        if let Some(old) = gpu.tile_id.take() {
                            er.free_texture(&old);
                        }
                        gpu.tile_id = Some(er.register_native_texture(
                            &gpu.render_state.device,
                            view,
                            wgpu::FilterMode::Nearest,
                        ));
                        drop(er);
                        if gpu.hdr_surface {
                            if let Some(view) = gpu.renderer.tile_hdr_display_view() {
                                if let Some(pack) = gpu
                                    .render_state
                                    .renderer
                                    .write()
                                    .callback_resources
                                    .get_mut::<crate::hdr_view::HdrBlitPack>()
                                {
                                    pack.bind(
                                        &gpu.render_state.device,
                                        crate::hdr_view::HdrSource::Tile,
                                        view,
                                        None,
                                    );
                                }
                            }
                        }
                        ctx.request_repaint();
                    }
                }
            }
        } else if !wants_tile && gpu.tile_id.is_some() {
            let mut er = gpu.render_state.renderer.write();
            if let Some(old) = gpu.tile_id.take() {
                er.free_texture(&old);
            }
            gpu.tile_info = None;
            gpu.tile_key = None;
            drop(er);
            if gpu.hdr_surface {
                if let Some(pack) = gpu
                    .render_state
                    .renderer
                    .write()
                    .callback_resources
                    .get_mut::<crate::hdr_view::HdrBlitPack>()
                {
                    pack.unbind(crate::hdr_view::HdrSource::Tile);
                    if let Some(view) = gpu.renderer.hdr_display_view() {
                        pack.bind(
                            &gpu.render_state.device,
                            crate::hdr_view::HdrSource::Proxy,
                            view,
                            None,
                        );
                    }
                }
            }
        }
        if wants_tile && interaction_active {
            // Panning/zooming can defer tile coverage, but image controls and
            // view options update the displayed full-resolution tile live.
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }

        // Noise loupe (Phase 4): a 1:1 crop at the view center, shown in
        // the Detail panel. Only while in fit view — when zoomed, the
        // viewport itself is 1:1 (and shares the tile resources).
        let wants_loupe = !wants_tile
            && !self.state.crop_mode
            && self.state.panels.detail
            && self.state.open_image.is_some()
            && self.state.active_size.is_some();
        if wants_loupe {
            if let Some((aw, ah)) = self.state.active_size {
                const LOUPE: u32 = 180;
                let cx = (self.state.view_center.0 * aw as f32) as u32;
                let cy = (self.state.view_center.1 * ah as f32) as u32;
                let x = cx.saturating_sub(LOUPE / 2).min(aw.saturating_sub(LOUPE));
                let y = cy.saturating_sub(LOUPE / 2).min(ah.saturating_sub(LOUPE));
                let region = (x, y, LOUPE.min(aw), LOUPE.min(ah));
                let lkey = (region, edit_for_render.clone(), self.state.show_original);
                if gpu.loupe_key.as_ref() != Some(&lkey) {
                    let (x, y, w, h) = region;
                    if let Some(info) = gpu.renderer.render_tile(&edit_for_render, x, y, w, h) {
                        gpu.loupe_info = Some(info);
                        gpu.loupe_key = Some(lkey);
                        // Shared texture: viewport tile cache is now stale.
                        gpu.tile_key = None;
                        // The tile texture may have been recreated; always
                        // re-register for the loupe id.
                        let view = gpu.renderer.tile_view().expect("loupe rendered");
                        let mut er = gpu.render_state.renderer.write();
                        if let Some(old) = gpu.loupe_id.take() {
                            er.free_texture(&old);
                        }
                        gpu.loupe_id = Some(er.register_native_texture(
                            &gpu.render_state.device,
                            view,
                            wgpu::FilterMode::Nearest,
                        ));
                        ctx.request_repaint();
                    }
                }
            }
        } else if gpu.loupe_id.is_some() {
            let mut er = gpu.render_state.renderer.write();
            if let Some(old) = gpu.loupe_id.take() {
                er.free_texture(&old);
            }
            gpu.loupe_info = None;
            gpu.loupe_key = None;
        }
    }

    /// The edit the viewport shows: the stored edit, or a hovered history
    /// step, snapshot, preset or look, with bypassed inspector groups
    /// switched off. None of these hover states touch the undo history.
    pub(super) fn display_edit(&self) -> EditState {
        let mut edit = self
            .state
            .history_preview
            .clone()
            .unwrap_or_else(|| self.state.edit.clone());
        if let Some(preset) = self.state.preset_preview {
            let mut base = self.state.builtin_preset_base();
            preset.apply(&mut base);
            edit = base;
        }
        if let Some(look) = &self.state.look_preview {
            edit.look.clone_from(look);
        }
        crate::edit_groups::with_bypass(&edit, self.state.bypass_mask)
    }

    pub(super) fn invalidate_profile_render(&mut self) {
        if let Some(gpu) = &mut self.gpu {
            gpu.last_rendered = None;
            gpu.tile_key = None;
        }
    }
}

#[cfg(test)]
mod live_preview_tests {
    use super::*;
    #[test]
    fn zoomed_tile_updates_pixels_before_a_control_drag_is_released() {
        let Ok(gpu) = GpuContext::headless() else {
            return;
        };
        let mut renderer = GpuRenderer::new(gpu).unwrap();
        renderer.set_hdr_display(true);
        let raw = sp_core::RawImage {
            cfa_data: vec![8192; 128 * 128],
            width: 128,
            height: 128,
            cfa: sp_core::CfaPattern::Rggb,
            black_levels: [0.; 4],
            white_levels: [65535.; 4],
            wb_coeffs: [1.; 4],
            cam_to_xyz: sp_core::color::REC2020_TO_XYZ,
            active_area: sp_core::raw::ActiveArea {
                x: 0,
                y: 0,
                width: 128,
                height: 128,
            },
            orientation: sp_core::raw::Orientation::Normal,
        };
        renderer.open_image(&raw).unwrap();
        let ctx = egui::Context::default();
        let mut previous = None;
        let mut pixels = Vec::new();
        for frame in 0..3 {
            let events = if frame == 0 {
                vec![egui::Event::PointerButton {
                    pos: egui::pos2(10., 10.),
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                }]
            } else {
                vec![]
            };
            let mut edit = EditState::default();
            edit.tone.exposure_ev = frame as f32 * 0.5;
            let _ = ctx.run_ui(
                egui::RawInput {
                    events,
                    time: Some(frame as f64 / 60.),
                    ..Default::default()
                },
                |ui| {
                    let held = ui.ctx().input(|i| i.pointer.primary_down());
                    assert!(held, "test must remain mid-drag");
                    renderer.render_without_histogram(&edit);
                    if live_tile_update_due(held, previous.as_ref() != Some(&edit)) {
                        renderer.render_tile(&edit, 16, 16, 64, 64).unwrap();
                        previous = Some(edit.clone());
                    }
                    let (rgba, w, h) = renderer.read_scrgb_display_srgb8(true).unwrap();
                    let center = ((h / 2 * w + w / 2) * 4) as usize;
                    pixels.push(rgba[center]);
                },
            );
        }
        assert!(
            pixels[0] < pixels[1] && pixels[1] < pixels[2],
            "tile must change on each held-slider frame: {pixels:?}"
        );
        assert!(
            !live_tile_update_due(true, false),
            "unchanged-image pan can defer coverage"
        );
        assert!(
            live_tile_update_due(false, false),
            "idle must finish deferred coverage"
        );
    }
}
