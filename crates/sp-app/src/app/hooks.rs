//! Per-frame upkeep hooks: QA screenshots, GPU device-loss recovery, EDR
//! and HDR display checks.

use super::*;

impl ShopPhotoApp {
    /// Debug/QA hook: with `SP_SCREENSHOT=/path/out.png`, captures the
    /// window ~2.5 s after launch (decode+render settled) and exits.
    /// Used by the agent/CI to review the real UI headlessly.
    pub(super) fn screenshot_hook(&mut self, ctx: &egui::Context) {
        let Some(out) = std::env::var_os("SP_SCREENSHOT") else {
            return;
        };
        // eframe's surface readback assumes 4-byte pixels and panics on the
        // float EDR swapchain. There the capture is the developed image read
        // from the renderer's scRGB display texture (the 1:1 tile when
        // zoomed), without the surrounding UI; with SHOPPHOTO_SDR=1 the whole
        // window is captured as before.
        let float_surface = self.gpu.as_ref().is_some_and(|g| g.hdr_surface);
        // A sidecar may start Core ML immediately after decode. Begin the QA
        // capture countdown only once that asynchronous source replacement
        // has settled, otherwise screenshots can accidentally record the
        // short-lived pre-denoise frame.
        if self.state.decode_in_flight
            || self.state.ai_denoise_busy
            || self.state.export_busy
            || self.state.enhancement_busy
        {
            self.frame_count = 0;
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
            return;
        }
        self.frame_count += 1;
        // Complete native asynchronous readback callbacks even when an
        // unchanged photo no longer triggers renderer readbacks itself.
        if let Some(gpu) = &self.gpu {
            let _ = gpu
                .render_state
                .device
                .poll(eframe::egui_wgpu::wgpu::PollType::Poll);
        }
        if self.frame_count == 1 {
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(50));
        if self.frame_count == 5 && !self.qa_actions_applied {
            self.qa_actions_applied = true;
            self.apply_test_ui();
        }
        // Detections arrive after the QA actions; tick the first one so the
        // selected-element overlay can be captured too.
        if self.frame_count == 60
            && std::env::var("SP_TEST_UI").is_ok_and(|tokens| tokens.contains("pick-element"))
        {
            if let Some(item) = self
                .state
                .mask_candidates
                .as_mut()
                .and_then(|candidates| candidates.items.first_mut())
            {
                item.selected = true;
            }
        }
        // `SP_TEST_LAYER_EV` / `SP_TEST_LAYER_CLARITY` push created layers so
        // edge halos can be captured with the mask overlay hidden.
        if self.frame_count == 30 {
            let value = |name| std::env::var(name).ok().and_then(|v| v.parse::<f32>().ok());
            if let Some(ev) = value("SP_TEST_LAYER_EV").filter(|ev| *ev != 0.0) {
                if !self.state.edit.local_adjustments.is_empty()
                    && self.state.edit.local_adjustments[0].tone.exposure_ev == 0.0
                {
                    let mut edit = self.state.edit.clone();
                    for layer in &mut edit.local_adjustments {
                        layer.tone.exposure_ev = ev;
                        layer.local.clarity = value("SP_TEST_LAYER_CLARITY").unwrap_or(0.0);
                    }
                    self.state.mask_show_overlay = false;
                    self.state.apply_edit_step(edit);
                }
            }
        }
        // Turn every detection into a layer; candidates are cleared once the
        // layers exist, so this fires only once.
        if self.frame_count == 60
            && std::env::var("SP_TEST_UI").is_ok_and(|tokens| tokens.contains("create-elements"))
        {
            if let Some(candidates) = &self.state.mask_candidates {
                let ids: Vec<_> = candidates
                    .items
                    .iter()
                    .map(|item| item.id.to_string())
                    .collect();
                self.state.enhancement_requested =
                    Some(format!("selected:{}:{}", candidates.kind, ids.join(",")));
            }
        }
        if float_surface {
            if self.frame_count == 90 {
                let capture = self
                    .gpu
                    .as_ref()
                    .map(|g| g.renderer.read_scrgb_display_srgb8(g.tile_info.is_some()));
                match capture {
                    Some(Ok((rgba, w, h))) => {
                        if let Err(e) =
                            image::save_buffer(&out, &rgba, w, h, image::ColorType::Rgba8)
                        {
                            tracing::error!("screenshot save failed: {e}");
                        }
                    }
                    Some(Err(e)) => tracing::error!("HDR viewport capture failed: {e}"),
                    None => tracing::error!("HDR viewport capture needs the GPU renderer"),
                }
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            return;
        }
        // The screenshot event can be dropped while a large RAW upload is
        // still keeping the event loop busy. Retry a few times instead of
        // waiting forever after a single request.
        if (90..=210).contains(&self.frame_count) && self.frame_count.is_multiple_of(15) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
        }
        let shot: Option<std::sync::Arc<egui::ColorImage>> = ctx.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let Some(img) = shot {
            let [w, h] = img.size;
            let rgba: Vec<u8> = img
                .pixels
                .iter()
                .flat_map(|p| p.to_srgba_unmultiplied())
                .collect();
            if let Err(e) =
                image::save_buffer(&out, &rgba, w as u32, h as u32, image::ColorType::Rgba8)
            {
                tracing::error!("screenshot save failed: {e}");
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        } else if self.frame_count == 241 {
            tracing::error!("SP_SCREENSHOT timed out without a viewport readback");
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    /// QA hook: `SP_TEST_UI=library,split,…` puts the UI into states that
    /// otherwise need pointer input, so `SP_SCREENSHOT` can capture them.
    /// Tokens: edit, library, select, split, sbs, crop, zoom, panels, color,
    /// sheet, toast, bypass, native-people, native-sky, pick-element,
    /// create-elements, mask-points. Sidecars are never saved while it is set.
    fn apply_test_ui(&mut self) {
        let Some(tokens) = std::env::var_os("SP_TEST_UI") else {
            return;
        };
        let tokens = tokens.to_string_lossy().to_string();
        for token in tokens.split(',').map(str::trim) {
            match token {
                "edit" => {
                    let mut edit = self.state.edit.clone();
                    edit.tone.exposure_ev = 0.6;
                    self.state.apply_edit_step(edit.clone());
                    edit.tone.highlights = -24.0;
                    edit.tone.shadows = 18.0;
                    self.state.apply_edit_step(edit.clone());
                    edit.color.vibrance = 15.0;
                    edit.color.grading.shadows = sp_core::edit::ColorGradeWheel {
                        hue_degrees: 200.0,
                        saturation: 30.0,
                    };
                    self.state.apply_edit_step(edit.clone());
                    edit.local.clarity = 12.0;
                    self.state.apply_edit_step(edit);
                }
                "library" => self.state.mode = Mode::Library,
                "export" => self.state.export_dialog_open = true,
                "native-people" | "native-sky" => {
                    self.state.left_rail_tab = crate::state::LeftRailTab::Ai;
                    self.state.mask_subject = if token == "native-people" {
                        crate::state::MaskSubject::People
                    } else {
                        crate::state::MaskSubject::Sky
                    };
                    self.state.enhancement_requested =
                        Some(self.state.mask_subject.request_kind().into());
                }
                "mask-points" => {
                    self.state.left_rail_tab = crate::state::LeftRailTab::Ai;
                    self.state.mask_method = crate::state::MaskMethod::Click;
                    self.state.mask_prompts = vec![([0.5, 0.55], true), ([0.5, 0.15], false)];
                    self.state.mask_point_pick = true;
                    self.state.mask_prompt_pick = Some(true);
                }
                "ai-runtime" => self.state.enhance_runtime.open = true,
                "print" => self.state.print_dialog_open = true,
                "gallery-export" => {
                    if let Some(path) = std::env::var_os("SP_TEST_GALLERY") {
                        let destination = PathBuf::from(path);
                        self.gallery_request = Some(gallery::Request {
                            receipts: destination.with_extension("receipts"),
                            destination,
                            title: "Waterfall photo proofs".into(),
                            caption: true,
                        });
                        self.state.export_start_requested = true;
                    }
                }
                "print-export" => {
                    if let Some(path) = std::env::var_os("SP_TEST_PRINT_PDF") {
                        self.print_request = Some(printer::PrintRequest {
                            layout: self.state.print_layout.clone(),
                            destination: path.into(),
                        });
                        self.state.export_start_requested = true;
                    }
                }
                "queue" => self.state.export_queue_open = true,
                "select" => {
                    let paths: Vec<_> = self
                        .state
                        .catalog_photos
                        .iter()
                        .take(3)
                        .map(|p| p.path.clone())
                        .collect();
                    self.state.grid_focus = paths.last().cloned();
                    self.state.selected_photos = paths.into_iter().collect();
                    self.state.library_filter.minimum_rating = 0;
                }
                "split" => self.state.compare_mode = crate::state::CompareMode::Split,
                "sbs" => self.state.compare_mode = crate::state::CompareMode::SideBySide,
                "crop" => {
                    self.state.crop_mode = true;
                    self.state.crop_aspect = Some(1.5);
                    self.state.edit.geometry.crop = [0.1, 0.12, 0.85, 0.9];
                }
                "zoom" => self.state.zoom = Some(1.0),
                "panels" => {
                    self.state.panels.history = true;
                    self.state.panels.color = true;
                    self.state.panels.basic = false;
                }
                "sheet" => self.state.shortcuts_open = true,
                "color" => {
                    self.state.panels.basic = false;
                    self.state.panels.color = true;
                }
                "toast" => {
                    self.state.push_toast_with_action(
                        "Moved 3 photos with sidecars",
                        false,
                        Some(crate::state::ToastAction::UndoFileMove),
                    );
                    self.state.push_toast(
                        "Could not export presets: Permission denied (os error 13)",
                        true,
                    );
                }
                "bypass" => self.state.bypass_mask |= crate::edit_groups::Group::Color.bit(),
                _ => {}
            }
        }
    }

    /// macOS: apply the layer-colorspace shim until it sticks, then poll
    /// the screen's live EDR headroom for the badge (see macos_edr.rs).
    #[cfg(target_os = "macos")]
    pub(super) fn macos_edr_upkeep(&mut self) {
        if !self.gpu.as_ref().is_some_and(|g| g.hdr_surface) {
            return;
        }
        self.edr_poll_counter += 1;
        if !self.state.edr_colorspace_applied {
            self.state.edr_colorspace_applied = crate::macos_edr::apply_extended_colorspace();
        }
        if self.state.edr_colorspace_applied
            && (self.edr_poll_counter % 120 == 1 || self.state.edr_headroom == 0.0)
        {
            self.state.edr_headroom = crate::macos_edr::current_edr_headroom();
            self.state.edr_potential = crate::macos_edr::potential_edr_headroom();
        }
    }

    pub(super) fn gpu_recovery_upkeep(&mut self, ctx: &egui::Context) {
        if self.gpu_recovery_started {
            return;
        }
        if std::env::var_os("SP_SIMULATE_DEVICE_LOSS").is_some() {
            self.gpu_recovery_test_frames = self.gpu_recovery_test_frames.saturating_add(1);
            ctx.request_repaint();
            if self.gpu_recovery_test_frames == 60 {
                self.gpu_device_loss.simulate("requested test loss");
            }
        }
        let Some(message) = self.gpu_device_loss.take() else {
            return;
        };
        tracing::error!(%message, "GPU device lost; restarting graphics process");
        if !self.save_sidecar_if_due(true) {
            // Keep the document in this process when its only safe copy is
            // still in memory; graphics recovery must not discard it.
            self.gpu = None;
            return;
        }
        let attempts = std::env::var("SHOPPHOTO_GPU_RECOVERY_COUNT")
            .ok()
            .and_then(|value| value.parse::<u8>().ok())
            .unwrap_or(0);
        if attempts >= 2 {
            self.gpu = None;
            self.state.push_toast(
                "GPU recovery failed twice; restart after updating the graphics driver",
                true,
            );
            return;
        }
        let result = std::env::current_exe().and_then(|executable| {
            let mut command = std::process::Command::new(executable);
            if let Some(path) = &self.state.open_image {
                command.arg(path);
            }
            command
                .env("SHOPPHOTO_GPU_RECOVERY_COUNT", (attempts + 1).to_string())
                .spawn()
                .map(|_| ())
        });
        match result {
            Ok(()) => {
                self.gpu_recovery_started = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            Err(error) => {
                self.gpu = None;
                self.state.push_toast(
                    crate::i18n::format(
                        "GPU recovery could not restart ShopPhoto: {0}",
                        &[error.to_string()],
                    ),
                    true,
                );
            }
        }
    }

    #[cfg(not(target_os = "macos"))]
    pub(super) fn macos_edr_upkeep(&mut self) {
        #[cfg(target_os = "linux")]
        if let Some(gpu) = &self.gpu {
            self.state.edr_colorspace_applied = gpu.hdr_surface
                && gpu.render_state.adapter.get_info().backend == wgpu::Backend::Vulkan;
            // Vulkan's surface describes the encoding, not current monitor
            // luminance. Do not invent a peak from a GPU texture format.
            self.state.edr_headroom = 0.0;
            self.state.edr_potential = 0.0;
        }
        #[cfg(target_os = "windows")]
        if self.gpu.as_ref().is_some_and(|gpu| gpu.hdr_surface) {
            self.edr_poll_counter += 1;
            if self.edr_poll_counter % 120 == 1 || self.state.edr_headroom == 0.0 {
                let headroom = crate::windows_hdr::display_headroom();
                self.state.edr_colorspace_applied = true;
                self.state.edr_headroom = headroom;
                self.state.edr_potential = headroom;
            }
        }
    }

    /// `--hdr-check`: report the actual display pipeline state on stdout
    /// after the window and layer have settled, then quit. This is the
    /// canonical way to verify a build/binary has working EDR.
    pub(super) fn hdr_check_hook(&mut self, ctx: &egui::Context) {
        if !self.hdr_check {
            return;
        }
        self.frame_count += 1;
        // Immediate repaint is intentional for this short-lived diagnostic.
        // Some platforms suspend delayed repaint callbacks while the new
        // native window is not yet key, which used to leave --hdr-check open
        // forever after its first frame.
        ctx.request_repaint();
        if self.frame_count != 60 {
            return;
        }
        let surface_format = self
            .gpu
            .as_ref()
            .map(|g| format!("{:?}", g.render_state.target_format))
            .unwrap_or_else(|| "no GPU".into());
        let float_surface = self.gpu.as_ref().is_some_and(|g| g.hdr_surface);
        println!("── ShopPhoto HDR check ──────────────────────────────");
        println!("build:              {}", env!("CARGO_PKG_VERSION"));
        println!("surface format:     {surface_format}");
        println!(
            "float EDR surface:  {}",
            if float_surface {
                "YES"
            } else {
                "NO (8-bit SDR path)"
            }
        );
        println!(
            "layer colorspace:   {}",
            if self.state.edr_colorspace_applied {
                "extended linear sRGB (applied)"
            } else {
                "NOT applied"
            }
        );
        #[cfg(not(target_os = "linux"))]
        println!(
            "panel capability:   ×{:.1} potential EDR headroom{}",
            self.state.edr_potential.max(1.0),
            if self.state.edr_potential > 1.05 {
                "  (HDR panel)"
            } else {
                "  (no HDR panel detected)"
            }
        );
        #[cfg(not(target_os = "linux"))]
        println!(
            "current headroom:   ×{:.2} (ramps up once EDR content displays;              drops at max brightness)",
            self.state.edr_headroom.max(1.0)
        );
        #[cfg(target_os = "linux")]
        println!("panel/headroom:     unknown (not reported by the Vulkan surface)");
        let verdict = if cfg!(target_os = "linux")
            && float_surface
            && self.state.edr_colorspace_applied
        {
            "EXPERIMENTAL — Vulkan linear scRGB surface is advertised; monitor peak and visible HDR need verification on this system"
        } else if float_surface
            && self.state.edr_colorspace_applied
            && self.state.edr_potential > 1.05
        {
            "PASS — HDR highlights will display brighter than UI white              (enable HDR on an image to see it)"
        } else if float_surface && self.state.edr_colorspace_applied {
            "READY — pipeline OK; this display reports no EDR capability"
        } else if float_surface {
            "PARTIAL — float surface up but colorspace shim not applied yet"
        } else {
            "SDR — this environment did not select a supported HDR surface; HDR exports remain available"
        };
        println!("verdict:            {verdict}");
        println!("─────────────────────────────────────────────────────");
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }
}
