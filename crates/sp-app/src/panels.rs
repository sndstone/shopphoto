//! All UI panels: top bar, left rail, develop right panel, filmstrip, library,
//! and the rendered viewport. Layout spec: BUILD.md §5.2.
//!
//! Widgets bind directly to `AppState.edit` (the live `EditState`); every
//! develop mutation schedules a proxy re-render.

use crate::state::{AppState, CurveChannel, GeometryTool, Mode, RetouchTool};
use crate::theme;
use sp_core::edit::ResolutionMode;

mod chrome;
mod develop;
mod dialogs;
mod library;
mod scopes;
mod tool_rail;
mod viewport;

pub use chrome::*;
pub use develop::*;
pub use dialogs::*;
pub use library::*;
pub use viewport::*;

// ---------------------------------------------------------------- top bar

// -------------------------------------------------------------- left rail

// ------------------------------------------------------------ right panel

// -------------------------------------------------------------- filmstrip

// ----------------------------------------------------------- library grid

// --------------------------------------------------------------- viewport

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drag_value_text_cannot_change_its_layout_advance() {
        let ctx = egui::Context::default();
        let mut advances = Vec::new();
        let _ = ctx.run_ui(Default::default(), |ui| {
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                for mut value in [0.0, -117.0, 50_000.0] {
                    let before = ui.cursor().left();
                    fixed_drag_value(ui, &mut value, -50_000.0..=50_000.0, 1.0);
                    advances.push(ui.cursor().left() - before);
                }
            });
        });

        assert_eq!(advances.len(), 3);
        assert!((advances[0] - advances[1]).abs() < f32::EPSILON);
        assert!((advances[0] - advances[2]).abs() < f32::EPSILON);
    }

    #[test]
    fn export_defaults_keep_high_resolution_files_near_twenty_five_megapixels() {
        assert_eq!(
            dialogs::recommended_export_size((10_000, 10_000)),
            sp_export::SizeMode::Percent(50.0)
        );
        assert_eq!(
            dialogs::recommended_export_size((10_000, 5_000)),
            sp_export::SizeMode::Percent(50.0)
        );
        assert_eq!(
            dialogs::recommended_export_size((6_000, 4_000)),
            sp_export::SizeMode::Original
        );
    }

    #[test]
    fn sony_fractional_exports_use_its_decoded_edge_dimensions() {
        let sony = (6_048, 4_024);
        assert_eq!(
            sp_export::SizeMode::Percent(50.0).output_dims(sony.0, sony.1),
            (3_024, 2_012)
        );
        assert_eq!(
            sp_export::SizeMode::Percent(25.0).output_dims(sony.0, sony.1),
            (1_512, 1_006)
        );
    }

    #[test]
    fn enabling_hdr_also_selects_the_hdr_target() {
        let mut edit = sp_core::EditState::default();
        assert_eq!(edit.hdr.target, sp_core::edit::HdrTarget::Sdr);

        edit.hdr.enabled = true;
        develop::select_hdr_target_when_enabled(&mut edit, false);

        assert_eq!(edit.hdr.target, sp_core::edit::HdrTarget::Hdr);
    }

    #[test]
    fn an_explicit_sdr_target_is_not_overridden_after_hdr_is_enabled() {
        let mut edit = sp_core::EditState::default();
        edit.hdr.enabled = true;
        edit.hdr.target = sp_core::edit::HdrTarget::Sdr;

        develop::select_hdr_target_when_enabled(&mut edit, true);

        assert_eq!(edit.hdr.target, sp_core::edit::HdrTarget::Sdr);
    }
}
