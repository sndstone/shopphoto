//! Top bar, left rail and the small shared chrome widgets.

use super::*;

pub(super) fn set_crop_aspect(edit: &mut sp_core::EditState, image: (u32, u32), aspect: f32) {
    let image_aspect = image.0 as f32 / image.1.max(1) as f32;
    edit.geometry.crop = if image_aspect > aspect {
        let width = aspect / image_aspect;
        [(1.0 - width) * 0.5, 0.0, (1.0 + width) * 0.5, 1.0]
    } else {
        let height = image_aspect / aspect;
        [0.0, (1.0 - height) * 0.5, 1.0, (1.0 + height) * 0.5]
    };
}

pub fn top_bar(ui: &mut egui::Ui, state: &mut AppState) {
    ui.horizontal_centered(|ui| {
        ui.add_space(theme::SPACE * 1.5);
        ui.label(
            egui::RichText::new(crate::i18n::text("Shop"))
                .size(16.5)
                .strong()
                .color(theme::TEXT_PRIMARY),
        );
        ui.add_space(-7.0);
        ui.label(
            egui::RichText::new(crate::i18n::text("Photo"))
                .size(16.5)
                .strong()
                .color(theme::ACCENT),
        );
        ui.add_space(theme::SPACE * 2.5);

        segmented(ui, &mut state.mode);
        if state.source_offline {
            ui.label(egui::RichText::new(crate::i18n::text("Offline copy")).small().color(theme::WARNING)).on_hover_text(crate::i18n::text("The original is unavailable. This photo is being edited from a verified local copy."));
        } else if state.source_pinned {
            ui.label(egui::RichText::new(crate::i18n::text("Available offline")).small().color(theme::TEXT_SECONDARY));
        }
        ui.add_space(theme::SPACE * 1.5);
        if pill_button(ui, crate::i18n::tr(state.language, crate::i18n::Text::Open), false).clicked() {
            state.open_dialog_requested = true;
        }
        ui.add_space(theme::SPACE);
        if let Some(culling) = &mut state.culling {
            let done = pill_button(ui, "Done comparing", true);
            if done.clicked() {
                state.culling_exit_requested = true;
            }
            done.on_hover_text(crate::i18n::text("Leave the side-by-side view (Esc)"));
            ui.checkbox(&mut culling.locked, crate::i18n::text("Lock pan & zoom"));
        } else if (2..=4).contains(&state.selected_photos.len()) {
            let compare = pill_button(ui, "Compare", false);
            if compare.clicked() {
                state.culling_requested = true;
            }
            compare.on_hover_text(crate::i18n::text("Show the selected photos side by side for culling (C)"));
        }

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_space(theme::SPACE * 1.5);
            let export = pill_button(ui, crate::i18n::tr(state.language, crate::i18n::Text::Export), state.open_image.is_some());
            if export.clicked() && state.open_image.is_some() {
                state.export_dialog_open = true;
            }
            export.on_hover_text(crate::i18n::text("Export the current developed image (E)"));
            ui.menu_button(crate::i18n::text("Tools"), |ui| {
                ui.set_width(220.0);
                if ui.button(crate::i18n::text("Lens profiles and calibration…")).clicked(){state.lens_manager.open=true;ui.close();}
                if ui.add_enabled(state.open_image.is_some(),egui::Button::new(crate::i18n::text("Print / contact sheet…"))).clicked(){state.print_dialog_open=true;ui.close();}
                if ui.button(crate::i18n::text("Export queue…")).clicked(){state.export_queue_open=true;ui.close();}
                if ui.button(crate::i18n::text("Settings…")).clicked(){state.customize.open=true;ui.close();}
                if ui.button(crate::i18n::text("System & photo capabilities…")).clicked(){state.diagnostics_open=true;ui.close();}
                if ui.button(crate::i18n::text("Proof gallery / slideshow…")).clicked(){state.gallery.open=true;ui.close();}
                if ui.add_enabled(state.open_image.is_some(),egui::Button::new(crate::i18n::text("Photo workflows…"))).clicked(){state.workflow.open=true;ui.close();}
                ui.menu_button(crate::i18n::text("Library & offline"), |ui| {
                    use crate::app::catalog_tools::Action;
                    let idle = !state.catalog_tool_busy && !state.catalog_scanning;
                    for (label, action) in [("Back up catalog…", Action::Backup), ("Restore catalog from backup…", Action::Restore), ("Check catalog integrity", Action::CheckIntegrity)] {
                        if ui.add_enabled(idle, egui::Button::new(crate::i18n::text(label))).clicked() { state.catalog_tool_requested = Some(action); ui.close(); }
                    }
                    ui.separator();
                    let selected = state.open_image.is_some() || !state.selected_photos.is_empty();
                    if ui.add_enabled(idle,egui::Button::new(crate::i18n::text("Review automatic photo stacks…"))).clicked(){state.auto_stack_open=true;state.auto_stack_span_s=state.auto_stack_span_s.max(2);ui.close();}
                    if ui.add_enabled(idle && selected,egui::Button::new(crate::i18n::text("Analyze selected previews for culling"))).clicked(){state.catalog_tool_requested=Some(Action::AnalyzeSelected);ui.close();}
                    if ui.add_enabled(idle && selected,egui::Button::new(crate::i18n::text("Analyze selected faces / eye detail (optional runtime)"))).clicked(){state.catalog_tool_requested=Some(Action::AnalyzeFacesSelected);ui.close();}
                    if ui.add_enabled(idle && selected,egui::Button::new(crate::i18n::text("Package selected photos…"))).on_hover_text(crate::i18n::text("Copy verified originals, edits, versions and catalog groupings into a movable project folder.")).clicked() {state.catalog_tool_requested=Some(Action::PackageSelected);ui.close();}
                    if ui.add_enabled(idle,egui::Button::new(crate::i18n::text("Open portable project…"))).clicked() {state.catalog_tool_requested=Some(Action::OpenProject);ui.close();}
                    if ui.add_enabled(idle && state.selected_photos.len() >= 2, egui::Button::new(crate::i18n::text("Stack selected photos"))).clicked() { state.catalog_tool_requested = Some(Action::StackSelected); ui.close(); }
                    for (label, action) in [("Keep selected photos offline", Action::PinSelected), ("Release selected offline copies", Action::UnpinSelected)] {
                        if ui.add_enabled(idle && selected, egui::Button::new(label)).clicked() { state.catalog_tool_requested = Some(action); ui.close(); }
                    }
                    ui.separator();
                    if ui.add_enabled(idle && state.open_image.is_some(), egui::Button::new(crate::i18n::text("Reload metadata from XMP"))).on_hover_text(crate::i18n::text("Replace ratings and descriptive metadata with the current XMP fields. Photo adjustments are retained.")).clicked() { state.catalog_tool_requested = Some(Action::ReloadXmp); ui.close(); }
                    if !state.catalog_folders.is_empty() {
                        ui.menu_button(crate::i18n::text("Relink moved folder"), |ui| {
                            for folder in &state.catalog_folders {
                                if ui.add_enabled(idle, egui::Button::new(folder.path.display().to_string())).clicked() { state.catalog_tool_requested = Some(Action::RelinkFolder(folder.id)); ui.close(); }
                            }
                        });
                    }
                    if state.catalog_tool_busy { ui.spinner(); ui.label(crate::i18n::text(&state.catalog_tool_status)); }
                });
            if state.open_image.is_some() {
                ui.menu_button(crate::i18n::text("External editor"), |ui| {
                    ui.selectable_value(&mut state.external.selected,None,crate::i18n::text("Default 16-bit TIFF"));
                    for (index,recipe) in state.external.recipes.iter().enumerate() {ui.selectable_value(&mut state.external.selected,Some(index),&recipe.name);}
                    if ui.button(crate::i18n::text("Edit a TIFF copy…")).clicked() {
                        state.external_edit_requested = true;
                        ui.close();
                    }
                    if ui.button(crate::i18n::text("Choose editor…")).clicked() {
                        state.external_editor_pick_requested = true;
                        ui.close();
                    }
                    if ui.button(crate::i18n::text("Manage recipes…")).clicked() {state.external.edit_selected();state.external.open=true;ui.close();}
                    if state.external.monitoring && ui.button(crate::i18n::text("Stop watching external saves")).clicked() {state.external.stop_requested=true;ui.close();}
                    let editor = state
                        .external_editor
                        .as_ref()
                        .and_then(|path| path.file_name())
                        .map_or_else(|| "System default".into(), |name| name.to_string_lossy());
                    ui.label(
                        egui::RichText::new(crate::i18n::format("Editor: {0}", &[editor.to_string()]))
                            .small()
                            .color(theme::TEXT_SECONDARY),
                    );
                });
            }
            if ui.add_enabled(!state.sequence.busy && state.selected_photos.len() >= 2, egui::Button::new(crate::i18n::text("Match sequence"))).clicked() {
                state.sequence.sources = state.selected_photos.iter().cloned().collect();
                state.sequence.sources.sort();
                state.sequence.reference = state.sequence.sources.len()/2;
                state.sequence.proposals.clear();state.sequence.errors.clear();state.sequence.status.clear();
                state.sequence.open = true;
            }
            if ui.button(crate::i18n::text("Tether capture")).clicked() {state.tether.open=true;}
            if ui.button(crate::i18n::text("Command console")).clicked() { state.command_console.open = true; }
            if ui.button(crate::i18n::text("AI runtime")).clicked() {
                state.enhance_runtime.open = true;
                if !state.enhance_runtime.busy { state.enhance_runtime.action = Some(crate::app::enhance_runtime::Action::Check); }
            }
            if ui.add_enabled(!state.merge.busy, egui::Button::new(crate::i18n::text("Photo merge"))).clicked() {
                state.merge.reset_focus();
                state.merge.sources = state.selected_photos.iter().cloned().collect();
                state.merge.sources.sort();
                state.merge.reference = state.merge.sources.len()/2;
                state.merge.exposure_values.clear();
                state.merge.status.clear();
                state.merge.open = true;
            }
            if state.open_image.is_some() {
                    ui.menu_button(crate::i18n::text("AI masks"), |ui| {
                        ui.set_width(320.0);
                    super::tool_rail::ai_mask_controls(ui, state, true);
                });
                    ui.menu_button(crate::i18n::text("Batch"), |ui| {
                        ui.set_width(320.0);
                    ui.label(crate::i18n::text("Prepare selected photographs for review"));
                    ui.label(egui::RichText::new(crate::i18n::text("Auto Balance runs separately on every file using its existing profile and look.")).small());
                    if ui.add_enabled(!state.auto_finish_busy && !state.export_busy, egui::Button::new(crate::i18n::text("Precompute Auto Finish"))).clicked() {
                        state.auto_finish_requested = true;
                        ui.close();
                    }
                    if state.auto_finish_busy {
                        ui.add(egui::ProgressBar::new(state.auto_finish_progress).show_percentage());
                        ui.label(crate::i18n::text(&state.auto_finish_status));
                        if ui.button(crate::i18n::text("Cancel after current analysis")).clicked() {
                            state.auto_finish_cancel_requested = true;
                        }
                    } else if !state.auto_finish_status.is_empty() {
                        ui.label(crate::i18n::text(&state.auto_finish_status));
                    }
                    if !state.auto_finish_errors.is_empty() {
                        ui.collapsing(crate::i18n::text("Items requiring review"), |ui| {
                            egui::ScrollArea::vertical().max_height(200.0).show(ui, |ui| {
                                for error in &state.auto_finish_errors { ui.label(error); }
                            });
                        });
                    }
                    if ui.add_enabled(!state.auto_finish_busy && !state.export_busy, egui::Button::new(crate::i18n::text("Export selected…"))).clicked() {
                        state.export_batch = true;
                        state.export_dialog_open = true;
                        ui.close();
                    }
                });
            }
            if state.open_image.is_some() && state.selected_photos.len() > 1 {
                ui.menu_button(crate::i18n::text("Sync"), |ui| {
                    ui.checkbox(&mut state.settings_groups.tone, crate::i18n::text("Tone & curves"));
                    ui.checkbox(&mut state.settings_groups.color, crate::i18n::text("Color"));
                    ui.checkbox(&mut state.settings_groups.detail, crate::i18n::text("Detail & denoise"));
                    ui.checkbox(&mut state.settings_groups.lens, crate::i18n::text("Lens"));
                    ui.checkbox(&mut state.settings_groups.effects, crate::i18n::text("Effects"));
                    ui.checkbox(&mut state.settings_groups.geometry, crate::i18n::text("Geometry"));
                    ui.checkbox(&mut state.settings_groups.white_balance, crate::i18n::text("White balance"));
                    if ui.button(crate::i18n::text("Sync from active")).clicked() {
                        state.sync_settings_requested = true;
                        ui.close();
                    }
                    if ui.button(crate::i18n::text("Undo last sync")).clicked() {
                        state.undo_sync_requested = true;
                    }
                });
            }
            });
            ui.add_space(theme::SPACE * 0.5);
            let view_modified = state.proof_space != sp_core::edit::ProofSpace::DisplayNative
                || state.gamut_warning
                || state.clipping_mode != 0;
            let proof_summary = match state.proof_space {
                sp_core::edit::ProofSpace::DisplayNative => "Display native".to_owned(),
                sp_core::edit::ProofSpace::Srgb => "sRGB".to_owned(),
                sp_core::edit::ProofSpace::DisplayP3 => "Display P3".to_owned(),
                sp_core::edit::ProofSpace::Rec2020 => "Rec.2020".to_owned(),
                sp_core::edit::ProofSpace::Custom => state.selected_custom_proof.clone().unwrap_or_else(|| "Custom ICC".to_owned()),
            };
            let clipping_summary = match state.clipping_mode {
                1 => "shadows",
                2 => "highlights",
                3 => "shadows + highlights",
                _ => "off",
            };
            let view_label = crate::i18n::tr(state.language, crate::i18n::Text::View);
            let view_menu = ui.menu_button(if view_modified { format!("{view_label}  •") } else { view_label.to_owned() }, |ui| {
                ui.label(theme::section_text("Soft proof"));
                ui.selectable_value(
                    &mut state.proof_space,
                    sp_core::edit::ProofSpace::DisplayNative,
                    crate::i18n::text("Display native"),
                );
                ui.selectable_value(
                    &mut state.proof_space,
                    sp_core::edit::ProofSpace::Srgb,
                    crate::i18n::text("sRGB"),
                );
                ui.selectable_value(
                    &mut state.proof_space,
                    sp_core::edit::ProofSpace::DisplayP3,
                    crate::i18n::text("Display P3"),
                );
                ui.selectable_value(
                    &mut state.proof_space,
                    sp_core::edit::ProofSpace::Rec2020,
                    crate::i18n::text("Rec.2020"),
                );
                if !state.custom_proof_names.is_empty() {
                    ui.menu_button(crate::i18n::text("Custom ICC"), |ui| {
                        for name in state.custom_proof_names.clone() {
                            let selected = state.selected_custom_proof.as_deref() == Some(name.as_str());
                            if ui.selectable_label(selected, &name).clicked() {
                                state.selected_custom_proof = Some(name);
                                state.proof_space = sp_core::edit::ProofSpace::Custom;
                                state.custom_proof_generation = state.custom_proof_generation.wrapping_add(1);
                                ui.close();
                            }
                        }
                    });
                }
                if ui.button(crate::i18n::text("Import display or printer ICC…")).clicked() {
                    state.import_icc_requested = true;
                    ui.close();
                }
                if state.proof_space == sp_core::edit::ProofSpace::Custom {
                    use sp_pipeline::proof::ProofIntent;
                    egui::ComboBox::from_id_salt("proof-intent")
                        .selected_text(format!("{:?}", state.proof_options.intent))
                        .show_ui(ui, |ui| {
                            for (value, label) in [(ProofIntent::Relative, "Relative colorimetric"), (ProofIntent::Perceptual, "Perceptual"), (ProofIntent::Saturation, "Saturation"), (ProofIntent::Absolute, "Absolute colorimetric")] {
                                ui.selectable_value(&mut state.proof_options.intent, value, label);
                            }
                        });
                    ui.checkbox(&mut state.proof_options.black_point_compensation, crate::i18n::text("Black-point compensation"));
                    ui.checkbox(&mut state.proof_options.simulate_paper, crate::i18n::text("Simulate paper and ink"));
                    if state.proof_busy { ui.label(crate::i18n::text("Updating proof…")); }
                }
                ui.separator();
                ui.checkbox(&mut state.gamut_warning, crate::i18n::text("Gamut warning"));
                ui.separator();
                ui.label(theme::section_text("Accessibility"));
                ui.add(
                    egui::Slider::new(&mut state.ui_scale_draft, 0.75..=2.0)
                        .step_by(0.01)
                        .text(crate::i18n::text("UI scale")),
                );
                ui.horizontal(|ui| {
                    ui.label({ let __l10n_args = (&(state.ui_scale * 100.0),); crate::i18n::format("Applied: {0}%", &[format!("{:.0}", __l10n_args.0)]) });
                    if ui
                        .add_enabled(
                            (state.ui_scale_draft - state.ui_scale).abs() >= 0.005,
                            egui::Button::new(crate::i18n::text("Apply")),
                        )
                        .on_hover_text(crate::i18n::text("Apply the selected scale after fine-tuning"))
                        .clicked()
                    {
                        state.ui_scale = state.ui_scale_draft.clamp(0.75, 2.0);
                    }
                });
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut state.language, crate::i18n::Language::English, crate::i18n::text("English"));
                    ui.selectable_value(&mut state.language, crate::i18n::Language::Dutch, crate::i18n::text("Nederlands"));
                });
                let mut shadows = state.clipping_mode & 1 != 0;
                if ui.checkbox(&mut shadows, crate::i18n::text("Shadow clipping  ·  J")).changed() {
                    state.clipping_mode ^= 1;
                }
                let mut highlights = state.clipping_mode & 2 != 0;
                if ui
                    .checkbox(&mut highlights, crate::i18n::text("Highlight clipping  ·  O"))
                    .changed()
                {
                    state.clipping_mode ^= 2;
                }
            });
            view_menu.response.on_hover_text({ let __l10n_args = (&(if state.gamut_warning { "on" } else { "off" }),); crate::i18n::format("Soft proof: {0}\nGamut warning: {1}\nClipping: {2}", &[proof_summary.to_string(), crate::i18n::text(__l10n_args.0).to_string(), crate::i18n::text(clipping_summary).to_string()]) });
            ui.add_space(theme::SPACE * 0.5);
            activity_indicator(ui, state);
            ui.add_space(theme::SPACE * 0.5);
            let hdr_active = state.edit.hdr.enabled
                && state.edit.hdr.target == sp_core::edit::HdrTarget::Hdr;
            let (hdr_badge, hdr_detail) = if hdr_active {
                if state.hdr_surface && state.edr_headroom > 1.05 {
                    (
                        format!("HDR ×{:.1}", state.edr_headroom),
                        { let __l10n_args = (&(state.edr_headroom),); crate::i18n::format("HDR display active · panel headroom ×{0}", &[format!("{:.1}", __l10n_args.0)]) },
                    )
                } else if state.hdr_surface {
                    (
                        "HDR · 0 headroom".to_owned(),
                        "HDR is active but the panel reports no current headroom; raising display brightness may help".to_owned(),
                    )
                } else {
                    (
                        format!("HDR · {:.1} EV", state.edit.hdr.headroom_stops),
                        { let __l10n_args = (&(state.edit.hdr.headroom_stops),); crate::i18n::format("SDR preview · HDR export headroom {0} stops", &[format!("{:.1}", __l10n_args.0)]) },
                    )
                }
            } else if state.hdr_surface {
                if state.edr_potential > 1.05 {
                    (
                        format!("EDR ×{:.1}", state.edr_potential),
                        "EDR surface ready · HDR editing is off".to_owned(),
                    )
                } else {
                    (
                        "EDR ready".to_owned(),
                        "16-bit EDR surface ready · HDR editing is off".to_owned(),
                    )
                }
            } else {
                (
                    "SDR".to_owned(),
                    "SDR display path · enable HDR in the HDR tool for HDR export".to_owned(),
                )
            };
            let badge = status_badge(ui, &hdr_badge, hdr_active);
            badge.on_hover_text(hdr_detail);
        });
    });
}

/// Fixed-height status pill (SDR / HDR / EDR), the same height as the
/// top-bar buttons so the row never changes height.
fn status_badge(ui: &mut egui::Ui, text: &str, active: bool) -> egui::Response {
    let color = if active {
        theme::ACCENT
    } else {
        theme::TEXT_SECONDARY
    };
    let galley = ui.painter().layout_no_wrap(
        crate::i18n::text(text).to_owned(),
        egui::FontId::proportional(10.5),
        color,
    );
    let size = egui::vec2(galley.size().x + 18.0, 24.0);
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::hover());
    ui.painter().rect(
        rect,
        12.0,
        if active {
            theme::ACCENT.linear_multiply(0.16)
        } else {
            theme::BG_RAISED
        },
        egui::Stroke::new(
            1.0,
            if active {
                theme::ACCENT
            } else {
                theme::HAIRLINE
            },
        ),
        egui::StrokeKind::Inside,
    );
    ui.painter()
        .galley(rect.center() - galley.size() / 2.0, galley, color);
    resp
}

/// One unit of background work shown by the activity indicator.
struct Activity {
    title: String,
    detail: String,
    progress: Option<f32>,
    cancel: Option<fn(&mut AppState)>,
}

fn activities(state: &AppState) -> Vec<Activity> {
    let mut out = Vec::new();
    if state.auto_finish_busy {
        out.push(Activity {
            title: "Auto Finish".into(),
            detail: state.auto_finish_status.clone(),
            progress: Some(state.auto_finish_progress),
            cancel: Some(|s| s.auto_finish_cancel_requested = true),
        });
    }
    if state.export_busy {
        out.push(Activity {
            title: if state.export_batch {
                "Export selected".into()
            } else {
                "Export".into()
            },
            detail: format!("{:.0}%", state.export_progress * 100.0),
            progress: Some(state.export_progress),
            cancel: Some(|s| s.export_cancel_requested = true),
        });
    }
    if state.import_busy {
        out.push(Activity {
            title: "Import from card".into(),
            detail: format!("{:.0}%", state.import_progress * 100.0),
            progress: Some(state.import_progress),
            cancel: Some(|s| s.import_cancel_requested = true),
        });
    }
    if state.ai_model_install_busy {
        out.push(Activity {
            title: "Download denoise models".into(),
            detail: format!("{:.0}%", state.ai_model_install_progress * 100.0),
            progress: Some(state.ai_model_install_progress),
            cancel: None,
        });
    }
    if state.enhancement_busy {
        out.push(Activity {
            title: "AI enhancement".into(),
            detail: state.enhancement_status.clone(),
            progress: None,
            cancel: Some(|s| s.enhancement_cancel_requested = true),
        });
    }
    if state.ai_denoise_busy {
        out.push(Activity {
            title: "AI denoise".into(),
            detail: state.ai_denoise_status.clone().unwrap_or_default(),
            progress: None,
            cancel: None,
        });
    }
    if state.catalog_scanning {
        out.push(Activity {
            title: "Scanning folders".into(),
            detail: {
                let __l10n_args = (&(state.catalog_scan_count),);
                crate::i18n::format("{0} files", &[__l10n_args.0.to_string()])
            },
            progress: None,
            cancel: None,
        });
    }
    if state.reference_fit_busy {
        out.push(Activity {
            title: "Reference match".into(),
            detail: "Fitting profile".into(),
            progress: None,
            cancel: None,
        });
    }
    out
}

/// Progress ring for background work. Hidden when idle; shows a check
/// for two seconds after the last job finishes; click for details.
fn activity_indicator(ui: &mut egui::Ui, state: &mut AppState) {
    let jobs = activities(state);
    let done_id = egui::Id::new("activity-done-at");
    let busy_id = egui::Id::new("activity-was-busy");
    let now = ui.input(|i| i.time);
    // `busy_id` holds when the current busy period started.
    let busy_since = ui.data(|d| d.get_temp::<f64>(busy_id));
    match (busy_since, jobs.is_empty()) {
        (Some(start), true) => {
            // Only confirm work the photographer could have noticed.
            if now - start > 1.5 {
                ui.data_mut(|d| d.insert_temp(done_id, now));
            }
            ui.data_mut(|d| {
                d.remove::<f64>(busy_id);
            });
        }
        (None, false) => {
            ui.data_mut(|d| {
                d.insert_temp(busy_id, now);
            });
        }
        _ => {}
    }
    let done_at = ui.data(|d| d.get_temp::<f64>(done_id));
    let flashing = jobs.is_empty() && done_at.is_some_and(|t| now - t < 2.0);
    if jobs.is_empty() && !flashing {
        return;
    }
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(28.0, 28.0), egui::Sense::click());
    let painter = ui.painter();
    if resp.hovered() {
        painter.circle_filled(rect.center(), 14.0, theme::BG_HOVER);
    }
    let center = rect.center();
    let radius = 9.0;
    painter.circle_stroke(center, radius, egui::Stroke::new(2.5, theme::BG_RAISED));
    let arc = |from: f32, to: f32, color: egui::Color32| {
        let steps = 32;
        let points: Vec<egui::Pos2> = (0..=steps)
            .map(|i| {
                let a = from + (to - from) * i as f32 / steps as f32 - std::f32::consts::FRAC_PI_2;
                center + egui::vec2(a.cos(), a.sin()) * radius
            })
            .collect();
        painter.add(egui::Shape::line(points, egui::Stroke::new(2.5, color)));
    };
    if flashing {
        arc(0.0, std::f32::consts::TAU, theme::SUCCESS);
        painter.line_segment(
            [
                center + egui::vec2(-3.5, 0.0),
                center + egui::vec2(-1.0, 3.0),
            ],
            egui::Stroke::new(1.8, theme::SUCCESS),
        );
        painter.line_segment(
            [
                center + egui::vec2(-1.0, 3.0),
                center + egui::vec2(4.0, -3.0),
            ],
            egui::Stroke::new(1.8, theme::SUCCESS),
        );
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(200));
    } else {
        let known: Vec<f32> = jobs.iter().filter_map(|j| j.progress).collect();
        if known.len() == jobs.len() {
            let fraction = known.iter().sum::<f32>() / known.len() as f32;
            arc(
                0.0,
                std::f32::consts::TAU * fraction.clamp(0.02, 1.0),
                theme::ACCENT,
            );
        } else {
            let start = (now as f32 * 4.0) % std::f32::consts::TAU;
            arc(start, start + 1.8, theme::ACCENT);
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(33));
        }
        if jobs.len() > 1 {
            painter.text(
                center,
                egui::Align2::CENTER_CENTER,
                jobs.len().to_string(),
                egui::FontId::proportional(9.5),
                theme::TEXT_PRIMARY,
            );
        }
    }
    let resp = resp.on_hover_text(crate::i18n::text(
        &(if jobs.is_empty() {
            crate::i18n::text("All background work finished").to_owned()
        } else {
            jobs.iter()
                .map(|j| crate::i18n::text(&j.title).to_owned())
                .collect::<Vec<_>>()
                .join(" · ")
        }),
    ));
    egui::Popup::menu(&resp).show(|ui| {
        ui.set_width(280.0);
        ui.label(theme::section_text("Background activity"));
        if jobs.is_empty() {
            ui.label(
                egui::RichText::new(crate::i18n::text("Nothing running"))
                    .color(theme::TEXT_SECONDARY),
            );
        }
        for job in &jobs {
            ui.separator();
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(crate::i18n::text(&job.title)).strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if let Some(cancel) = job.cancel {
                        if ui.small_button(crate::i18n::text("Cancel")).clicked() {
                            cancel(state);
                        }
                    }
                });
            });
            match job.progress {
                Some(p) => {
                    ui.add(egui::ProgressBar::new(p).desired_height(4.0));
                }
                None => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                    });
                }
            }
            if !job.detail.is_empty() {
                ui.label(
                    egui::RichText::new(crate::i18n::text(&job.detail))
                        .size(11.0)
                        .color(theme::TEXT_SECONDARY),
                );
            }
        }
        if !state.auto_finish_errors.is_empty() {
            ui.separator();
            ui.collapsing(
                {
                    let __l10n_args = (&(state.auto_finish_errors.len()),);
                    crate::i18n::format("{0} items need review", &[__l10n_args.0.to_string()])
                },
                |ui| {
                    egui::ScrollArea::vertical()
                        .max_height(160.0)
                        .show(ui, |ui| {
                            for error in &state.auto_finish_errors {
                                ui.label(egui::RichText::new(error).size(11.0));
                            }
                        });
                },
            );
        }
    });
}

/// Two-state Library/Develop segmented control.
pub(super) fn segmented(ui: &mut egui::Ui, mode: &mut Mode) {
    let seg_h = 28.0;
    let seg_w = 92.0;
    egui::Frame::new()
        .fill(theme::BG_VIEWPORT)
        .corner_radius(seg_h / 2.0)
        .inner_margin(egui::Margin::same(3))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                for (m, label) in [(Mode::Library, "Library"), (Mode::Develop, "Develop")] {
                    let selected = *mode == m;
                    let (rect, resp) = ui
                        .allocate_exact_size(egui::vec2(seg_w, seg_h - 6.0), egui::Sense::click());
                    if selected {
                        ui.painter()
                            .rect_filled(rect, (seg_h - 6.0) / 2.0, theme::BG_HOVER);
                    }
                    ui.painter().text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        label,
                        egui::FontId::proportional(12.5),
                        if selected {
                            theme::TEXT_PRIMARY
                        } else {
                            theme::TEXT_SECONDARY
                        },
                    );
                    if resp.clicked() {
                        *mode = m;
                    }
                    if resp.hovered() && !selected {
                        ui.painter().rect_filled(
                            rect,
                            (seg_h - 6.0) / 2.0,
                            egui::Color32::from_white_alpha(4),
                        );
                    }
                }
            });
        });
}

/// Rounded pill button; `primary` renders accent-filled.
pub(super) fn pill_button(ui: &mut egui::Ui, label: &str, primary: bool) -> egui::Response {
    let text = egui::RichText::new(crate::i18n::text(label)).size(12.5);
    let button = if primary {
        egui::Button::new(text.color(theme::ON_ACCENT).strong())
            .fill(theme::ACCENT)
            .corner_radius(14.0)
    } else {
        egui::Button::new(text)
            .fill(theme::BG_RAISED)
            .corner_radius(14.0)
    };
    ui.add(button)
}

pub fn left_rail(
    ui: &mut egui::Ui,
    state: &mut AppState,
    preset_thumbs: &[Option<egui::TextureHandle>],
) {
    ui.add_space(theme::SPACE * 1.5);
    if state.mode == Mode::Develop {
        // A full-height scroll area: it must not sit inside a horizontal
        // row, whose height is capped at one control.
        egui::Frame::new()
            .inner_margin(egui::Margin {
                left: 6,
                right: 0,
                top: 0,
                bottom: 0,
            })
            .show(ui, |ui| {
                super::tool_rail::develop_rail(ui, state, preset_thumbs)
            });
        return;
    }
    ui.horizontal(|ui| {
        ui.add_space(theme::SPACE * 0.75);
        ui.vertical(|ui| match state.mode {
            Mode::Library => {
                ui.label(theme::section_text("Folders"));
                ui.add_space(theme::SPACE * 0.5);
                let button_w = ui.available_width();
                if ui
                    .add_sized(
                        [button_w, 26.0],
                        egui::Button::new(crate::i18n::text("+ Add folder…")),
                    )
                    .clicked()
                {
                    state.library_add_folder_requested = true;
                }
                if ui
                    .add_enabled_ui(
                        !state.catalog_scanning && !state.catalog_folders.is_empty(),
                        |ui| {
                            ui.add_sized(
                                [button_w, 26.0],
                                egui::Button::new(crate::i18n::text("↻ Refresh folders")),
                            )
                        },
                    )
                    .inner
                    .on_hover_text(crate::i18n::text(
                        "Rescan folders, including network shares",
                    ))
                    .clicked()
                {
                    state.library_refresh_requested = true;
                }
                if ui
                    .add_sized(
                        [button_w, 26.0],
                        egui::Button::new(crate::i18n::text("Import from card…")),
                    )
                    .clicked()
                {
                    state.import_dialog_open = true;
                    state.import_sources_scan_requested = true;
                }
                if state.catalog_scanning {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label({
                            let __l10n_args = (&(state.catalog_scan_count),);
                            crate::i18n::format("Scanning {0} files", &[__l10n_args.0.to_string()])
                        });
                    });
                }
                let mut remove_folder = None;
                for folder in &state.catalog_folders {
                    // Folder icon + name on the left, warning and remove on the
                    // right, so every row lines up under the section header.
                    ui.horizontal(|ui| {
                        let full = ui.available_width();
                        let (icon, _) =
                            ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::hover());
                        theme::paint_folder_icon(
                            ui.painter(),
                            icon.center(),
                            theme::TEXT_SECONDARY,
                        );
                        let error = state.catalog_folder_errors.get(&folder.path);
                        let trailing = 22.0 + if error.is_some() { 18.0 } else { 0.0 };
                        let name = folder
                            .path
                            .file_name()
                            .and_then(|name| name.to_str())
                            .unwrap_or_else(|| folder.path.to_str().unwrap_or("Folder"));
                        let label = ui
                            .allocate_ui_with_layout(
                                egui::vec2((full - 14.0 - trailing - 8.0).max(40.0), 20.0),
                                egui::Layout::left_to_right(egui::Align::Center),
                                |ui| {
                                    ui.add(
                                        egui::Label::new(egui::RichText::new(name).size(12.0))
                                            .truncate()
                                            .sense(egui::Sense::click()),
                                    )
                                },
                            )
                            .inner;
                        if let Some(error) = error {
                            let (mark, response) = ui
                                .allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::hover());
                            theme::paint_warning_icon(ui.painter(), mark.center(), 11.0);
                            response.on_hover_text(crate::i18n::format(
                                "Folder unavailable; existing photos were kept.\n{0}",
                                std::slice::from_ref(error),
                            ));
                        }
                        if ui
                            .add_enabled(
                                !state.catalog_scanning,
                                egui::Button::new(crate::i18n::text("×"))
                                    .small()
                                    .frame(false),
                            )
                            .on_hover_text(crate::i18n::text(
                                "Remove from library (files stay on disk)",
                            ))
                            .clicked()
                        {
                            remove_folder = Some(folder.id);
                        }
                        label.context_menu(|ui| {
                            if ui
                                .add_enabled(
                                    !state.catalog_tool_busy && !state.catalog_scanning,
                                    egui::Button::new(crate::i18n::text("Relink moved folder…")),
                                )
                                .clicked()
                            {
                                state.catalog_tool_requested = Some(
                                    crate::app::catalog_tools::Action::RelinkFolder(folder.id),
                                );
                                ui.close();
                            }
                        });
                        label.on_hover_text(folder.path.display().to_string());
                    });
                }
                if let Some(folder_id) = remove_folder {
                    state.library_remove_folder_requested = Some(folder_id);
                }
                if state.catalog_folders.is_empty() && !state.catalog_scanning {
                    ui.label(
                        egui::RichText::new(crate::i18n::text("Add a folder to begin"))
                            .italics()
                            .size(12.0)
                            .color(theme::TEXT_SECONDARY),
                    );
                }
                ui.add_space(theme::SPACE * 2.0);
                ui.label(theme::section_text("Collections"));
                if ui
                    .selectable_label(
                        state.library_collection.is_none(),
                        crate::i18n::text("All photographs"),
                    )
                    .clicked()
                {
                    state.library_collection = None;
                }
                egui::ComboBox::from_id_salt("new-collection-parent")
                    .selected_text(
                        state
                            .new_collection_parent
                            .and_then(|id| {
                                state
                                    .catalog_collections
                                    .iter()
                                    .find(|collection| collection.id == id)
                            })
                            .map_or("Top level", |collection| collection.name.as_str()),
                    )
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut state.new_collection_parent,
                            None,
                            crate::i18n::text("Top level"),
                        );
                        for collection in &state.catalog_collections {
                            ui.selectable_value(
                                &mut state.new_collection_parent,
                                Some(collection.id),
                                &collection.name,
                            );
                        }
                    });
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut state.new_collection_name)
                            .hint_text(crate::i18n::text("New collection"))
                            .desired_width(120.0),
                    );
                    if ui
                        .add_enabled(
                            !state.new_collection_name.trim().is_empty(),
                            egui::Button::new(crate::i18n::text("+")),
                        )
                        .clicked()
                    {
                        state.collection_create_requested = true;
                    }
                });
                collection_tree(ui, state, None, 0);
                ui.collapsing(crate::i18n::text("Keywords"), |ui| {
                    ui.label(
                        egui::RichText::new(crate::i18n::text(
                            "Use Parent|Child in metadata to build a hierarchy.",
                        ))
                        .small()
                        .color(theme::TEXT_SECONDARY),
                    );
                    if ui
                        .selectable_label(
                            state.library_filter.keyword.is_none(),
                            crate::i18n::text("All keywords"),
                        )
                        .clicked()
                    {
                        state.library_filter.keyword = None;
                    }
                    let mut counts = std::collections::BTreeMap::<String, usize>::new();
                    for photo in &state.catalog_photos {
                        let mut present = std::collections::HashSet::new();
                        for keyword in &photo.keywords {
                            let mut prefix = String::new();
                            for part in keyword.split('|').filter(|part| !part.trim().is_empty()) {
                                if !prefix.is_empty() {
                                    prefix.push('|');
                                }
                                prefix.push_str(part.trim());
                                present.insert(prefix.clone());
                            }
                        }
                        for keyword in present {
                            *counts.entry(keyword).or_default() += 1;
                        }
                    }
                    egui::ScrollArea::vertical()
                        .id_salt("keyword-tree")
                        .max_height(250.0)
                        .show(ui, |ui| {
                            for (keyword, count) in counts {
                                ui.horizontal(|ui| {
                                    ui.add_space(keyword.matches('|').count() as f32 * 10.0);
                                    let label = format!(
                                        "{} ({count})",
                                        keyword.rsplit('|').next().unwrap_or(&keyword)
                                    );
                                    if ui
                                        .selectable_label(
                                            state.library_filter.keyword.as_ref() == Some(&keyword),
                                            label,
                                        )
                                        .on_hover_text(&keyword)
                                        .clicked()
                                    {
                                        state.library_filter.keyword = Some(keyword);
                                    }
                                });
                            }
                        });
                });
            }
            Mode::Develop => super::tool_rail::develop_rail(ui, state, preset_thumbs),
        });
    });
}

fn collection_tree(ui: &mut egui::Ui, state: &mut AppState, parent: Option<i64>, depth: usize) {
    if depth > 64 {
        return;
    }
    let collections: Vec<_> = state
        .catalog_collections
        .iter()
        .filter(|collection| collection.parent_id == parent)
        .cloned()
        .collect();
    for collection in collections {
        ui.horizontal(|ui| {
            ui.add_space(depth as f32 * 12.0);
            let response = ui
                .selectable_label(
                    state.library_collection == Some(collection.id),
                    &collection.name,
                )
                .on_hover_text(crate::i18n::text(
                    "Show this collection and its nested collections. Drop a photo here to add it.",
                ));
            if response.clicked() {
                state.library_collection = Some(collection.id);
            }
            if response.hovered() && ui.input(|input| input.pointer.any_released()) {
                if let Some(path) = state.dragged_photo.take() {
                    state.collection_add_requested = Some((collection.id, path));
                }
            }
            response.context_menu(|ui| {
                ui.add_enabled_ui(!state.catalog_tool_busy && !state.catalog_scanning, |ui| {
                    ui.menu_button(crate::i18n::text("Move collection into"), |ui| {
                        if ui.button(crate::i18n::text("Top level")).clicked() {
                            state.catalog_tool_requested =
                                Some(crate::app::catalog_tools::Action::MoveCollection(
                                    collection.id,
                                    None,
                                ));
                            ui.close();
                        }
                        for target in &state.catalog_collections {
                            if target.id != collection.id && ui.button(&target.name).clicked() {
                                state.catalog_tool_requested =
                                    Some(crate::app::catalog_tools::Action::MoveCollection(
                                        collection.id,
                                        Some(target.id),
                                    ));
                                ui.close();
                            }
                        }
                    });
                    if ui
                        .button(crate::i18n::text("Remove collection (keep photos)"))
                        .clicked()
                    {
                        state.catalog_tool_requested = Some(
                            crate::app::catalog_tools::Action::DeleteCollection(collection.id),
                        );
                        if state.library_collection == Some(collection.id) {
                            state.library_collection = None;
                        }
                        ui.close();
                    }
                });
            });
        });
        collection_tree(ui, state, Some(collection.id), depth + 1);
    }
}

/// Built-in presets as a two-column grid of thumbnails of the open photo.
/// Hovering previews on the main image without an undo step; clicking
/// applies; Amount blends between the pre-preset edit and the preset.
pub(super) fn presets_rail(
    ui: &mut egui::Ui,
    state: &mut AppState,
    thumbs: &[Option<egui::TextureHandle>],
) {
    state.preset_preview = None;
    egui::ScrollArea::vertical()
        .id_salt("presets-rail")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.set_width(ui.available_width() - 6.0);
            ui.horizontal(|ui| {
                ui.label(theme::section_text(crate::i18n::text("Presets")));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new(crate::i18n::text("hover to preview"))
                            .size(10.0)
                            .color(theme::TEXT_FAINT),
                    );
                });
            });
            ui.add_space(theme::SPACE * 0.5);

            // Amount / modified state for the active preset.
            if let (Some(active), Some(base)) = (state.active_preset, state.preset_base.clone()) {
                let mut full = base.clone();
                active.apply(&mut full);
                let expected = crate::edit_groups::blend(&base, &full, state.preset_amount / 100.0);
                if expected == state.edit {
                    let mut amount = state.preset_amount;
                    if develop::param_slider_spec(
                        ui,
                        "Amount",
                        &mut amount,
                        0.0..=100.0,
                        100.0,
                        develop::SliderSpec::unit(develop::Unit::Percent),
                    ) {
                        state.preset_amount = amount;
                        state.edit = crate::edit_groups::blend(&base, &full, amount / 100.0);
                    }
                } else {
                    egui::Frame::new()
                        .fill(theme::ACCENT.gamma_multiply(0.12))
                        .stroke(egui::Stroke::new(1.0, theme::ACCENT.gamma_multiply(0.5)))
                        .corner_radius(theme::RADIUS_CONTROL)
                        .inner_margin(egui::Margin::symmetric(8, 4))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(
                                    egui::RichText::new({
                                        let __l10n_args = (&(active.name()),);
                                        crate::i18n::format(
                                            "{0} · modified",
                                            &[__l10n_args.0.to_string()],
                                        )
                                    })
                                    .size(11.0)
                                    .color(theme::ACCENT_HOVER),
                                );
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if ui
                                            .small_button(crate::i18n::text("Re-apply"))
                                            .on_hover_text(crate::i18n::text(
                                                "Apply the preset again at the current Amount",
                                            ))
                                            .clicked()
                                        {
                                            state.apply_edit_step_labeled(
                                                expected.clone(),
                                                Some({
                                                    let __l10n_args = (&(active.name()),);
                                                    crate::i18n::format(
                                                        "Preset: {0}",
                                                        &[__l10n_args.0.to_string()],
                                                    )
                                                }),
                                            );
                                        }
                                    },
                                );
                            });
                        });
                }
                ui.add_space(theme::SPACE * 0.5);
            }

            let spacing = 6.0;
            let tile_w = ((ui.available_width() - spacing) / 2.0).floor();
            let mut previous_category = "";
            let mut clicked = None;
            let presets = sp_core::preset::DevelopPreset::ALL;
            let mut index = 0;
            while index < presets.len() {
                let category = presets[index].category();
                if category != previous_category {
                    previous_category = category;
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new(category.to_uppercase())
                            .size(10.0)
                            .color(theme::TEXT_FAINT),
                    );
                }
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = spacing;
                    for _ in 0..2 {
                        let Some(preset) = presets
                            .get(index)
                            .copied()
                            .filter(|p| p.category() == category)
                        else {
                            break;
                        };
                        let thumb = thumbs.get(index).and_then(Option::as_ref);
                        let resp = preset_tile(
                            ui,
                            preset,
                            thumb,
                            tile_w,
                            state.active_preset == Some(preset),
                        );
                        if resp.hovered() {
                            state.preset_preview = Some(preset);
                        }
                        if resp.clicked() {
                            clicked = Some(preset);
                        }
                        resp.on_hover_text(crate::i18n::text(preset.description()));
                        index += 1;
                    }
                });
            }
            if let Some(preset) = clicked {
                state.preset_preview = None;
                let base = state.builtin_preset_base();
                let mut edit = base.clone();
                preset.apply(&mut edit);
                state.preset_base = Some(base);
                state.preset_amount = 100.0;
                state.apply_edit_step_labeled(
                    edit,
                    Some({
                        let __l10n_args = (&(preset.name()),);
                        crate::i18n::format("Preset: {0}", &[__l10n_args.0.to_string()])
                    }),
                );
                state.active_preset = Some(preset);
            }

            if !state.user_presets.is_empty() {
                ui.add_space(theme::SPACE);
                ui.label(
                    egui::RichText::new(crate::i18n::text("YOUR PRESETS"))
                        .size(10.0)
                        .color(theme::TEXT_FAINT),
                );
                let mut apply = None;
                for (index, preset) in state.user_presets.iter().enumerate() {
                    if ui
                        .add_sized(
                            [ui.available_width(), 26.0],
                            egui::Button::new(egui::RichText::new(&preset.name).size(11.5)),
                        )
                        .clicked()
                    {
                        apply = Some(index);
                    }
                }
                if let Some(index) = apply {
                    let mut edit = state.edit.clone();
                    state.user_presets[index].apply(&mut edit);
                    let name = state.user_presets[index].name.clone();
                    state.apply_edit_step_labeled(
                        edit,
                        Some(crate::i18n::format(
                            "Preset: {0}",
                            std::slice::from_ref(&name),
                        )),
                    );
                    state.active_preset = None;
                    state.preset_base = None;
                }
            }
            ui.add_space(theme::SPACE);
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut state.user_preset_name)
                        .hint_text(crate::i18n::text("Preset name"))
                        .desired_width(ui.available_width() - 64.0),
                );
                if ui
                    .add_enabled(
                        !state.user_preset_name.trim().is_empty(),
                        egui::Button::new(crate::i18n::text("Save")),
                    )
                    .on_hover_text(crate::i18n::text("Save the current edit as a preset"))
                    .clicked()
                {
                    state.user_preset_save_requested = true;
                }
            });
            ui.horizontal(|ui| {
                if ui.small_button(crate::i18n::text("Import…")).clicked() {
                    state.user_preset_import_requested = true;
                }
                if ui
                    .add_enabled(
                        !state.user_presets.is_empty(),
                        egui::Button::new(crate::i18n::text("Export all…")).small(),
                    )
                    .clicked()
                {
                    state.user_preset_export_requested = true;
                }
            });
            ui.add_space(theme::SPACE * 0.5);
            ui.label(
                egui::RichText::new(crate::i18n::text(
                    "Presets keep brightness, white balance, crop, denoise and lens settings.",
                ))
                .size(10.0)
                .color(theme::TEXT_FAINT),
            );
        });
}

fn preset_tile(
    ui: &mut egui::Ui,
    preset: sp_core::preset::DevelopPreset,
    thumb: Option<&egui::TextureHandle>,
    width: f32,
    selected: bool,
) -> egui::Response {
    let image_h = (width * 0.68).round();
    let (rect, resp) =
        ui.allocate_exact_size(egui::vec2(width, image_h + 20.0), egui::Sense::click());
    let painter = ui.painter();
    let hovered = resp.hovered();
    painter.rect_filled(
        rect,
        6.0,
        if hovered {
            theme::BG_HOVER
        } else {
            theme::BG_CARD
        },
    );
    let image_rect = egui::Rect::from_min_size(
        rect.min + egui::vec2(3.0, 3.0),
        egui::vec2(width - 6.0, image_h - 3.0),
    );
    match thumb {
        Some(texture) => {
            // Cover-fit the thumbnail into the tile.
            let size = texture.size_vec2();
            let (ta, ra) = (size.x / size.y.max(1.0), image_rect.aspect_ratio());
            let uv = if ta > ra {
                let w = ra / ta;
                egui::Rect::from_min_max(
                    egui::pos2((1.0 - w) / 2.0, 0.0),
                    egui::pos2((1.0 + w) / 2.0, 1.0),
                )
            } else {
                let h = ta / ra;
                egui::Rect::from_min_max(
                    egui::pos2(0.0, (1.0 - h) / 2.0),
                    egui::pos2(1.0, (1.0 + h) / 2.0),
                )
            };
            painter.image(texture.id(), image_rect, uv, egui::Color32::WHITE);
        }
        None => {
            painter.rect_filled(image_rect, 4.0, theme::BG_RAISED);
        }
    }
    let color = if selected || hovered {
        theme::TEXT_PRIMARY
    } else {
        theme::TEXT_SECONDARY
    };
    painter.with_clip_rect(rect.shrink(4.0)).text(
        egui::pos2(rect.left() + 6.0, rect.bottom() - 9.0),
        egui::Align2::LEFT_CENTER,
        crate::i18n::text(preset.name()),
        egui::FontId::proportional(11.0),
        color,
    );
    if selected {
        painter.rect_stroke(
            rect,
            6.0,
            egui::Stroke::new(1.5, theme::ACCENT),
            egui::StrokeKind::Inside,
        );
    }
    resp
}
