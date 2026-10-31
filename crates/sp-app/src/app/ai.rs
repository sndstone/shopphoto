//! Neural RAW denoise scheduling and the before/after comparison renderer.

use super::*;

/// See [`ShopPhotoApp::compare_request`].
pub(super) struct CompareRequest {
    pub(super) kind: CompareKind,
    pub(super) source: ImageSource,
    pub(super) edit: EditState,
    pub(super) external_raw_denoise: bool,
}

impl ShopPhotoApp {
    pub(super) fn sync_ai_denoise(&mut self, ctx: &egui::Context) {
        use sp_core::edit::CameraDenoiseMode;
        let mode = self.state.edit.nr.camera_denoise_mode;
        let compute = self.state.raw_denoise_compute;
        let compute_changed = compute != self.ai_requested_compute;
        if mode == self.ai_requested_mode && !compute_changed {
            return;
        }
        self.ai_requested_mode = mode;
        self.ai_requested_compute = compute;
        if compute_changed {
            self.ai_inferred_raw = None;
        }
        self.jobs.cancel_ai_denoise();
        self.ai_generation = None;
        self.state.ai_denoise_busy = false;
        if mode == CameraDenoiseMode::Off {
            self.ai_applied_mode = None;
            self.state.ai_denoise_status = Some("AI denoise off".into());
            self.state.ai_compare_original = false;
            self.state.ai_compare_split = false;
            self.ai_result_raw = None;
            if let (Some(source), Some(mut output)) =
                (self.ai_source_raw.clone(), self.current.take())
            {
                output.source = ImageSource::Raw(source);
                self.gpu_open(&output, false, ctx);
                self.current = Some(output);
            }
            return;
        }
        if !self.state.ai_denoise_available {
            self.state.ai_denoise_status =
                Some("AI models unavailable · using GPU fallback".into());
            return;
        }
        let (Some(path), Some(source), Some(meta)) = (
            self.state.open_image.clone(),
            self.ai_source_raw.clone(),
            self.state.meta.clone(),
        ) else {
            return;
        };
        let cached = self.ai_inferred_raw.clone();
        let reblend_only = cached.is_some();
        let generation = self
            .jobs
            .request_ai_denoise(path, source, meta, mode, compute, cached);
        self.state.ai_compare_original = false;
        self.state.ai_compare_split = false;
        self.ai_generation = Some(generation);
        self.state.ai_denoise_busy = true;
        self.state.ai_denoise_status = Some(
            match (mode, reblend_only) {
                (CameraDenoiseMode::Cleanest, false) => "Enhancing RAW · Cleanest…",
                (CameraDenoiseMode::PreserveDetail, false) => "Enhancing RAW · Preserve detail…",
                (CameraDenoiseMode::Cleanest, true) => "Blending · Cleanest…",
                (CameraDenoiseMode::PreserveDetail, true) => "Blending · Preserve detail…",
                (CameraDenoiseMode::Off, _) => unreachable!(),
            }
            .into(),
        );
        ctx.request_repaint_after(std::time::Duration::from_millis(50));
    }

    pub(super) fn sync_ai_compare(&mut self) {
        let available = self.ai_applied_mode == Some(self.state.edit.nr.camera_denoise_mode)
            && self.ai_source_raw.is_some()
            && self.ai_result_raw.is_some();
        if !available {
            self.state.ai_compare_original = false;
            self.state.ai_compare_split = false;
        }
    }

    pub(super) fn clear_ai_split_renderer(&mut self) {
        let Some(gpu) = &mut self.gpu else {
            return;
        };
        let Some(mut compare) = gpu.compare.take() else {
            return;
        };
        let mut renderer = gpu.render_state.renderer.write();
        renderer.free_texture(&compare.tex_id);
        if let Some(id) = compare.tile_id.take() {
            renderer.free_texture(&id);
        }
        if gpu.hdr_surface {
            if let Some(pack) = renderer
                .callback_resources
                .get_mut::<crate::hdr_view::HdrBlitPack>()
            {
                // Back to single-source bindings: the comparison views are
                // about to be freed.
                if let Some(view) = gpu.renderer.hdr_display_view() {
                    pack.bind(
                        &gpu.render_state.device,
                        crate::hdr_view::HdrSource::Proxy,
                        view,
                        None,
                    );
                }
                match gpu.renderer.tile_hdr_display_view() {
                    Some(view) => pack.bind(
                        &gpu.render_state.device,
                        crate::hdr_view::HdrSource::Tile,
                        view,
                        None,
                    ),
                    None => pack.unbind(crate::hdr_view::HdrSource::Tile),
                }
            }
        }
    }

    /// The comparison the viewport currently asks for, if any: the denoise
    /// before/after (untouched sensor data, current edit) or the edit
    /// before/after (current source, "before" edit).
    pub(super) fn compare_request(&self) -> Option<CompareRequest> {
        let ai_available = self.ai_applied_mode == Some(self.state.edit.nr.camera_denoise_mode)
            && self.ai_source_raw.is_some()
            && self.ai_result_raw.is_some();
        if (self.state.ai_compare_original || self.state.ai_compare_split) && ai_available {
            let edit = if self.state.show_original {
                EditState::default()
            } else {
                self.display_edit()
            };
            return Some(CompareRequest {
                kind: CompareKind::Denoise,
                source: ImageSource::Raw(self.ai_source_raw.clone()?),
                edit,
                external_raw_denoise: true,
            });
        }
        if self.state.compare_mode != crate::state::CompareMode::Off {
            let source = self.current.as_ref()?.source.clone();
            let edit = self
                .state
                .compare_before
                .as_ref()
                .map(|(_, edit)| edit.clone())
                .unwrap_or_default();
            return Some(CompareRequest {
                kind: CompareKind::Edit,
                source,
                edit,
                external_raw_denoise: self.ai_applied_mode
                    == Some(self.state.edit.nr.camera_denoise_mode),
            });
        }
        None
    }

    /// Keep the comparison renderer in lockstep with the develop renderer
    /// while a comparison is on screen. Both receive the same view options,
    /// profile, HDR mode, pan and zoom; they differ only in the sensor
    /// source (denoise compare) or the edit (edit compare).
    pub(super) fn sync_ai_split_compare(&mut self, ctx: &egui::Context) {
        let ai_available = self.ai_applied_mode == Some(self.state.edit.nr.camera_denoise_mode)
            && self.ai_source_raw.is_some()
            && self.ai_result_raw.is_some();
        if (self.state.ai_compare_original || self.state.ai_compare_split) && !ai_available {
            self.state.ai_compare_original = false;
            self.state.ai_compare_split = false;
        }
        let Some(request) = self.compare_request() else {
            self.pending_compare = None;
            self.clear_ai_split_renderer();
            return;
        };
        if self
            .gpu
            .as_ref()
            .and_then(|gpu| gpu.compare.as_ref())
            .is_some_and(|compare| compare.kind != request.kind)
        {
            self.clear_ai_split_renderer();
        }
        if self
            .pending_compare
            .as_ref()
            .is_some_and(|(_, kind, _)| *kind != request.kind)
        {
            self.pending_compare = None;
        }

        let edit_for_render = request.edit.clone();
        let render_key = (edit_for_render.clone(), self.state.show_original);
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
            mask_preview: None,
            analysis: false,
        };
        let profile = if request.kind == CompareKind::Denoise && self.state.show_original {
            None
        } else {
            self.profile_for(&edit_for_render)
        };
        let custom_look = self.custom_look_for(&edit_for_render);
        let custom_proof = self.custom_proof_for_view();
        let lens_key = self.lens_key();
        let lens = self.state.meta.as_ref().and_then(|meta| {
            resolve_lens(&self.lenses, &self.state.edit.lens, meta)
                .map(|profile| (profile.clone(), meta.aperture_f))
        });

        if self.gpu.as_ref().is_some_and(|gpu| gpu.compare.is_none()) {
            if self.pending_compare.is_none() {
                let setup =
                    self.renderer_setup(edit_for_render.clone(), request.external_raw_denoise, ctx);
                let rx = setup.and_then(|setup| {
                    self.spawn_renderer_build(request.source.clone(), setup, false, ctx)
                });
                match rx {
                    Some(rx) => {
                        self.pending_compare = Some((self.gpu_open_generation, request.kind, rx));
                    }
                    None => {
                        self.state.ai_compare_original = false;
                        self.state.ai_compare_split = false;
                        self.state.compare_mode = crate::state::CompareMode::Off;
                        self.state
                            .push_toast("Could not start the comparison: GPU unavailable", true);
                    }
                }
            }
            return;
        }

        let Some(gpu) = &mut self.gpu else {
            return;
        };
        let primary_tile_info = gpu.tile_info;
        let Some(compare) = &mut gpu.compare else {
            return;
        };
        let interaction_active = ctx.input(|input| {
            input.pointer.primary_down()
                || (input.zoom_delta() - 1.0).abs() > 1.0e-4
                || input.smooth_scroll_delta.length_sq() > 0.0
        });
        let image_changed = compare.last_rendered.as_ref() != Some(&render_key)
            || compare.last_view != Some(view_key)
            || compare.last_lens.as_ref() != Some(&lens_key);
        if !live_tile_update_due(interaction_active, image_changed) {
            // Keep comparisons in sync during control changes; defer only gestures.
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
            return;
        }
        if compare.last_lens.as_ref() != Some(&lens_key) {
            match &lens {
                Some((profile, aperture)) => {
                    compare.renderer.set_lens_profile(Some(profile), *aperture)
                }
                None => compare.renderer.set_lens_profile(None, None),
            }
            compare.last_lens = Some(lens_key);
            compare.last_rendered = None;
        }
        if compare.last_rendered.as_ref() != Some(&render_key)
            || compare.last_view != Some(view_key)
        {
            compare.renderer.set_view_options(
                view_key.proof_space,
                view_key.gamut_warning,
                view_key.clipping_mode,
            );
            compare.renderer.set_custom_proof(custom_proof);
            compare
                .renderer
                .set_sharpen_mask_preview(view_key.sharpen_mask_preview);
            compare.renderer.set_profile(profile);
            compare.renderer.set_custom_look(custom_look);
            compare.renderer.render_without_histogram(&edit_for_render);
            compare.last_rendered = Some(render_key.clone());
            compare.last_view = Some(view_key);
            compare.tile_key = None;
        }

        let wants_tile = !self.state.crop_mode
            && self
                .state
                .effective_zoom()
                .zip(self.state.active_size)
                .is_some_and(|((z, _), (aw, _))| {
                    let proxy_density = compare.out_size.x / aw as f32;
                    z > proxy_density * 1.2
                });
        if wants_tile {
            if let (Some(region), Some(active)) =
                (self.state.visible_region(), self.state.active_size)
            {
                let cached_region = primary_tile_info
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
                let contains_visible = compare.tile_info.is_some_and(|info| {
                    info.x <= region.0
                        && info.y <= region.1
                        && info.x + info.w >= region.0 + region.2
                        && info.y + info.h >= region.1 + region.3
                });
                let tile_key = (
                    cached_region,
                    edit_for_render.clone(),
                    self.state.show_original,
                );
                if !contains_visible
                    || compare.tile_key.as_ref().is_none_or(|key| {
                        key.0 != tile_key.0 || key.1 != tile_key.1 || key.2 != tile_key.2
                    })
                {
                    let (x, y, width, height) = cached_region;
                    if let Some(info) =
                        compare
                            .renderer
                            .render_tile(&edit_for_render, x, y, width, height)
                    {
                        compare.tile_info = Some(info);
                        compare.tile_key = Some(tile_key);
                        let view = compare.renderer.tile_view().expect("tile rendered");
                        let mut egui_renderer = gpu.render_state.renderer.write();
                        if let Some(old) = compare.tile_id.take() {
                            egui_renderer.free_texture(&old);
                        }
                        compare.tile_id = Some(egui_renderer.register_native_texture(
                            &gpu.render_state.device,
                            view,
                            wgpu::FilterMode::Nearest,
                        ));
                    }
                }
            }
        } else if let Some(old) = compare.tile_id.take() {
            gpu.render_state.renderer.write().free_texture(&old);
            compare.tile_info = None;
            compare.tile_key = None;
        }

        // The HDR shader combines both scRGB sources before writing to the
        // float swapchain. The proxy pair is always bound; the tile pair
        // carries the untouched tile only when both tiles cover the same
        // region, so a denoised tile is never paired with a stale twin. The
        // viewport makes the same consistency check before using tiles.
        if gpu.hdr_surface {
            let tiles_match = wants_tile
                && gpu
                    .tile_info
                    .zip(compare.tile_info)
                    .is_some_and(|(a, b)| a == b);
            let proxy_pair = gpu
                .renderer
                .hdr_display_view()
                .zip(compare.renderer.hdr_display_view());
            let tile_pair = gpu.renderer.tile_hdr_display_view().map(|primary| {
                (
                    primary,
                    compare
                        .renderer
                        .tile_hdr_display_view()
                        .filter(|_| tiles_match),
                )
            });
            if let Some(pack) = gpu
                .render_state
                .renderer
                .write()
                .callback_resources
                .get_mut::<crate::hdr_view::HdrBlitPack>()
            {
                if let Some((primary, untouched)) = proxy_pair {
                    pack.bind(
                        &gpu.render_state.device,
                        crate::hdr_view::HdrSource::Proxy,
                        primary,
                        Some(untouched),
                    );
                }
                match tile_pair {
                    Some((primary, untouched)) => pack.bind(
                        &gpu.render_state.device,
                        crate::hdr_view::HdrSource::Tile,
                        primary,
                        untouched,
                    ),
                    None => pack.unbind(crate::hdr_view::HdrSource::Tile),
                }
            }
        }
    }
}
