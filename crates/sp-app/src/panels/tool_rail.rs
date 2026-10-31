//! Switchable Develop sidebar; actions use the existing application workers.
use super::*;
use crate::state::LeftRailTab;

pub(super) fn develop_rail(
    ui: &mut egui::Ui,
    state: &mut AppState,
    thumbs: &[Option<egui::TextureHandle>],
) {
    // A hover from a hidden preset must never remain on the viewport.
    state.preset_preview = None;
    state.mask_candidate_hover = None;
    // Line the switch up with the cards below, which sit left of the
    // scroll bar gutter.
    let width = ui.available_width() - ui.spacing().scroll.allocated_width();
    ui.allocate_ui(egui::vec2(width, 0.0), |ui| {
        segmented_tabs(
            ui,
            &mut state.left_rail_tab,
            &[
                (LeftRailTab::Presets, "Presets"),
                (LeftRailTab::Ai, "AI"),
                (LeftRailTab::Workflow, "Workflow"),
            ],
        );
    });
    ui.add_space(theme::SPACE);
    match state.left_rail_tab {
        LeftRailTab::Presets => chrome::presets_rail(ui, state, thumbs),
        LeftRailTab::Ai => {
            egui::ScrollArea::vertical()
                .id_salt("ai-rail")
                .auto_shrink([false, false])
                .show(ui, |ui| ai_tools(ui, state));
        }
        LeftRailTab::Workflow => {
            egui::ScrollArea::vertical()
                .id_salt("workflow-rail")
                .auto_shrink([false, false])
                .show(ui, |ui| workflow_tools(ui, state));
        }
    }
}

/// Full-width segmented control in the style of the Library/Develop switch.
/// Options share the width evenly; when a label would be cramped they wrap
/// into a balanced grid rather than leaving one option orphaned on a line.
fn segmented_tabs<T: Copy + PartialEq>(ui: &mut egui::Ui, value: &mut T, options: &[(T, &str)]) {
    const PAD: f32 = 3.0;
    const GAP: f32 = 2.0;
    const ROW_H: f32 = 24.0;
    let font = egui::FontId::proportional(12.0);
    let enabled = ui.is_enabled();
    let width = ui.available_width();
    let labels: Vec<_> = options
        .iter()
        .map(|(_, label)| {
            ui.painter().layout_no_wrap(
                crate::i18n::text(label).to_owned(),
                font.clone(),
                // Painted with the state colour below.
                egui::Color32::PLACEHOLDER,
            )
        })
        .collect();
    let fits = |columns: usize| {
        let cell = (width - 2.0 * PAD - GAP * (columns - 1) as f32) / columns as f32;
        labels.iter().all(|galley| galley.size().x + 8.0 <= cell)
    };
    let columns = if fits(options.len()) {
        options.len()
    } else {
        options.len().div_ceil(2)
    };
    let rows = options.len().div_ceil(columns);
    let height = 2.0 * PAD + rows as f32 * ROW_H + (rows - 1) as f32 * GAP;
    let (outer, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
    let radius = if rows == 1 { height / 2.0 } else { 8.0 };
    ui.painter().rect_filled(outer, radius, theme::BG_VIEWPORT);
    let cell_w = (width - 2.0 * PAD - GAP * (columns - 1) as f32) / columns as f32;
    for (index, ((option, label), galley)) in options.iter().zip(labels).enumerate() {
        let (column, row) = (index % columns, index / columns);
        let cell = egui::Rect::from_min_size(
            outer.min
                + egui::vec2(
                    PAD + column as f32 * (cell_w + GAP),
                    PAD + row as f32 * (ROW_H + GAP),
                ),
            egui::vec2(cell_w, ROW_H),
        );
        let response = ui.interact(cell, ui.id().with(("segment", index)), egui::Sense::click());
        let selected = *value == *option;
        let corner = if rows == 1 { ROW_H / 2.0 } else { 6.0 };
        if selected {
            ui.painter().rect_filled(cell, corner, theme::BG_HOVER);
        } else if enabled && response.hovered() {
            ui.painter()
                .rect_filled(cell, corner, egui::Color32::from_white_alpha(6));
        }
        let color = match (enabled, selected) {
            (false, _) => theme::TEXT_FAINT.gamma_multiply(0.7),
            (true, true) => theme::TEXT_PRIMARY,
            (true, false) => theme::TEXT_SECONDARY,
        };
        ui.painter()
            .galley(cell.center() - galley.size() / 2.0, galley, color);
        if enabled && response.hovered() && !selected {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        response.widget_info(|| {
            egui::WidgetInfo::selected(
                egui::WidgetType::SelectableLabel,
                enabled,
                selected,
                crate::i18n::text(label),
            )
        });
        if response.clicked() {
            *value = *option;
        }
    }
}

fn photo_ready(state: &AppState) -> bool {
    state.open_image.is_some()
        && !state.decode_in_flight
        && !state.enhancement_busy
        && !state.ai_denoise_busy
        && !state.auto_in_flight
        && !state.auto_finish_busy
        && !state.export_busy
}

fn note(ui: &mut egui::Ui, text: &str) {
    ui.label(
        egui::RichText::new(crate::i18n::text(text))
            .size(11.0)
            .color(theme::TEXT_SECONDARY),
    );
}

fn action(ui: &mut egui::Ui, enabled: bool, text: &str) -> bool {
    ui.add_enabled(
        enabled,
        egui::Button::new(crate::i18n::text(text))
            .wrap()
            .min_size(egui::vec2(ui.available_width(), 28.0)),
    )
    .clicked()
}

fn section(ui: &mut egui::Ui, title: &str, contents: impl FnOnce(&mut egui::Ui)) {
    ui.push_id(title, |ui| {
        egui::Frame::new()
            .fill(theme::BG_CARD)
            .corner_radius(theme::RADIUS_PANEL)
            .inner_margin(egui::Margin::same(8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(theme::section_text(crate::i18n::text(title)));
                ui.add_space(4.0);
                contents(ui);
            });
    });
    ui.add_space(theme::SPACE);
}

fn ai_tools(ui: &mut egui::Ui, state: &mut AppState) {
    let ready = photo_ready(state);
    section(ui, "AI selections", |ui| {
        ai_mask_controls(ui, state, false);
    });
    section(ui, "Photo finishing", |ui| {
        note(
            ui,
            "Balance the current photo or prepare a batch for review.",
        );
        if action(ui, ready, "Auto Balance") {
            state.auto_tone_requested = true;
        }
        if action(ui, ready, "Precompute Auto Finish") {
            state.auto_finish_requested = true;
        }
        if state.auto_finish_busy {
            ui.add(egui::ProgressBar::new(state.auto_finish_progress).show_percentage());
            ui.label(crate::i18n::text(&state.auto_finish_status));
            if action(ui, true, "Cancel after current analysis") {
                state.auto_finish_cancel_requested = true;
            }
        }
    });
    section(ui, "Camera AI denoise", |ui| {
        use sp_core::edit::CameraDenoiseMode;
        let mut mode = state.edit.nr.camera_denoise_mode;
        ui.add_enabled_ui(ready && state.ai_denoise_available, |ui| {
            segmented_tabs(
                ui,
                &mut mode,
                &[
                    (CameraDenoiseMode::Off, "Off"),
                    (CameraDenoiseMode::Cleanest, "Cleanest"),
                    (CameraDenoiseMode::PreserveDetail, "Preserve detail"),
                ],
            );
        });
        if mode != state.edit.nr.camera_denoise_mode {
            let mut edit = state.edit.clone();
            edit.nr.camera_denoise_mode = mode;
            state
                .apply_edit_step_labeled(edit, Some(crate::i18n::text("Camera AI denoise").into()));
        }
        if let Some(status) = &state.ai_denoise_status {
            note(ui, status);
        } else if !state.ai_denoise_available {
            note(
                ui,
                "Camera denoise needs a supported RAW and its installed model.",
            );
        }
        if action(ui, state.open_image.is_some(), "Detail & denoise controls") {
            state.inspector_query.clear();
            state.panels.detail = true;
            state.inspector_jump = Some(2);
        }
    });
    section(ui, "Enhancement setup", |ui| {
        note(ui, "Manage optional selection and super-resolution models.");
        if action(ui, true, "AI runtime") {
            open_runtime(state);
        }
        if action(ui, ready, "Choose upscale model…") {
            state.upscale_model_pick_requested = true;
        }
    });
}

pub(super) fn open_runtime(state: &mut AppState) {
    state.enhance_runtime.open = true;
    if !state.enhance_runtime.busy {
        state.enhance_runtime.action = Some(crate::app::enhance_runtime::Action::Check);
    }
}

/// Numbered headings for the AI selection flow; numbering follows the
/// controls actually shown for the chosen method.
struct Steps(usize);

impl Steps {
    fn next(&mut self, ui: &mut egui::Ui, title: &str) {
        self.0 += 1;
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::hover());
            ui.painter()
                .circle_filled(rect.center(), 9.0, theme::ACCENT);
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                self.0.to_string(),
                egui::FontId::proportional(11.0),
                theme::ON_ACCENT,
            );
            ui.add(
                egui::Label::new(
                    egui::RichText::new(crate::i18n::text(title))
                        .strong()
                        .color(theme::TEXT_PRIMARY),
                )
                .wrap(),
            );
        });
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Tone {
    Idle,
    Working,
    Action,
    Done,
    Warning,
}

/// What the selection flow is doing now and what the user should do next.
fn selection_status(state: &AppState) -> (Tone, String, String) {
    let text = |source: &str| crate::i18n::text(source).to_owned();
    if state.open_image.is_none() {
        return (
            Tone::Idle,
            text("Open a photo first"),
            text("AI selections work on the photo in the viewport."),
        );
    }
    if state.decode_in_flight {
        return (Tone::Working, text("Loading photo…"), String::new());
    }
    if state.enhancement_busy {
        let detail = if state.enhancement_status.is_empty() {
            text("Finding the selection…")
        } else {
            text(&state.enhancement_status)
        };
        return (
            Tone::Working,
            text("Working on it"),
            format!(
                "{detail}\n{}",
                crate::i18n::text("The result is highlighted in the preview when it is ready.")
            ),
        );
    }
    if state.mask_point_pick {
        return match state.mask_prompt_pick {
            None => (
                Tone::Action,
                text("Click in the photo"),
                text("Click the object you want to select. Esc cancels."),
            ),
            Some(true) => (
                Tone::Action,
                text("Click to add a keep point"),
                text("Click a part of the object that should be selected."),
            ),
            Some(false) => (
                Tone::Action,
                text("Click to add an exclude point"),
                text("Click an area that should not be selected."),
            ),
        };
    }
    if let Some(candidates) = &state.mask_candidates {
        if !state.mask_candidates_current() {
            return (
                Tone::Warning,
                text("Detections are out of date"),
                text("The photo or its edit changed. Run the detection again."),
            );
        }
        let chosen = candidates.items.iter().filter(|item| item.selected).count();
        return (
            Tone::Action,
            crate::i18n::format(
                "{0} found · {1} chosen",
                &[candidates.items.len().to_string(), chosen.to_string()],
            ),
            text("Each element is numbered and tinted in the preview. Click it there or tick it below, then create the layers."),
        );
    }
    if !state.mask_prompts.is_empty() {
        return (
            Tone::Action,
            crate::i18n::format("{0} points placed", &[state.mask_prompts.len().to_string()]),
            text("Build the selection, or add more keep and exclude points first."),
        );
    }
    if let Some(layer) = state
        .retouch_active_local
        .and_then(|index| state.edit.local_adjustments.get(index))
    {
        return (
            Tone::Done,
            crate::i18n::format("Selection ready: {0}", std::slice::from_ref(&layer.name)),
            if state.mask_show_overlay {
                text("The pink area in the preview is selected. Edit it with Local & retouch on the right.")
            } else {
                text("Turn on \"Show mask in preview\" below to see what is selected.")
            },
        );
    }
    if !state.enhancement_status.is_empty() {
        return (
            Tone::Idle,
            text("Last result"),
            text(&state.enhancement_status),
        );
    }
    (
        Tone::Idle,
        text("Ready to select"),
        text("Choose a method below. Each selection becomes its own adjustment layer, so edits only change what is selected."),
    )
}

fn status_card(ui: &mut egui::Ui, state: &mut AppState) {
    let (tone, title, detail) = selection_status(state);
    let color = match tone {
        Tone::Idle => theme::TEXT_FAINT,
        Tone::Working | Tone::Action => theme::ACCENT,
        Tone::Done => theme::SUCCESS,
        Tone::Warning => theme::WARNING,
    };
    egui::Frame::new()
        .fill(theme::BG_RAISED)
        .stroke(egui::Stroke::new(1.0, color.gamma_multiply(0.7)))
        .corner_radius(theme::RADIUS_CONTROL)
        .inner_margin(egui::Margin::same(8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                if tone == Tone::Working {
                    ui.spinner();
                } else {
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
                    ui.painter().circle_filled(rect.center(), 4.0, color);
                }
                ui.label(
                    egui::RichText::new(title)
                        .strong()
                        .color(theme::TEXT_PRIMARY),
                );
            });
            if !detail.is_empty() {
                ui.label(
                    egui::RichText::new(detail)
                        .size(11.0)
                        .color(theme::TEXT_SECONDARY),
                );
            }
            if state.enhancement_busy && action(ui, true, "Cancel") {
                state.enhancement_cancel_requested = true;
            }
            if state.mask_point_pick && action(ui, true, "Stop picking") {
                state.mask_point_pick = false;
                state.mask_prompt_pick = None;
            }
        });
}

fn close(ui: &mut egui::Ui, close_menu: bool) {
    if close_menu {
        ui.close();
    }
}

/// Shared with Tools → AI masks, so both entry points retain the same guards.
pub(super) fn ai_mask_controls(ui: &mut egui::Ui, state: &mut AppState, close_menu: bool) {
    use crate::state::MaskMethod;
    state.mask_candidate_hover = None;
    let ready = photo_ready(state);
    status_card(ui, state);
    let mut steps = Steps(0);
    ui.add_enabled_ui(ready, |ui| {
        steps.next(ui, "Choose how to select");
        segmented_tabs(
            ui,
            &mut state.mask_method,
            &MaskMethod::ALL.map(|method| (method, method.label())),
        );
        note(ui, state.mask_method.hint());
        match state.mask_method {
            MaskMethod::Subject => subject_steps(ui, state, &mut steps, close_menu),
            MaskMethod::Describe => describe_steps(ui, state, &mut steps, close_menu),
            MaskMethod::Click => click_steps(ui, state, &mut steps, close_menu),
            MaskMethod::Depth => depth_steps(ui, state, &mut steps, close_menu),
        }
    });
    let active_layer = state
        .retouch_active_local
        .and_then(|index| state.edit.local_adjustments.get(index))
        .map(|layer| layer.name.clone());
    if let Some(name) = active_layer {
        steps.next(ui, "Check and edit the selection");
        ui.label(
            egui::RichText::new(crate::i18n::format("Layer: {0}", &[name]))
                .size(11.5)
                .color(theme::TEXT_SECONDARY),
        );
        let shortcut = crate::app::customize::label_for(
            &state.customize.bindings,
            "Show AI mask coverage",
            "M",
        );
        ui.checkbox(
            &mut state.mask_show_overlay,
            crate::i18n::format("Show mask in preview  {0}", &[shortcut]),
        );
        note(
            ui,
            "Pink = selected. Use the layer's sliders to change only that area, or refine its edges with the brush.",
        );
        if action(ui, true, "Open in Local & retouch") {
            state.panels.local_retouch = true;
            state.inspector_query.clear();
            state.inspector_jump = None;
            state.inspector_focus_card = Some("Local & retouch".into());
            close(ui, close_menu);
        }
    }
    ui.add_space(6.0);
    ui.separator();
    ui.add_enabled_ui(ready, |ui| {
        note(ui, "Already have a mask from another app?");
        if action(ui, true, "Import full-resolution alpha PNG…") {
            state.mask_import_requested = true;
            close(ui, close_menu);
        }
    });
}

fn subject_steps(ui: &mut egui::Ui, state: &mut AppState, steps: &mut Steps, close_menu: bool) {
    use crate::state::MaskSubject;
    steps.next(ui, "Pick what to find");
    egui::ComboBox::from_id_salt("ai-mask-subject")
        .selected_text(crate::i18n::text(state.mask_subject.label()))
        .width(ui.available_width() - 8.0)
        .show_ui(ui, |ui| {
            for subject in MaskSubject::ALL {
                if ui
                    .selectable_value(
                        &mut state.mask_subject,
                        subject,
                        crate::i18n::text(subject.label()),
                    )
                    .changed()
                {
                    state.mask_candidates = None;
                }
            }
        });
    let sky = state.mask_subject == MaskSubject::Sky;
    if sky {
        steps.next(ui, "Select it");
        note(ui, "The whole sky becomes one adjustment layer.");
        if action(ui, true, "Select the sky") {
            state.enhancement_requested = Some(state.mask_subject.request_kind().into());
            close(ui, close_menu);
        }
        return;
    }
    steps.next(ui, "Find them in the photo");
    note(
        ui,
        "Each person or vehicle is detected separately so you can choose which ones to edit.",
    );
    let has_candidates = state.mask_candidates.is_some();
    if action(
        ui,
        true,
        if has_candidates {
            "Detect again"
        } else {
            "Detect elements"
        },
    ) {
        state.enhancement_requested = Some(state.mask_subject.request_kind().into());
        close(ui, close_menu);
    }
    if !has_candidates {
        return;
    }
    steps.next(ui, "Choose which to edit");
    if !state.mask_candidates_current() {
        note(ui, "Image or edit changed; detect elements again.");
        return;
    }
    let Some(candidates) = &mut state.mask_candidates else {
        return;
    };
    ui.horizontal_wrapped(|ui| {
        if ui.small_button(crate::i18n::text("Select all")).clicked() {
            candidates
                .items
                .iter_mut()
                .for_each(|item| item.selected = true);
        }
        if ui.small_button(crate::i18n::text("Select none")).clicked() {
            candidates
                .items
                .iter_mut()
                .for_each(|item| item.selected = false);
        }
    });
    let mut hover = None;
    egui::ScrollArea::vertical()
        .id_salt("element-candidates")
        .max_height(300.0)
        .show(ui, |ui| {
            for (index, candidate) in candidates.items.iter_mut().enumerate() {
                let row = ui.horizontal(|ui| {
                    let texture = candidate.texture.get_or_insert_with(|| {
                        ui.ctx().load_texture(
                            format!("element-{}-{}", candidates.generation, candidate.id),
                            candidate.preview.clone(),
                            egui::TextureOptions::LINEAR,
                        )
                    });
                    let color = theme::candidate_color(index);
                    let image =
                        ui.add(egui::Image::new(&*texture).max_size(egui::vec2(64.0, 64.0)));
                    ui.painter().rect_stroke(
                        image.rect,
                        2.0,
                        egui::Stroke::new(if candidate.selected { 2.0 } else { 1.0 }, color),
                        egui::StrokeKind::Outside,
                    );
                    ui.checkbox(
                        &mut candidate.selected,
                        crate::i18n::format(
                            "#{0} · {1}% sure",
                            &[
                                (index + 1).to_string(),
                                format!("{:.0}", candidate.confidence * 100.0),
                            ],
                        ),
                    );
                });
                if row.response.contains_pointer() {
                    hover = Some(index);
                }
            }
        });
    let selected: Vec<_> = candidates
        .items
        .iter()
        .filter(|item| item.selected)
        .map(|item| item.id.to_string())
        .collect();
    let request = format!("selected:{}:{}", candidates.kind, selected.join(","));
    state.mask_candidate_hover = hover;
    let label = match selected.len() {
        0 => crate::i18n::text("Tick at least one element").to_owned(),
        1 => crate::i18n::text("Create 1 adjustment layer").to_owned(),
        count => crate::i18n::format("Create {0} adjustment layers", &[count.to_string()]),
    };
    if action(ui, !selected.is_empty(), &label) {
        state.enhancement_requested = Some(request);
        close(ui, close_menu);
    }
}

fn describe_steps(ui: &mut egui::Ui, state: &mut AppState, steps: &mut Steps, close_menu: bool) {
    steps.next(ui, "Describe what to select");
    ui.add(
        egui::TextEdit::singleline(&mut state.mask_text_prompt)
            .hint_text(crate::i18n::text("the red car, trees"))
            .char_limit(256)
            .desired_width(ui.available_width()),
    );
    note(ui, "Separate several things with commas (up to 8).");
    let prompts: Vec<_> = state.mask_text_prompt.split(',').collect();
    let valid = state.mask_text_prompt.chars().count() <= 256
        && prompts.len() <= 8
        && prompts.iter().all(|part| !part.trim().is_empty());
    steps.next(ui, "Select it");
    if action(ui, valid, "Select described objects") {
        state.enhancement_requested = Some("text".into());
        close(ui, close_menu);
    }
}

fn click_steps(ui: &mut egui::Ui, state: &mut AppState, steps: &mut Steps, close_menu: bool) {
    steps.next(ui, "Click the object");
    note(ui, "Your click selects the whole object under the pointer.");
    if action(ui, true, "Click an object in the photo") {
        state.mask_point_pick = true;
        state.mask_prompt_pick = None;
        close(ui, close_menu);
    }
    steps.next(ui, "Refine with points");
    note(ui, "Optional. Green keep points add to the selection; red exclude points remove areas from it.");
    for (label, foreground) in [("Add keep point", true), ("Add exclude point", false)] {
        if action(ui, state.mask_prompts.len() < 32, label) {
            state.mask_point_pick = true;
            state.mask_prompt_pick = Some(foreground);
            close(ui, close_menu);
        }
    }
    if !state.mask_prompts.is_empty() {
        let keep = state
            .mask_prompts
            .iter()
            .filter(|(_, include)| *include)
            .count();
        ui.horizontal_wrapped(|ui| {
            ui.label(
                egui::RichText::new(crate::i18n::format(
                    "{0} keep · {1} exclude",
                    &[
                        keep.to_string(),
                        (state.mask_prompts.len() - keep).to_string(),
                    ],
                ))
                .size(11.5)
                .color(theme::TEXT_SECONDARY),
            );
            if ui.small_button(crate::i18n::text("Undo point")).clicked() {
                state.mask_prompts.pop();
            }
            if ui.small_button(crate::i18n::text("Clear points")).clicked() {
                state.mask_prompts.clear();
            }
        });
        if action(ui, keep > 0, "Build new selection from points") {
            state.enhancement_requested = Some("object".into());
            close(ui, close_menu);
        }
        let can_refine = keep > 0
            && state
                .retouch_active_local
                .and_then(|index| state.edit.local_adjustments.get(index))
                .is_some_and(|layer| matches!(layer.mask, sp_core::edit::LocalMask::Alpha(_)));
        if action(ui, can_refine, "Refine the current layer with points") {
            state.enhancement_requested = Some("refine_object".into());
            close(ui, close_menu);
        }
        if keep == 0 {
            note(ui, "Add at least one keep point first.");
        } else {
            note(
                ui,
                "Refining keeps the layer's edits and only replaces its mask.",
            );
        }
    }
    ui.collapsing(crate::i18n::text("Exact coordinates"), |ui| {
        ui.horizontal(|ui| {
            ui.label(crate::i18n::text("Point"));
            for coordinate in &mut state.mask_point {
                ui.add(
                    egui::DragValue::new(coordinate)
                        .range(0.0..=1.0)
                        .speed(0.01),
                );
            }
        });
        if action(ui, true, "Select object at point") {
            state.mask_prompts.clear();
            state.enhancement_requested = Some("object".into());
            close(ui, close_menu);
        }
    });
}

fn depth_steps(ui: &mut egui::Ui, state: &mut AppState, steps: &mut Steps, close_menu: bool) {
    steps.next(ui, "Set the distance range");
    note(ui, "0 is the farthest point in the photo, 1 the nearest.");
    ui.add(egui::Slider::new(&mut state.mask_depth_min, 0.0..=1.0).text(crate::i18n::text("From")));
    ui.add(egui::Slider::new(&mut state.mask_depth_max, 0.0..=1.0).text(crate::i18n::text("To")));
    steps.next(ui, "Select it");
    if action(
        ui,
        state.mask_depth_min <= state.mask_depth_max,
        "Select depth range",
    ) {
        state.enhancement_requested = Some("depth".into());
        close(ui, close_menu);
    }
    if state.mask_depth_min > state.mask_depth_max {
        note(ui, "\"From\" must not be larger than \"To\".");
    }
}

fn workflow_tools(ui: &mut egui::Ui, state: &mut AppState) {
    let ready = photo_ready(state);
    section(ui, "Edit & organize", |ui| {
        if action(ui, ready, "Photo workflows…") {
            state.workflow.open = true;
        }
        if action(ui, ready, "History & versions") {
            state.inspector_query.clear();
            state.panels.history = true;
            state.inspector_jump = Some(0);
        }
        if action(
            ui,
            !state.merge.busy && !state.selected_photos.is_empty(),
            "Photo merge",
        ) {
            state.merge.reset_focus();
            state.merge.sources = state.selected_photos.iter().cloned().collect();
            state.merge.sources.sort();
            state.merge.reference = state.merge.sources.len() / 2;
            state.merge.exposure_values.clear();
            state.merge.status.clear();
            state.merge.open = true;
        }
        if action(ui, true, "Lens profiles and calibration…") {
            state.lens_manager.open = true;
        }
    });
    section(ui, "Delivery", |ui| {
        if action(ui, ready, "Export current photo…") {
            state.export_batch = false;
            state.export_dialog_open = true;
        }
        if action(
            ui,
            ready && !state.selected_photos.is_empty(),
            "Export selected…",
        ) {
            state.export_batch = true;
            state.export_dialog_open = true;
        }
        if action(ui, true, "Export queue…") {
            state.export_queue_open = true;
        }
        if action(ui, ready, "Print / contact sheet…") {
            state.print_dialog_open = true;
        }
        if action(ui, true, "Proof gallery / slideshow…") {
            state.gallery.open = true;
        }
    });
    section(ui, "Workspace", |ui| {
        if action(ui, true, "Workspaces, shortcuts & accessibility…") {
            state.customize.open = true;
        }
        if action(ui, true, "Command console") {
            state.command_console.open = true;
        }
        if action(ui, true, "System & photo capabilities…") {
            state.diagnostics_open = true;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(
        ctx: &egui::Context,
        state: &mut AppState,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    // Tall enough for every AI rail section without scrolling.
                    egui::vec2(1024.0, 1800.0),
                )),
                events,
                ..Default::default()
            },
            |ui| {
                egui::Panel::left("rail-test")
                    .exact_size(theme::LEFT_RAIL_W)
                    .resizable(false)
                    .show(ui, |ui| chrome::left_rail(ui, state, &[]));
            },
        )
    }

    fn text_position(shape: &egui::Shape, label: &str) -> Option<egui::Pos2> {
        match shape {
            egui::Shape::Text(text) if text.galley.job.text == label => {
                Some(text.pos + text.galley.rect.center().to_vec2())
            }
            egui::Shape::Vec(shapes) => shapes.iter().find_map(|shape| text_position(shape, label)),
            _ => None,
        }
    }

    fn click(ctx: &egui::Context, state: &mut AppState, label: &str) {
        let _ = frame(ctx, state, vec![]);
        let output = frame(ctx, state, vec![]);
        let position = output
            .shapes
            .iter()
            .find_map(|shape| text_position(&shape.shape, label))
            .unwrap_or_else(|| panic!("No visible control {label}"));
        for pressed in [true, false] {
            let _ = frame(
                ctx,
                state,
                vec![
                    egui::Event::PointerMoved(position),
                    egui::Event::PointerButton {
                        pos: position,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: Default::default(),
                    },
                ],
            );
        }
    }

    #[test]
    fn sidebar_switches_tools_and_blocks_edits_during_decode() {
        crate::i18n::set_language(crate::i18n::Language::English);
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let mut state = AppState {
            open_image: Some("photo.png".into()),
            preset_preview: Some(sp_core::preset::DevelopPreset::ALL[0]),
            ..Default::default()
        };
        let edit = state.edit.clone();
        click(&ctx, &mut state, "AI");
        assert_eq!(state.left_rail_tab, LeftRailTab::Ai);
        assert!(state.preset_preview.is_none());
        click(&ctx, &mut state, "Auto Balance");
        assert!(state.auto_tone_requested);
        state.auto_tone_requested = false;
        state.decode_in_flight = true;
        click(&ctx, &mut state, "Auto Balance");
        assert!(!state.auto_tone_requested);
        state.decode_in_flight = false;
        click(&ctx, &mut state, "Workflow");
        assert_eq!(state.left_rail_tab, LeftRailTab::Workflow);
        state.export_batch = true;
        click(&ctx, &mut state, "Export current photo…");
        assert!(state.export_dialog_open);
        assert!(!state.export_batch);
        assert_eq!(state.edit, edit);
    }

    #[test]
    fn click_method_arms_exclude_points_and_reports_the_next_step() {
        crate::i18n::set_language(crate::i18n::Language::English);
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let mut state = AppState {
            open_image: Some("photo.png".into()),
            left_rail_tab: LeftRailTab::Ai,
            ..Default::default()
        };
        assert_eq!(selection_status(&state).1, "Ready to select");
        click(&ctx, &mut state, "Click");
        assert_eq!(state.mask_method, crate::state::MaskMethod::Click);
        click(&ctx, &mut state, "Add exclude point");
        assert!(state.mask_point_pick);
        assert_eq!(state.mask_prompt_pick, Some(false));
        assert_eq!(selection_status(&state).1, "Click to add an exclude point");
        click(&ctx, &mut state, "Stop picking");
        assert!(!state.mask_point_pick);
        state.mask_prompts.push(([0.5, 0.5], false));
        // Exclusion-only prompts cannot build a selection.
        click(&ctx, &mut state, "Build new selection from points");
        assert!(state.enhancement_requested.is_none());
        state.mask_prompts.push(([0.4, 0.4], true));
        click(&ctx, &mut state, "Build new selection from points");
        assert_eq!(state.enhancement_requested.as_deref(), Some("object"));
    }

    #[test]
    fn camera_denoise_is_gated_and_one_undo_step_from_sidebar() {
        crate::i18n::set_language(crate::i18n::Language::English);
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let mut state = AppState {
            open_image: Some("photo.3fr".into()),
            left_rail_tab: LeftRailTab::Ai,
            ..Default::default()
        };
        let edit = state.edit.clone();
        click(&ctx, &mut state, "Cleanest");
        assert_eq!(state.edit, edit);
        state.ai_denoise_available = true;
        click(&ctx, &mut state, "Cleanest");
        assert_eq!(
            state.edit.nr.camera_denoise_mode,
            sp_core::edit::CameraDenoiseMode::Cleanest
        );
        state.undo();
        assert_eq!(state.edit, edit);
    }
}
