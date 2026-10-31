//! Explicit donor suggestions and review-only dust corrections.
use super::*;
pub(super) struct Refinement {
    source: Option<PathBuf>,
    virtual_copy: Option<String>,
    before: Box<EditState>,
    index: usize,
    continue_painting: bool,
    result: Result<sp_core::mask::AlphaMask, String>,
}
impl ShopPhotoApp {
    pub(super) fn retouch_requests(&mut self, ctx: &egui::Context) {
        if let Some(receiver) = &self.mask_refine_job {
            match receiver.try_recv() {
                Ok(completed) => {
                    self.mask_refine_job = None;
                    self.state.spot_ui.refine_busy = false;
                    if completed.source == self.state.open_image
                        && completed.virtual_copy == self.state.virtual_copy
                        && *completed.before == self.state.edit
                    {
                        match completed.result {
                            Ok(mask) => {
                                let mut edit = self.state.edit.clone();
                                if let Some(adjustment) =
                                    edit.local_adjustments.get_mut(completed.index)
                                {
                                    adjustment.mask = sp_core::edit::LocalMask::Alpha(mask);
                                    adjustment.mask_inverted = false;
                                    adjustment.opacity = 1.0;
                                    self.state.apply_edit_step_labeled(
                                        edit,
                                        Some("Refine mask edges".into()),
                                    );
                                    if self.state.retouch_active_local == Some(completed.index) {
                                        self.state.retouch_tool = if completed.continue_painting {
                                            if self.state.retouch_tool
                                                == crate::state::RetouchTool::MaskSubtract
                                            {
                                                crate::state::RetouchTool::MaskSubtract
                                            } else {
                                                crate::state::RetouchTool::MaskAdd
                                            }
                                        } else {
                                            crate::state::RetouchTool::None
                                        };
                                    }
                                    self.state.mask_show_overlay = true;
                                }
                            }
                            Err(error) => self.state.push_toast(
                                crate::i18n::format(
                                    "Mask refinement failed: {0}",
                                    std::slice::from_ref(&error),
                                ),
                                true,
                            ),
                        }
                    } else if completed.continue_painting
                        && completed.source == self.state.open_image
                        && completed.virtual_copy == self.state.virtual_copy
                        && self.state.spot_ui.refine_after_stroke
                        && self.state.spot_ui.refine_request.is_none()
                    {
                        // A stroke or adjustment changed while processing. Coalesce
                        // to the latest active mask instead of replacing newer edits.
                        self.state.spot_ui.refine_request = self.state.retouch_active_local;
                        self.state.spot_ui.refine_stroke_pending = true;
                    } else if !completed.continue_painting
                        && completed.source == self.state.open_image
                    {
                        self.state.push_toast(
                            "Mask changed during refinement; the newer edit was preserved",
                            false,
                        );
                    }
                }
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    self.mask_refine_job = None;
                    self.state.spot_ui.refine_busy = false;
                    self.state
                        .push_toast("Mask refinement worker stopped", true);
                }
                Err(crossbeam_channel::TryRecvError::Empty) => {
                    ctx.request_repaint_after(std::time::Duration::from_millis(50));
                }
            }
        }
        if self.mask_refine_job.is_none() {
            if let Some(index) = self.state.spot_ui.refine_request.take() {
                let continue_painting =
                    std::mem::take(&mut self.state.spot_ui.refine_stroke_pending);
                let input = self
                    .gpu
                    .as_mut()
                    .ok_or_else(|| "Open an image first".to_owned())
                    .and_then(|gpu| {
                        gpu.renderer
                            .mask_refinement_input(&self.state.edit, index)
                            .map_err(|e| e.to_string())
                    });
                match input {
                    Ok((mask, guide)) => {
                        let before = Box::new(self.state.edit.clone());
                        let source = self.state.open_image.clone();
                        let virtual_copy = self.state.virtual_copy.clone();
                        let radius = self.state.spot_ui.refine_radius;
                        let tolerance = self.state.spot_ui.refine_tolerance;
                        let (tx, rx) = crossbeam_channel::bounded(1);
                        let repaint = ctx.clone();
                        std::thread::spawn(move || {
                            let result =
                                sp_core::mask::refine_guided(&mask, &guide, radius, tolerance);
                            let _ = tx.send(Refinement {
                                source,
                                virtual_copy,
                                before,
                                index,
                                continue_painting,
                                result,
                            });
                            repaint.request_repaint();
                        });
                        self.mask_refine_job = Some(rx);
                        self.state.spot_ui.refine_busy = true;
                    }
                    Err(error) => self.state.push_toast(
                        crate::i18n::format(
                            "Mask refinement failed: {0}",
                            std::slice::from_ref(&error),
                        ),
                        true,
                    ),
                }
            }
        }

        let source_request = self.state.spot_ui.source_request.take();
        let detect = std::mem::take(&mut self.state.spot_ui.detect_dust);
        if source_request.is_some() || detect {
            let result = self
                .gpu
                .as_mut()
                .map(|gpu| gpu.renderer.retouch_analysis_frame(&self.state.edit));
            match result {
                Some(Ok(frame)) => {
                    if let Some(index) = source_request {
                        if let Some(spot) = self.state.edit.spots.get(index) {
                            if let Some(source) = sp_core::retouch::suggest_source(&frame, spot) {
                                let mut edit = self.state.edit.clone();
                                edit.spots[index].source = source;
                                self.state.apply_edit_step_labeled(
                                    edit,
                                    Some("Find matching source".into()),
                                );
                                self.state.push_toast(
                                    "Source suggested · inspect and adjust its handle",
                                    false,
                                );
                            } else {
                                self.state
                                    .push_toast("No suitable source fits this correction", false);
                            }
                        }
                    }
                    if detect {
                        let mut candidates = sp_core::retouch::find_dust(
                            &frame,
                            self.state.spot_ui.dust_sensitivity,
                        );
                        let short = frame.width.min(frame.height) as f32;
                        candidates.retain(|candidate| {
                            !self.state.edit.spots.iter().any(|spot| {
                                spot.enabled
                                    && ((candidate.position[0] - spot.target[0])
                                        * frame.width as f32)
                                        .hypot(
                                            (candidate.position[1] - spot.target[1])
                                                * frame.height as f32,
                                        )
                                        < spot.radius * short
                            })
                        });
                        self.state.push_toast(
                            {
                                let __l10n_args = (&(candidates.len()),);
                                crate::i18n::format(
                                    "{0} possible dust marks · review before healing",
                                    &[__l10n_args.0.to_string()],
                                )
                            },
                            false,
                        );
                        self.state.spot_ui.dust_candidates = candidates;
                        self.state.spot_ui.dust_selected = None;
                        self.state.spot_ui.dust_context = Some(std::sync::Arc::new(frame));
                    }
                }
                Some(Err(error)) => self.state.push_toast(
                    crate::i18n::format("Retouch analysis failed: {0}", &[error.to_string()]),
                    true,
                ),
                None => self
                    .state
                    .push_toast("Open an image before analyzing", false),
            }
        }
        if let Some(index) = self.state.spot_ui.dust_apply.take() {
            let Some(candidate) = self.state.spot_ui.dust_candidates.get(index) else {
                return;
            };
            let mut spot = sp_core::edit::SpotCorrection {
                target: candidate.position,
                radius: candidate.radius,
                feather: 0.65,
                heal_texture: 100.0,
                ..Default::default()
            };
            let source = self
                .state
                .spot_ui
                .dust_context
                .as_ref()
                .and_then(|frame| sp_core::retouch::suggest_source(frame, &spot));
            let Some(source) = source else {
                self.state
                    .push_toast("No suitable donor · create a manual correction", false);
                return;
            };
            spot.source = source;
            let mut edit = self.state.edit.clone();
            let selected = edit.spots.len();
            edit.spots.push(spot);
            self.state
                .apply_edit_step_labeled(edit, Some("Heal reviewed dust".into()));
            self.state.spot_ui.selected = Some(selected);
            self.state.retouch_tool = crate::state::RetouchTool::Heal;
            self.state.spot_ui.dust_candidates.remove(index);
            self.state.spot_ui.dust_selected = None;
        }
    }
}
