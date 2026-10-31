//! Automatic analysis (Auto balance, exposure, defringe, CA, level) driven
//! by asynchronous histogram readbacks.

use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AutoReadback {
    /// A normal live histogram still occupies the single staging buffer.
    BalanceWaitingForSlot,
    /// The in-flight histogram belongs to the neutral-tone analysis render.
    BalanceWaitingForNeutralHistogram,
    /// Exposure-only is waiting for the single staging buffer.
    ExposureWaitingForSlot,
    /// The in-flight histogram belongs to the current edited render.
    ExposureWaitingForCurrentHistogram,
    /// Auto Defringe is waiting for the single histogram staging buffer.
    DefringeWaitingForSlot,
    /// The in-flight histogram includes purple/green fringe-score planes.
    DefringeWaitingForHistogram,
}

impl AutoReadback {
    pub(super) fn is_analysis_histogram(self) -> bool {
        matches!(
            self,
            Self::BalanceWaitingForNeutralHistogram
                | Self::ExposureWaitingForCurrentHistogram
                | Self::DefringeWaitingForHistogram
        )
    }

    pub(super) fn retry_after_changed_edit(self) -> Self {
        match self {
            Self::BalanceWaitingForNeutralHistogram => Self::BalanceWaitingForSlot,
            Self::ExposureWaitingForCurrentHistogram => Self::ExposureWaitingForSlot,
            Self::DefringeWaitingForHistogram => Self::DefringeWaitingForSlot,
            waiting => waiting,
        }
    }
}

impl ShopPhotoApp {
    /// Schedule the neutral-tone render used by full Auto Balance once the
    /// renderer's single histogram staging buffer is free.
    pub(super) fn schedule_auto_balance_histogram(&mut self) -> bool {
        let Some(gpu) = &mut self.gpu else {
            return false;
        };
        if gpu.renderer.histogram_pending() {
            return false;
        }
        let mut analysis_edit = self.state.edit.clone();
        self.auto_analysis_edit = Some(self.state.edit.clone());
        analysis_edit.tone = sp_core::edit::ToneParams::default();
        let completed = gpu.renderer.render(&analysis_edit);
        debug_assert!(completed.is_none(), "histogram slot was checked empty");
        // The analysis render must reach the histogram pass, but it must not
        // flash in the viewport. Queue a display-only render of the actual
        // edit immediately after it; GPU submission order preserves both.
        gpu.renderer.render_without_histogram(&self.state.edit);
        gpu.renderer.histogram_pending()
    }

    /// Schedule a fresh histogram of the current edited image for the
    /// exposure-only command. This prevents a quick click after a slider move
    /// from applying an older histogram that happened to be on screen.
    pub(super) fn schedule_auto_exposure_histogram(&mut self) -> bool {
        let Some(gpu) = &mut self.gpu else {
            return false;
        };
        if gpu.renderer.histogram_pending() {
            return false;
        }
        self.auto_analysis_edit = Some(self.state.edit.clone());
        let completed = gpu.renderer.render(&self.state.edit);
        debug_assert!(completed.is_none(), "histogram slot was checked empty");
        gpu.renderer.histogram_pending()
    }

    /// Schedule the expensive spatial fringe scan only for the explicit Auto
    /// command. The correction is disabled in the analysis copy so a second
    /// click measures the original artifact rather than its previous repair.
    pub(super) fn schedule_auto_defringe_histogram(&mut self) -> bool {
        let Some(gpu) = &mut self.gpu else {
            return false;
        };
        if gpu.renderer.histogram_pending() {
            return false;
        }
        self.auto_analysis_edit = Some(self.state.edit.clone());
        let mut analysis_edit = self.state.edit.clone();
        analysis_edit.lens.defringe = false;
        let completed = gpu.renderer.render_for_fringe_analysis(&analysis_edit);
        debug_assert!(completed.is_none(), "histogram slot was checked empty");
        gpu.renderer.render_without_histogram(&self.state.edit);
        gpu.renderer.histogram_pending()
    }

    pub(super) fn run_auto_ca(&mut self) {
        let Some(gpu) = &mut self.gpu else {
            self.state
                .push_toast("Automatic lateral CA requires the GPU renderer", true);
            return;
        };
        let mut analysis = self.state.edit.clone();
        analysis.lens.lateral_ca = [0.0; 2];
        gpu.renderer.set_sharpen_mask_preview(false);
        gpu.renderer.render_without_histogram(&analysis);
        let result = gpu
            .renderer
            .read_display_srgb8()
            .ok()
            .and_then(|(rgba, width, height)| sp_pipeline::auto_ca::estimate(&rgba, width, height));
        match result {
            Some(result) => {
                self.state.edit.lens.enable = true;
                self.state.edit.lens.lateral_ca = [result.red_scale, result.blue_scale];
                gpu.last_rendered = None;
                gpu.tile_key = None;
                self.state.push_toast(
                    {
                        let __l10n_args = (&(result.red_scale), &(result.blue_scale));
                        crate::i18n::format(
                            "Auto lateral CA · red {0} · blue {1}",
                            &[
                                format!("{:+.4}", __l10n_args.0),
                                format!("{:+.4}", __l10n_args.1),
                            ],
                        )
                    },
                    false,
                );
            }
            None => self.state.push_toast(
                "Auto lateral CA found no reliable radial color shift",
                false,
            ),
        }
    }

    pub(super) fn run_auto_level(&mut self) {
        let Some(gpu) = &mut self.gpu else {
            self.state
                .push_toast("Auto level requires the GPU renderer", true);
            return;
        };
        gpu.renderer.set_sharpen_mask_preview(false);
        gpu.renderer.render_without_histogram(&self.state.edit);
        let result = gpu
            .renderer
            .read_display_srgb8()
            .ok()
            .and_then(|(rgba, width, height)| {
                sp_pipeline::auto_level::estimate_auto_level(&rgba, width, height)
            });
        match result {
            Some(angle) => {
                self.state.edit.geometry.rotate_deg = angle;
                gpu.last_rendered = None;
                gpu.tile_key = None;
                self.state.push_toast(
                    crate::i18n::format("Auto level · {0}°", &[format!("{:+.2}", angle)]),
                    false,
                );
            }
            None => self
                .state
                .push_toast("Auto level found no reliable horizon", false),
        }
    }
}
