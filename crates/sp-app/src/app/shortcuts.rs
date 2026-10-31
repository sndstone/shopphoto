//! Keyboard shortcuts, and the table the shortcut sheet is generated from.

use super::*;
use crate::state::Annotate;

/// Every shortcut, grouped for the `?` sheet. Keep in step with
/// [`ShopPhotoApp::handle_shortcuts`]; the sheet searches this table.
pub const SHORTCUTS: &[(&str, &str, &str)] = &[
    ("Navigation", "⌘O", "Open photo…"),
    ("Navigation", "D", "Develop"),
    ("Navigation", "G", "Library grid"),
    (
        "Navigation",
        "← →",
        "Previous / next photo (grid: move focus)",
    ),
    ("Navigation", "↑ ↓", "Grid: move focus by a row"),
    ("Navigation", "⇧ + arrows", "Grid: extend the selection"),
    (
        "Navigation",
        "Return",
        "Grid: open the focused photo · Crop: apply",
    ),
    ("Navigation", "⌘A", "Grid: select all"),
    (
        "Navigation",
        "Esc",
        "Clear selection · leave compare · cancel tool",
    ),
    (
        "Navigation",
        "C",
        "Compare 2–4 selected photos side by side",
    ),
    (
        "Navigation",
        "Tab",
        "Hide side panels (when no control has focus)",
    ),
    ("View", "Z", "Fit ↔ 100%"),
    ("View", "\\", "Before / after (whole frame)"),
    ("View", "Y", "Cycle split / side-by-side / off"),
    ("View", "J", "Shadow clipping overlay"),
    ("View", "O", "Highlight clipping overlay"),
    ("View", "M", "Show AI mask coverage"),
    ("View", "F12", "Performance overlay"),
    (
        "Rating & labels",
        "0–5",
        "Set rating (grid: every selected photo)",
    ),
    ("Rating & labels", "P", "Pick"),
    ("Rating & labels", "X", "Reject · Crop: swap ratio"),
    ("Rating & labels", "U", "Unflag"),
    ("Rating & labels", "6", "Red label"),
    ("Rating & labels", "7", "Yellow label"),
    ("Rating & labels", "8", "Green label"),
    ("Rating & labels", "9", "Blue label"),
    ("Rating & labels", "⇧9", "Purple label"),
    ("Editing", "⌘Z", "Undo"),
    ("Editing", "⇧⌘Z", "Redo"),
    ("Editing", "E", "Export…"),
    ("Editing", "⌘F", "Find a tool in the inspector"),
    ("Editing", "Drag label", "Scrub a slider (⇧ for fine steps)"),
    (
        "Editing",
        "Double-click",
        "Reset a slider (on its label or track)",
    ),
    ("Editing", "⇧ double-click", "Auto Exposure (on Exposure)"),
    (
        "Editing",
        "Option drag",
        "Sharpen mask preview (on Masking)",
    ),
    (
        "Editing",
        "Option-click header",
        "Open only that inspector card",
    ),
    ("Help", "?", "This sheet"),
];

impl ShopPhotoApp {
    pub(super) fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        // Never steal keys while a text field is focused.
        if ctx.egui_wants_keyboard_input() || self.state.shortcuts_open {
            return;
        }
        if self.state.customize.open {
            return;
        }
        use super::customize::{pressed, Action};
        let bindings = self.state.customize.bindings.clone();
        // Tab and arrows belong to a focused control (button, slider…).
        let widget_focused = ctx.memory(|m| m.focused().is_some());
        let mut open_dialog = false;
        let before_annotations = self.state.annotations;
        let before_color = self.state.descriptive_metadata.color_label;
        let mut navigate = 0i32;
        let mut grid_move: Option<(i32, bool)> = None;
        let mut grid_annotate: Option<Annotate> = None;
        let mut grid_open = false;
        let mut grid_select_all = false;
        let culling_active = self.state.culling.is_some();
        let library = self.state.mode == Mode::Library && !culling_active;
        ctx.input(|i| {
            let action = |command| pressed(&bindings, command, i);
            // Single-letter shortcuts ignore ⌘/Ctrl/⌥ chords, so ⌘E does
            // not also open Export.
            let chord = i.modifiers.command || i.modifiers.ctrl || i.modifiers.alt;
            let plain = |key: egui::Key| !chord && i.key_pressed(key);
            if action(Action::Compare) {
                if culling_active {
                    self.state.culling_exit_requested = true;
                } else {
                    self.state.culling_requested = true;
                }
            }
            if culling_active {
                if i.key_pressed(egui::Key::Escape) {
                    self.state.culling_exit_requested = true;
                }
                if let Some(culling) = &mut self.state.culling {
                    let count = culling.paths.len().max(1);
                    if i.key_pressed(egui::Key::ArrowLeft) {
                        culling.focused = (culling.focused + count - 1) % count;
                    }
                    if i.key_pressed(egui::Key::ArrowRight) {
                        culling.focused = (culling.focused + 1) % count;
                    }
                    if i.key_pressed(egui::Key::Enter) {
                        self.state.culling_open_requested =
                            culling.paths.get(culling.focused).cloned();
                    }
                }
                if action(Action::Pick) {
                    self.state.culling_annotate =
                        Some(crate::state::CullingAnnotate::Flag(sp_catalog::Flag::Pick));
                }
                if action(Action::Reject) {
                    self.state.culling_annotate = Some(crate::state::CullingAnnotate::Flag(
                        sp_catalog::Flag::Reject,
                    ));
                }
                if action(Action::Unflag) {
                    self.state.culling_annotate =
                        Some(crate::state::CullingAnnotate::Flag(sp_catalog::Flag::None));
                }
                for (key, rating) in [
                    (egui::Key::Num0, 0),
                    (egui::Key::Num1, 1),
                    (egui::Key::Num2, 2),
                    (egui::Key::Num3, 3),
                    (egui::Key::Num4, 4),
                    (egui::Key::Num5, 5),
                ] {
                    if plain(key) {
                        self.state.culling_annotate =
                            Some(crate::state::CullingAnnotate::Rating(rating));
                    }
                }
                if action(Action::Zoom) {
                    self.state.toggle_zoom();
                }
                return;
            }
            if !widget_focused && !chord && i.key_pressed(egui::Key::Tab) {
                self.state.toggle_rails();
            }
            if action(Action::Zoom) {
                self.state.toggle_zoom();
            }
            if action(Action::Develop) {
                self.state.mode = Mode::Develop;
            }
            if action(Action::Library) {
                self.state.mode = Mode::Library;
            }
            if action(Action::Open) {
                open_dialog = true;
            }
            if action(Action::Shadows) {
                self.state.clipping_mode ^= 1;
            }
            if action(Action::Highlights) {
                self.state.clipping_mode ^= 2;
            }
            if action(Action::Redo) && !self.state.decode_in_flight {
                self.state.redo();
            } else if action(Action::Undo) && !self.state.decode_in_flight {
                self.state.undo();
            }
            if action(Action::Before) {
                self.state.show_original = !self.state.show_original;
                self.state.compare_mode = crate::state::CompareMode::Off;
            }
            if action(Action::CompareMode) && self.state.open_image.is_some() {
                self.state.compare_mode = self.state.compare_mode.next();
                self.state.show_original = false;
            }
            if action(Action::Export) && self.state.open_image.is_some() {
                self.state.export_dialog_open = true;
            }
            if i.key_pressed(egui::Key::Questionmark) {
                self.state.shortcuts_open = true;
            }
            if action(Action::Debug) {
                self.state.debug_overlay = !self.state.debug_overlay;
            }

            // Ratings, flags and labels: the grid selection in the Library,
            // the open photo in Develop.
            let mut annotate = None;
            for (key, rating) in [
                (egui::Key::Num0, 0),
                (egui::Key::Num1, 1),
                (egui::Key::Num2, 2),
                (egui::Key::Num3, 3),
                (egui::Key::Num4, 4),
                (egui::Key::Num5, 5),
            ] {
                if plain(key) {
                    annotate = Some(Annotate::Rating(rating));
                }
            }
            for (key, label) in [
                (egui::Key::Num6, sp_catalog::ColorLabel::Red),
                (egui::Key::Num7, sp_catalog::ColorLabel::Yellow),
                (egui::Key::Num8, sp_catalog::ColorLabel::Green),
                (egui::Key::Num9, sp_catalog::ColorLabel::Blue),
            ] {
                if !chord && i.key_pressed(key) {
                    annotate = Some(Annotate::Label(
                        if i.modifiers.shift && key == egui::Key::Num9 {
                            sp_catalog::ColorLabel::Purple
                        } else {
                            label
                        },
                    ));
                }
            }
            if action(Action::Pick) {
                annotate = Some(Annotate::Flag(sp_catalog::Flag::Pick));
            }
            // X swaps the crop ratio while cropping.
            if action(Action::Reject) && !self.state.crop_mode {
                annotate = Some(Annotate::Flag(sp_catalog::Flag::Reject));
            }
            if action(Action::Unflag) {
                annotate = Some(Annotate::Flag(sp_catalog::Flag::None));
            }

            if library {
                grid_annotate = annotate;
                if !widget_focused {
                    for (key, dx, dy) in [
                        (egui::Key::ArrowLeft, -1, 0),
                        (egui::Key::ArrowRight, 1, 0),
                        (egui::Key::ArrowUp, 0, -1),
                        (egui::Key::ArrowDown, 0, 1),
                    ] {
                        if i.key_pressed(key) {
                            grid_move =
                                Some((if dx != 0 { dx } else { dy * 1000 }, i.modifiers.shift));
                        }
                    }
                    if i.key_pressed(egui::Key::Enter) {
                        grid_open = true;
                    }
                    if i.key_pressed(egui::Key::Escape) {
                        self.state.selected_photos.clear();
                    }
                }
                if i.modifiers.command && i.key_pressed(egui::Key::A) {
                    grid_select_all = true;
                }
                return;
            }

            if self.state.open_image.is_some() && !self.state.decode_in_flight {
                match annotate {
                    Some(Annotate::Rating(rating)) => {
                        self.state.annotations.rating = rating;
                        self.state.sidecar_dirty = true;
                        self.state.last_edit_change = Some(std::time::Instant::now());
                    }
                    Some(Annotate::Flag(flag)) => {
                        self.state.annotations.flag = flag;
                        self.state.sidecar_dirty = true;
                    }
                    Some(Annotate::Label(label)) => {
                        let current = &mut self.state.descriptive_metadata.color_label;
                        *current = if *current == label {
                            sp_catalog::ColorLabel::None
                        } else {
                            label
                        };
                        self.state.sidecar_dirty = true;
                        self.state.last_edit_change = Some(std::time::Instant::now());
                    }
                    None => {}
                }
            }
            if !widget_focused {
                if i.key_pressed(egui::Key::ArrowLeft) {
                    navigate = -1;
                }
                if i.key_pressed(egui::Key::ArrowRight) {
                    navigate = 1;
                }
            }
        });

        if library {
            self.library_keys(ctx, grid_move, grid_annotate, grid_open, grid_select_all);
        }
        if self.state.annotations != before_annotations {
            if let Some(path) = &self.state.open_image {
                if let Some(photo) = self
                    .state
                    .catalog_photos
                    .iter_mut()
                    .find(|photo| &photo.path == path)
                {
                    photo.annotations = self.state.annotations;
                }
            }
        }
        if self.state.descriptive_metadata.color_label != before_color {
            if let Some(path) = &self.state.open_image {
                if let Some(photo) = self
                    .state
                    .catalog_photos
                    .iter_mut()
                    .find(|photo| &photo.path == path)
                {
                    photo.color_label = self.state.descriptive_metadata.color_label;
                }
            }
        }
        if navigate != 0 {
            let paths: Vec<_> = if self.state.catalog_photos.is_empty() {
                self.state.session.clone()
            } else {
                self.state
                    .catalog_photos
                    .iter()
                    .map(|photo| photo.path.clone())
                    .collect()
            };
            if let Some(current) = self
                .state
                .open_image
                .as_ref()
                .and_then(|path| paths.iter().position(|candidate| candidate == path))
            {
                let next = (current as i64 + i64::from(navigate))
                    .clamp(0, paths.len().saturating_sub(1) as i64)
                    as usize;
                if next != current {
                    self.state.filmstrip_clicked = Some(paths[next].clone());
                }
            }
        }
        if open_dialog {
            self.open_file_dialog();
        }

        // Resolve folders off the UI thread, using the same import enumerator as card import.
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .filter_map(|f| f.path.clone())
                .collect()
        });
        if !dropped.is_empty() {
            let context = ctx.clone();
            std::thread::spawn(move || {
                let batch = collect_dropped(&dropped);
                context.data_mut(|data| {
                    let batches = data
                        .get_temp_mut_or_default::<Vec<DropBatch>>(egui::Id::new("drop-imports"));
                    batches.push(batch);
                });
                context.request_repaint();
            });
        }
        let batches = ctx
            .data_mut(|data| data.remove_temp::<Vec<DropBatch>>(egui::Id::new("drop-imports")))
            .unwrap_or_default();
        let mut folders = Vec::new();
        for batch in batches {
            if let Some(path) = batch.files.first() {
                self.start_decode(path.clone(), ctx);
            }
            for path in batch.files {
                if !self.state.session.contains(&path) {
                    self.state.session.push(path);
                }
            }
            folders.extend(batch.folders);
            if batch.skipped > 0 {
                self.state.push_toast(
                    format!("Skipped {} unsupported files", batch.skipped),
                    false,
                );
            }
            for error in batch.errors {
                self.state.push_toast(error, true);
            }
        }
        // Queue scans while another catalog operation is in flight.
        ctx.data_mut(|data| {
            data.get_temp_mut_or_default::<Vec<PathBuf>>(egui::Id::new("drop-folders"))
                .extend(folders);
        });
        if self.catalog_job.is_none() && !self.state.catalog_tool_busy {
            let mut folders = ctx
                .data_mut(|data| data.remove_temp::<Vec<PathBuf>>(egui::Id::new("drop-folders")))
                .unwrap_or_default();
            folders.sort();
            folders.dedup();
            if !folders.is_empty() {
                self.refresh_catalog(folders, None);
            }
        }
    }

    /// Library grid keyboard: move focus (⇧ extends the selection), open,
    /// select all, and apply ratings/flags/labels to the selection.
    fn library_keys(
        &mut self,
        ctx: &egui::Context,
        grid_move: Option<(i32, bool)>,
        annotate: Option<Annotate>,
        open: bool,
        select_all: bool,
    ) {
        let order = self.state.library_visible.clone();
        if order.is_empty() {
            return;
        }
        if select_all {
            self.state.selected_photos = order.iter().cloned().collect();
        }
        if let Some((step, shift)) = grid_move {
            let columns = ctx
                .data(|d| d.get_temp::<usize>(egui::Id::new("library-columns")))
                .unwrap_or(1)
                .max(1) as i64;
            let delta = if step.abs() >= 1000 {
                i64::from(step.signum()) * columns
            } else {
                i64::from(step)
            };
            let current = self
                .state
                .grid_focus
                .as_ref()
                .and_then(|focus| order.iter().position(|p| p == focus))
                .map_or(-1, |index| index as i64);
            let next = if current < 0 {
                0
            } else {
                (current + delta).clamp(0, order.len() as i64 - 1)
            } as usize;
            let path = order[next].clone();
            crate::panels::grid_select(&mut self.state, &path, false, shift);
            ctx.data_mut(|d| d.insert_temp(egui::Id::new("grid-scroll-to-focus"), true));
        }
        if open {
            if let Some(focus) = self.state.grid_focus.clone() {
                self.state.filmstrip_clicked = Some(focus);
                self.state.mode = Mode::Develop;
            }
        }
        if let Some(change) = annotate {
            let mut targets: Vec<PathBuf> = order
                .iter()
                .filter(|path| self.state.selected_photos.contains(*path))
                .cloned()
                .collect();
            if targets.is_empty() {
                targets.extend(self.state.grid_focus.clone());
            }
            if !targets.is_empty() {
                let what = match change {
                    Annotate::Rating(0) => "Cleared the rating of".to_owned(),
                    Annotate::Rating(n) => {
                        let __l10n_args = (&("★".repeat(usize::from(n))),);
                        crate::i18n::format("Rated {0} ·", std::slice::from_ref(__l10n_args.0))
                    }
                    Annotate::Flag(sp_catalog::Flag::Pick) => "Picked".to_owned(),
                    Annotate::Flag(sp_catalog::Flag::Reject) => "Rejected".to_owned(),
                    Annotate::Flag(sp_catalog::Flag::None) => "Unflagged".to_owned(),
                    Annotate::Label(_) => "Labelled".to_owned(),
                };
                if targets.len() > 1 {
                    self.state.push_toast(
                        {
                            let __l10n_args = (&(targets.len()),);
                            crate::i18n::format(
                                "{0} {1} photos",
                                &[what.to_string(), __l10n_args.0.to_string()],
                            )
                        },
                        false,
                    );
                }
                self.state.library_annotate = Some((change, targets));
            }
        }
    }
}

#[derive(Clone, Default)]
struct DropBatch {
    files: Vec<PathBuf>,
    folders: Vec<PathBuf>,
    skipped: usize,
    errors: Vec<String>,
}

fn collect_dropped(paths: &[PathBuf]) -> DropBatch {
    let mut batch = DropBatch::default();
    for path in paths {
        if path.is_dir() {
            match sp_catalog::workflow::import_candidates(path) {
                Ok(files) => {
                    batch.files.extend(files);
                    batch.folders.push(path.clone());
                }
                Err(error) => batch.errors.push(error.to_string()),
            }
        } else if path.is_file() && sp_raw::is_supported_image(path) {
            batch.files.push(path.clone());
            if let Some(parent) = path.parent() {
                batch.folders.push(parent.to_path_buf());
            }
        } else {
            batch.skipped += 1;
        }
    }
    let mut seen = std::collections::HashSet::new();
    batch.files.retain(|path| seen.insert(path.clone()));
    batch
}

#[cfg(test)]
mod drop_tests {
    use super::*;

    #[test]
    fn mixed_drop_opens_first_supported_and_imports_files_and_folders() {
        let directory = tempfile::tempdir().unwrap();
        let folder = directory.path().join("folder");
        std::fs::create_dir(&folder).unwrap();
        let unsupported = directory.path().join("notes.txt");
        let first = directory.path().join("first.PNG");
        let second = folder.join("second.ARW");
        for path in [&unsupported, &first, &second] {
            std::fs::write(path, []).unwrap();
        }
        let batch = collect_dropped(&[unsupported, first.clone(), folder, first.clone()]);
        assert_eq!(batch.files, vec![first, second]);
        assert_eq!(batch.skipped, 1);
        assert!(batch.errors.is_empty());
    }
}
