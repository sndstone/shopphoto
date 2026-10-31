//! Library grid, filmstrip and the card-import dialog.

use super::*;

pub fn import_dialog(
    ctx: &egui::Context,
    state: &mut AppState,
    thumbs: &std::collections::HashMap<std::path::PathBuf, egui::TextureHandle>,
) {
    if !state.import_dialog_open {
        return;
    }
    let mut open = true;
    let mut close_requested = false;
    egui::Window::new(crate::i18n::text("Import from card"))
        .id(egui::Id::new("card-import-dialog"))
        .collapsible(false)
        .resizable(true)
        .default_width(610.0)
        .open(&mut open)
        .show(ctx, |ui| {
            ui.add_enabled_ui(!state.import_busy, |ui| {
            ui.label(crate::i18n::text("Copy originals into the library with optional backup, rename and preset application."));
            ui.label(egui::RichText::new(crate::i18n::text("Every copied photo and backup is checked against its content checksum. Existing files are preserved.")).small().color(theme::TEXT_SECONDARY));
            ui.separator();
            if state.import_sources_scanning {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(crate::i18n::text("Looking for cards…"));
                });
            }
            if !state.import_detected_sources.is_empty() {
                egui::ComboBox::from_id_salt("detected-import-source")
                    .selected_text(
                        state
                            .import_source
                            .as_ref()
                            .map_or(crate::i18n::text("Detected cards").into(), |path| path.display().to_string()),
                    )
                    .show_ui(ui, |ui| {
                        for source in &state.import_detected_sources {
                            ui.selectable_value(
                                &mut state.import_source,
                                Some(source.clone()),
                                source.display().to_string(),
                            );
                        }
                    });
            }
            for (label, path, request) in [
                ("Source", &state.import_source, &mut state.import_source_requested),
                (
                    "Destination",
                    &state.import_destination,
                    &mut state.import_destination_requested,
                ),
                ("Backup", &state.import_backup, &mut state.import_backup_requested),
            ] {
                ui.horizontal(|ui| {
                    ui.label(crate::i18n::text(label));
                    ui.label(path.as_ref().map_or_else(
                        || if label == "Backup" { crate::i18n::text("None").into() } else { crate::i18n::text("Not selected").into() },
                        |path| path.display().to_string(),
                    ));
                    if ui.button(crate::i18n::text("Choose…")).clicked() {
                        *request = true;
                    }
                });
            }
            if state.import_backup.is_some() && ui.small_button(crate::i18n::text("Clear backup")).clicked() {
                state.import_backup = None;
            }
            ui.horizontal(|ui| {
                if ui.add_enabled(state.import_source.is_some() && !state.import_review_busy, egui::Button::new(crate::i18n::text("Review and select photos…"))).clicked() {
                    state.import_review_requested = true;
                }
                if state.import_review_busy { ui.spinner(); ui.label(crate::i18n::text("Reading previews…")); }
            });
            if state.import_review_source == state.import_source && state.import_review_source.is_some() {
                let selected = state.import_review.iter().filter(|entry| entry.selected).count();
                ui.horizontal(|ui| {
                    ui.label({ let __l10n_args = (&(state.import_review.len()),); crate::i18n::format("{0} of {1} selected", &[selected.to_string(), __l10n_args.0.to_string()]) });
                    if ui.small_button(crate::i18n::text("All")).clicked() { for entry in &mut state.import_review { entry.selected = true; } }
                    if ui.small_button(crate::i18n::text("None")).clicked() { for entry in &mut state.import_review { entry.selected = false; } }
                });
                egui::ScrollArea::vertical().id_salt("import-photo-selection").max_height(220.0).show_rows(ui, 54.0, state.import_review.len(), |ui, range| {
                    for entry in &mut state.import_review[range] {
                        ui.horizontal(|ui| {
                            ui.checkbox(&mut entry.selected, crate::i18n::text(""));
                            if let Some(texture) = thumbs.get(&entry.path) {
                                ui.add(egui::Image::new(texture).fit_to_exact_size(egui::vec2(72.0, 48.0)));
                            }
                            ui.vertical(|ui| {
                                ui.label(entry.path.strip_prefix(state.import_source.as_ref().map_or(std::path::Path::new(""), |v| v.as_path())).unwrap_or(&entry.path).display().to_string());
                                ui.label(egui::RichText::new(entry.captured_at.as_deref().unwrap_or("Capture date unavailable")).small().color(theme::TEXT_SECONDARY));
                                if let Some(warning) = &entry.warning { ui.small(crate::i18n::text("Preview unavailable")).on_hover_text(warning); }
                            });
                        });
                    }
                });
            }
            ui.horizontal(|ui| {
                ui.label(crate::i18n::text("Rename"));
                ui.text_edit_singleline(&mut state.import_filename_pattern);
            });
            ui.label(
                egui::RichText::new(crate::i18n::text("Tokens: {name}, {seq}, {date}. Photos without a capture date use Undated."))
                    .size(10.0)
                    .color(theme::TEXT_FAINT),
            );
            egui::ComboBox::from_id_salt("import-folder-layout")
                .selected_text(match state.import_folder_layout { sp_catalog::workflow::ImportFolderLayout::Flat => "One destination folder", sp_catalog::workflow::ImportFolderLayout::CaptureDate => "Capture date: YYYY / YYYY-MM-DD" })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut state.import_folder_layout, sp_catalog::workflow::ImportFolderLayout::Flat, crate::i18n::text("One destination folder"));
                    ui.selectable_value(&mut state.import_folder_layout, sp_catalog::workflow::ImportFolderLayout::CaptureDate, crate::i18n::text("Capture date: YYYY / YYYY-MM-DD"));
                });
            ui.collapsing(crate::i18n::text("Metadata to apply on import"), |ui| {
                ui.label(egui::RichText::new(crate::i18n::text("Blank fields preserve source metadata. Keywords are added to existing keywords.")).small().color(theme::TEXT_SECONDARY));
                ui.horizontal(|ui| {
                    for (label, action) in [("Load template…", crate::app::catalog_tools::Action::LoadImportMetadata), ("Save template…", crate::app::catalog_tools::Action::SaveImportMetadata)] {
                        if ui.add_enabled(!state.catalog_tool_busy && !state.catalog_scanning, egui::Button::new(label)).clicked() { state.catalog_tool_requested = Some(action); }
                    }
                });
                for (label, value) in [
                    ("Creator", &mut state.import_metadata.creator),
                    ("Copyright", &mut state.import_metadata.copyright),
                    ("Usage rights", &mut state.import_metadata.rights),
                    ("Contact email", &mut state.import_metadata.contact),
                    ("Caption", &mut state.import_metadata.caption),
                    ("Keywords, comma separated", &mut state.import_keywords_draft),
                ] { ui.horizontal(|ui| { ui.label(crate::i18n::text(label)); ui.text_edit_singleline(value); }); }
            });
            egui::ComboBox::from_id_salt("import-preset")
                .selected_text(state.import_preset_name.as_deref().unwrap_or("No preset"))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut state.import_preset_name, None, crate::i18n::text("No preset"));
                    for preset in &state.user_presets {
                        ui.selectable_value(
                            &mut state.import_preset_name,
                            Some(preset.name.clone()),
                            &preset.name,
                        );
                    }
                });
            ui.checkbox(&mut state.import_skip_duplicates, crate::i18n::text("Skip duplicate content"));
            ui.small(crate::i18n::text("Interrupted imports resume verified originals, edits and backups before duplicate checks. Keep the same source, destination and backup folders."));
            });
            if state.import_busy {
                ui.add(egui::ProgressBar::new(state.import_progress).show_percentage());
            }
            ui.separator();
            ui.horizontal(|ui| {
                if state.import_busy {
                    if ui.button(crate::i18n::text("Cancel import")).clicked() {
                        state.import_cancel_requested = true;
                    }
                } else if ui
                    .add_enabled(
                        state.import_source.is_some()
                            && state.import_destination.is_some()
                            && !state.import_review_busy
                            && (state.import_review_source != state.import_source || state.import_review.iter().any(|entry| entry.selected))
                            && !state.import_filename_pattern.trim().is_empty(),
                        egui::Button::new(crate::i18n::text("Import / resume")),
                    )
                    .clicked()
                {
                    state.import_start_requested = true;
                }
                if ui.add_enabled(!state.import_busy, egui::Button::new(crate::i18n::text("Close"))).clicked() {
                    close_requested = true;
                }
            });
        });
    state.import_dialog_open = open && !close_requested;
}

/// Photo paths the filmstrip shows: the catalog window around the open
/// photo, or the session list when no folder is catalogued.
fn strip_paths(state: &AppState) -> (Vec<std::path::PathBuf>, usize, usize) {
    if state.catalog_photos.is_empty() {
        let current = state
            .open_image
            .as_ref()
            .and_then(|path| state.session.iter().position(|p| p == path))
            .unwrap_or(0);
        return (state.session.clone(), current, state.session.len());
    }
    let current = state
        .open_image
        .as_ref()
        .and_then(|path| {
            state
                .catalog_photos
                .iter()
                .position(|photo| &photo.path == path)
        })
        .unwrap_or(0);
    let start = current.saturating_sub(25);
    let end = (start + 51).min(state.catalog_photos.len());
    (
        state.catalog_photos[start..end]
            .iter()
            .map(|photo| photo.path.clone())
            .collect(),
        current,
        state.catalog_photos.len(),
    )
}

fn photo_for<'a>(
    state: &'a AppState,
    path: &std::path::Path,
) -> Option<&'a sp_catalog::CatalogPhoto> {
    state.catalog_photos.iter().find(|photo| photo.path == path)
}

fn paint_stars(
    painter: &egui::Painter,
    left_center: egui::Pos2,
    rating: u8,
    size: f32,
    empty: bool,
) {
    for i in 0..5u8 {
        if i >= rating && !empty {
            break;
        }
        painter.text(
            left_center + egui::vec2(f32::from(i) * (size + 1.0), 0.0),
            egui::Align2::LEFT_CENTER,
            "★",
            egui::FontId::proportional(size),
            if i < rating {
                theme::STAR
            } else {
                theme::TEXT_FAINT.gamma_multiply(0.5)
            },
        );
    }
}

fn paint_flag(painter: &egui::Painter, center: egui::Pos2, flag: sp_catalog::Flag, radius: f32) {
    let stroke = egui::Stroke::new(1.6, theme::ON_ACCENT);
    match flag {
        sp_catalog::Flag::Pick => {
            painter.circle_filled(center, radius, theme::SUCCESS);
            painter.line_segment(
                [
                    center + egui::vec2(-radius * 0.45, 0.0),
                    center + egui::vec2(-radius * 0.1, radius * 0.4),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    center + egui::vec2(-radius * 0.1, radius * 0.4),
                    center + egui::vec2(radius * 0.5, -radius * 0.35),
                ],
                stroke,
            );
        }
        sp_catalog::Flag::Reject => {
            painter.circle_filled(center, radius, theme::ERROR);
            let d = radius * 0.38;
            painter.line_segment(
                [center + egui::vec2(-d, -d), center + egui::vec2(d, d)],
                stroke,
            );
            painter.line_segment(
                [center + egui::vec2(-d, d), center + egui::vec2(d, -d)],
                stroke,
            );
        }
        sp_catalog::Flag::None => {}
    }
}

/// Cover-fit UVs for a texture drawn into `rect`.
fn contain_rect(texture_size: egui::Vec2, rect: egui::Rect) -> egui::Rect {
    let scale = (rect.width() / texture_size.x).min(rect.height() / texture_size.y);
    egui::Rect::from_center_size(rect.center(), texture_size * scale)
}

pub fn filmstrip(
    ui: &mut egui::Ui,
    state: &mut AppState,
    thumbs: &std::collections::HashMap<std::path::PathBuf, egui::TextureHandle>,
) {
    let (paths, current_index, total) = strip_paths(state);
    if paths.is_empty() {
        ui.centered_and_justified(|ui| {
            ui.label(
                egui::RichText::new(crate::i18n::text("Filmstrip — drop photos or use Open"))
                    .color(theme::TEXT_SECONDARY),
            );
        });
        return;
    }
    let cell_h = theme::FILMSTRIP_H - 40.0;
    ui.horizontal(|ui| {
        // Position counter with previous/next.
        ui.vertical(|ui| {
            ui.set_width(58.0);
            ui.add_space(12.0);
            ui.vertical_centered(|ui| {
                ui.label(
                    egui::RichText::new(format!("{} / {total}", current_index + 1))
                        .size(11.5)
                        .color(theme::TEXT_PRIMARY),
                );
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    let all: Vec<_> = if state.catalog_photos.is_empty() {
                        state.session.clone()
                    } else {
                        state
                            .catalog_photos
                            .iter()
                            .map(|p| p.path.clone())
                            .collect()
                    };
                    if ui
                        .add_enabled(
                            current_index > 0,
                            egui::Button::new(crate::i18n::text("‹")).small(),
                        )
                        .on_hover_text(crate::i18n::text("Previous photo · ←"))
                        .clicked()
                    {
                        state.filmstrip_clicked = all.get(current_index - 1).cloned();
                    }
                    if ui
                        .add_enabled(
                            current_index + 1 < total,
                            egui::Button::new(crate::i18n::text("›")).small(),
                        )
                        .on_hover_text(crate::i18n::text("Next photo · →"))
                        .clicked()
                    {
                        state.filmstrip_clicked = all.get(current_index + 1).cloned();
                    }
                });
                if state.selected_photos.len() > 1 {
                    ui.label(
                        egui::RichText::new({
                            let __l10n_args = (&(state.selected_photos.len()),);
                            crate::i18n::format("{0} sel.", &[__l10n_args.0.to_string()])
                        })
                        .size(10.0)
                        .color(theme::ACCENT_HOVER),
                    );
                }
            });
        });
        egui::ScrollArea::horizontal()
            .id_salt("filmstrip-scroll")
            .auto_shrink([false, false])
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 5.0;
                    ui.add_space(4.0);
                    let last_current_id = egui::Id::new("filmstrip-last-current");
                    let last_current =
                        ui.data(|d| d.get_temp::<std::path::PathBuf>(last_current_id));
                    for path in paths {
                        if !state.visible_thumbnail_paths.contains(&path) {
                            state.visible_thumbnail_paths.push(path.clone());
                        }
                        let is_current = state.open_image.as_ref() == Some(&path);
                        let selected = state.selected_photos.contains(&path)
                            && state.selected_photos.len() > 1;
                        let texture = thumbs.get(&path);
                        let aspect = texture
                            .map(|t| t.size_vec2().x / t.size_vec2().y.max(1.0))
                            .unwrap_or(1.5)
                            .clamp(0.5, 2.2);
                        let (slot, resp) = ui.allocate_exact_size(
                            egui::vec2(cell_h * aspect, cell_h + 8.0),
                            egui::Sense::click(),
                        );
                        // The current photo is lifted and ringed.
                        let lift = if is_current {
                            0.0
                        } else if resp.hovered() {
                            2.0
                        } else {
                            4.0
                        };
                        let rect = egui::Rect::from_min_size(
                            slot.min + egui::vec2(0.0, lift),
                            egui::vec2(slot.width(), cell_h),
                        );
                        let painter = ui.painter();
                        let photo = photo_for(state, &path)
                            .map(|p| (p.annotations, p.edited, p.color_label));
                        let rejected =
                            photo.is_some_and(|(a, _, _)| a.flag == sp_catalog::Flag::Reject);
                        match texture {
                            Some(tex) => {
                                painter.rect_filled(rect, 4.0, theme::BG_RAISED);
                                painter.image(
                                    tex.id(),
                                    rect,
                                    egui::Rect::from_min_max(
                                        egui::pos2(0.0, 0.0),
                                        egui::pos2(1.0, 1.0),
                                    ),
                                    if rejected {
                                        egui::Color32::from_gray(90)
                                    } else {
                                        egui::Color32::WHITE
                                    },
                                );
                            }
                            None if state.thumbnail_failures.contains_key(&path) => {
                                painter.rect_filled(rect, 4.0, theme::BG_RAISED);
                                paint_no_preview(
                                    painter,
                                    rect,
                                    &path,
                                    state
                                        .thumbnail_failures
                                        .get(&path)
                                        .is_some_and(|f| f.missing),
                                    true,
                                );
                            }
                            None => {
                                if !state.thumbnail_requests.contains(&path) {
                                    state.thumbnail_requests.push(path.clone());
                                }
                                // Loading shimmer with the file type.
                                let t = ui.input(|i| i.time) as f32;
                                let phase = (t * 1.5).fract();
                                painter.rect_filled(rect, 4.0, theme::BG_RAISED);
                                let band_x =
                                    egui::lerp(rect.left() - 30.0..=rect.right() + 30.0, phase);
                                painter.with_clip_rect(rect).rect_filled(
                                    egui::Rect::from_center_size(
                                        egui::pos2(band_x, rect.center().y),
                                        egui::vec2(30.0, rect.height()),
                                    ),
                                    0.0,
                                    egui::Color32::from_white_alpha(10),
                                );
                                let kind = path
                                    .extension()
                                    .and_then(|e| e.to_str())
                                    .unwrap_or("RAW")
                                    .to_ascii_uppercase();
                                painter.text(
                                    rect.center(),
                                    egui::Align2::CENTER_CENTER,
                                    kind,
                                    egui::FontId::proportional(10.0),
                                    theme::TEXT_FAINT,
                                );
                                ui.ctx()
                                    .request_repaint_after(std::time::Duration::from_millis(50));
                            }
                        }
                        if let Some((annotations, edited, color_label)) = photo {
                            if annotations.rating > 0 {
                                painter.rect_filled(
                                    egui::Rect::from_min_max(
                                        egui::pos2(rect.left(), rect.bottom() - 13.0),
                                        rect.max,
                                    ),
                                    egui::CornerRadius {
                                        nw: 0,
                                        ne: 0,
                                        sw: 4,
                                        se: 4,
                                    },
                                    egui::Color32::from_black_alpha(150),
                                );
                                paint_stars(
                                    painter,
                                    egui::pos2(rect.left() + 3.0, rect.bottom() - 6.5),
                                    annotations.rating,
                                    8.5,
                                    false,
                                );
                            }
                            paint_flag(
                                painter,
                                rect.left_top() + egui::vec2(8.0, 8.0),
                                annotations.flag,
                                5.5,
                            );
                            if edited {
                                painter.circle_filled(
                                    rect.right_top() + egui::vec2(-6.0, 6.0),
                                    2.5,
                                    egui::Color32::WHITE,
                                );
                            }
                            if let Some(color) = theme::label_color(color_label) {
                                painter.rect_filled(
                                    egui::Rect::from_min_max(
                                        egui::pos2(rect.left() + 3.0, rect.bottom() - 2.5),
                                        egui::pos2(rect.right() - 3.0, rect.bottom()),
                                    ),
                                    1.0,
                                    color,
                                );
                            }
                        }
                        if selected {
                            painter.rect_filled(rect, 4.0, theme::ACCENT.gamma_multiply(0.25));
                            painter.rect_stroke(
                                rect,
                                4.0,
                                egui::Stroke::new(1.0, theme::ACCENT),
                                egui::StrokeKind::Inside,
                            );
                        }
                        if is_current {
                            painter.rect_stroke(
                                rect.expand(1.0),
                                5.0,
                                egui::Stroke::new(2.0, theme::ACCENT),
                                egui::StrokeKind::Outside,
                            );
                            if last_current.as_ref() != Some(&path) {
                                resp.scroll_to_me(Some(egui::Align::Center));
                                ui.data_mut(|d| d.insert_temp(last_current_id, path.clone()));
                            }
                        }
                        if resp.clicked() {
                            let multi = ui.input(|input| input.modifiers.command);
                            if multi {
                                if state.selected_photos.is_empty() {
                                    if let Some(open) = &state.open_image {
                                        state.selected_photos.insert(open.clone());
                                    }
                                }
                                if !state.selected_photos.remove(&path) {
                                    state.selected_photos.insert(path.clone());
                                }
                            } else {
                                state.selected_photos.clear();
                                state.selected_photos.insert(path.clone());
                                if !is_current {
                                    state.filmstrip_clicked = Some(path.clone());
                                }
                            }
                        }
                        resp.on_hover_text(thumbnail_hover_text(state, &path));
                    }
                });
            });
    });
}

// ------------------------------------------------------------ filter bar

fn filtered_photos(state: &AppState) -> Vec<sp_catalog::CatalogPhoto> {
    let filter = &state.library_filter;
    let mut collections = std::collections::HashSet::new();
    if let Some(id) = state.library_collection {
        collections.insert(id);
    }
    loop {
        let before = collections.len();
        for collection in &state.catalog_collections {
            if collection
                .parent_id
                .is_some_and(|parent| collections.contains(&parent))
            {
                collections.insert(collection.id);
            }
        }
        if before == collections.len() {
            break;
        }
    }
    let mut photos: Vec<_> = state
        .catalog_photos
        .iter()
        .filter(|photo| {
            state.library_collection.is_none()
                || photo
                    .collection_ids
                    .iter()
                    .any(|id| collections.contains(id))
        })
        .filter(|photo| photo.annotations.rating >= filter.minimum_rating)
        .filter(|photo| {
            filter
                .flag
                .is_none_or(|flag| photo.annotations.flag == flag)
        })
        .filter(|photo| sp_catalog::search::matches(photo, &filter.text))
        .filter(|photo| {
            filter.keyword.as_ref().is_none_or(|keyword| {
                photo
                    .keywords
                    .iter()
                    .any(|value| sp_catalog::search::keyword_matches(value, keyword))
            })
        })
        .filter(|photo| {
            filter
                .color_label
                .is_none_or(|label| photo.color_label == label)
        })
        .filter(|photo| {
            filter
                .iso_min
                .is_none_or(|iso| photo.iso.is_some_and(|v| v >= iso))
        })
        .filter(|photo| {
            filter
                .iso_max
                .is_none_or(|iso| photo.iso.is_some_and(|v| v <= iso))
        })
        .filter(|photo| {
            filter
                .camera_model
                .as_ref()
                .is_none_or(|c| &photo.camera_model == c)
        })
        .filter(|photo| {
            filter
                .lens_model
                .as_ref()
                .is_none_or(|l| &photo.lens_model == l)
        })
        .filter(|photo| {
            filter
                .date_from
                .as_ref()
                .is_none_or(|date| photo.captured_at.as_ref().is_some_and(|v| v >= date))
        })
        .filter(|photo| {
            filter
                .date_to
                .as_ref()
                .is_none_or(|date| photo.captured_at.as_ref().is_some_and(|v| v <= date))
        })
        .filter(|photo| {
            filter
                .file_type
                .as_ref()
                .is_none_or(|kind| &photo.file_type == kind)
        })
        .cloned()
        .collect();
    match state.library_sort {
        sp_catalog::LibrarySort::CaptureTime => photos.sort_by(|a, b| {
            a.captured_at
                .cmp(&b.captured_at)
                .then_with(|| a.path.cmp(&b.path))
        }),
        sp_catalog::LibrarySort::Name => photos.sort_by(|a, b| a.path.cmp(&b.path)),
        sp_catalog::LibrarySort::Rating => photos.sort_by(|a, b| {
            b.annotations
                .rating
                .cmp(&a.annotations.rating)
                .then_with(|| a.path.cmp(&b.path))
        }),
    }
    if state.detail_sort {
        photos.sort_by(|a, b| {
            state
                .preview_quality
                .get(&b.path)
                .map_or(-1.0, |q| q.detail)
                .total_cmp(
                    &state
                        .preview_quality
                        .get(&a.path)
                        .map_or(-1.0, |q| q.detail),
                )
        });
    }
    if let Some(reference) = state
        .similarity_reference
        .as_ref()
        .and_then(|path| state.preview_quality.get(path))
    {
        photos.retain(|photo| {
            state
                .preview_quality
                .get(&photo.path)
                .is_some_and(|quality| quality.similarity(*reference) >= 0.88)
        });
    }
    let mut representatives = std::collections::HashMap::new();
    for photo in &photos {
        if let Some(stack) = photo.stack_id {
            if photo.stack_cover || !representatives.contains_key(&stack) {
                representatives.insert(stack, photo.path.clone());
            }
        }
    }
    photos.retain(|photo| {
        photo.stack_id.is_none_or(|stack| {
            state.expanded_stacks.contains(&stack)
                || representatives.get(&stack) == Some(&photo.path)
        })
    });
    photos
}

const LABELS: [(sp_catalog::ColorLabel, &str); 5] = [
    (sp_catalog::ColorLabel::Red, "Red"),
    (sp_catalog::ColorLabel::Yellow, "Yellow"),
    (sp_catalog::ColorLabel::Green, "Green"),
    (sp_catalog::ColorLabel::Blue, "Blue"),
    (sp_catalog::ColorLabel::Purple, "Purple"),
];

fn filter_bar(ui: &mut egui::Ui, state: &mut AppState) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        ui.add(
            egui::TextEdit::singleline(&mut state.library_filter.text)
                .hint_text(crate::i18n::tr(
                    state.language,
                    crate::i18n::Text::SearchPhotos,
                ))
                .desired_width(150.0),
        ).on_hover_text(crate::i18n::text("Words combine with AND. Use OR, -word, quoted phrases, camera:, lens:, caption:, keyword:, type:, rating:>=4, iso:<=800, flag:pick. Hierarchical keywords use Parent|Child."));
        // Minimum rating: click a star; click the same star again to clear.
        let (rect, resp) =
            ui.allocate_exact_size(egui::vec2(5.0 * 15.0, 22.0), egui::Sense::click());
        let hover_star = resp
            .hover_pos()
            .map(|p| ((p.x - rect.left()) / 15.0).floor() as u8 + 1);
        let shown = hover_star.unwrap_or(state.library_filter.minimum_rating);
        for i in 0..5u8 {
            ui.painter().text(
                egui::pos2(rect.left() + f32::from(i) * 15.0 + 7.0, rect.center().y),
                egui::Align2::CENTER_CENTER,
                "★",
                egui::FontId::proportional(14.0),
                if i < shown {
                    theme::STAR
                } else {
                    theme::TEXT_FAINT.gamma_multiply(0.5)
                },
            );
        }
        if resp.clicked() {
            if let Some(star) = hover_star {
                state.library_filter.minimum_rating = if state.library_filter.minimum_rating == star
                {
                    0
                } else {
                    star.min(5)
                };
            }
        }
        resp.on_hover_text(crate::i18n::text("Minimum rating · click again to clear"));
        // Flags.
        for (flag, tip) in [
            (sp_catalog::Flag::Pick, "Picked only"),
            (sp_catalog::Flag::Reject, "Rejected only"),
            (sp_catalog::Flag::None, "Unflagged only"),
        ] {
            let on = state.library_filter.flag == Some(flag);
            let (rect, resp) = ui.allocate_exact_size(egui::vec2(22.0, 22.0), egui::Sense::click());
            if on || resp.hovered() {
                ui.painter().rect_filled(
                    rect,
                    5.0,
                    if on {
                        theme::ACCENT.gamma_multiply(0.3)
                    } else {
                        theme::BG_HOVER
                    },
                );
            }
            if flag == sp_catalog::Flag::None {
                ui.painter().circle_stroke(
                    rect.center(),
                    6.0,
                    egui::Stroke::new(1.2, theme::TEXT_SECONDARY),
                );
            } else {
                paint_flag(ui.painter(), rect.center(), flag, 6.5);
            }
            if resp.on_hover_text(tip).clicked() {
                state.library_filter.flag = if on { None } else { Some(flag) };
            }
        }
        // Color labels.
        for (label, name) in LABELS {
            let (rect, resp) = ui.allocate_exact_size(egui::vec2(14.0, 22.0), egui::Sense::click());
            let on = state.library_filter.color_label == Some(label);
            let color = theme::label_color(label).expect("labelled");
            ui.painter().circle_filled(
                rect.center(),
                if on || resp.hovered() { 5.5 } else { 4.5 },
                if on { color } else { color.gamma_multiply(0.7) },
            );
            if on {
                ui.painter()
                    .circle_stroke(rect.center(), 7.0, egui::Stroke::new(1.5, color));
            }
            if resp.clicked() {
                state.library_filter.color_label = if on { None } else { Some(label) };
            }
            resp.on_hover_text(crate::i18n::format("{0} label only", &[name.to_string()]));
        }
        more_filters_menu(ui, state);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            egui::ComboBox::from_id_salt("library-sort")
                .selected_text(match state.library_sort {
                    sp_catalog::LibrarySort::CaptureTime => "Capture time",
                    sp_catalog::LibrarySort::Name => "Name",
                    sp_catalog::LibrarySort::Rating => "Rating",
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut state.library_sort,
                        sp_catalog::LibrarySort::CaptureTime,
                        crate::i18n::text("Capture time"),
                    );
                    ui.selectable_value(
                        &mut state.library_sort,
                        sp_catalog::LibrarySort::Name,
                        crate::i18n::text("Name"),
                    );
                    ui.selectable_value(
                        &mut state.library_sort,
                        sp_catalog::LibrarySort::Rating,
                        crate::i18n::text("Rating"),
                    );
                });
        });
    });
}

fn more_filters_menu(ui: &mut egui::Ui, state: &mut AppState) {
    ui.menu_button(crate::i18n::text("More filters"), |ui| {
        ui.set_min_width(260.0);
        ui.label(theme::section_text("File type"));
        ui.horizontal_wrapped(|ui| {
            if ui
                .selectable_label(
                    state.library_filter.file_type.is_none(),
                    crate::i18n::text("All"),
                )
                .clicked()
            {
                state.library_filter.file_type = None;
            }
            for extension in sp_raw::SUPPORTED_EXTENSIONS {
                let on = state.library_filter.file_type.as_deref() == Some(*extension);
                if ui
                    .selectable_label(on, extension.to_ascii_uppercase())
                    .clicked()
                {
                    state.library_filter.file_type =
                        if on { None } else { Some((*extension).into()) };
                }
            }
        });
        ui.separator();
        let mut cameras: Vec<_> = state
            .catalog_photos
            .iter()
            .map(|p| p.camera_model.clone())
            .filter(|v| !v.is_empty())
            .collect();
        cameras.sort();
        cameras.dedup();
        egui::ComboBox::from_id_salt("library-camera")
            .selected_text(
                state
                    .library_filter
                    .camera_model
                    .as_deref()
                    .unwrap_or("Any camera"),
            )
            .show_ui(ui, |ui| {
                ui.selectable_value(
                    &mut state.library_filter.camera_model,
                    None,
                    crate::i18n::text("Any camera"),
                );
                for camera in &cameras {
                    ui.selectable_value(
                        &mut state.library_filter.camera_model,
                        Some(camera.clone()),
                        camera,
                    );
                }
            });
        let mut lenses: Vec<_> = state
            .catalog_photos
            .iter()
            .map(|p| p.lens_model.clone())
            .filter(|v| !v.is_empty())
            .collect();
        lenses.sort();
        lenses.dedup();
        egui::ComboBox::from_id_salt("library-lens")
            .selected_text(
                state
                    .library_filter
                    .lens_model
                    .as_deref()
                    .unwrap_or("Any lens"),
            )
            .show_ui(ui, |ui| {
                ui.selectable_value(
                    &mut state.library_filter.lens_model,
                    None,
                    crate::i18n::text("Any lens"),
                );
                for lens in &lenses {
                    ui.selectable_value(
                        &mut state.library_filter.lens_model,
                        Some(lens.clone()),
                        lens,
                    );
                }
            });
        ui.horizontal(|ui| {
            ui.label(crate::i18n::text("ISO"));
            let mut minimum = state.library_filter.iso_min.unwrap_or(0);
            let mut maximum = state.library_filter.iso_max.unwrap_or(0);
            if ui
                .add(
                    egui::DragValue::new(&mut minimum)
                        .range(0..=409_600)
                        .prefix("min "),
                )
                .changed()
            {
                state.library_filter.iso_min = (minimum != 0).then_some(minimum);
            }
            if ui
                .add(
                    egui::DragValue::new(&mut maximum)
                        .range(0..=409_600)
                        .prefix("max "),
                )
                .changed()
            {
                state.library_filter.iso_max = (maximum != 0).then_some(maximum);
            }
        });
        ui.horizontal(|ui| {
            ui.label(crate::i18n::text("Date"));
            let mut from = state.library_filter.date_from.clone().unwrap_or_default();
            let mut to = state.library_filter.date_to.clone().unwrap_or_default();
            if ui
                .add(
                    egui::TextEdit::singleline(&mut from)
                        .desired_width(92.0)
                        .hint_text(crate::i18n::text("YYYY-MM-DD")),
                )
                .changed()
            {
                state.library_filter.date_from = (!from.trim().is_empty()).then_some(from);
            }
            if ui
                .add(
                    egui::TextEdit::singleline(&mut to)
                        .desired_width(92.0)
                        .hint_text(crate::i18n::text("YYYY-MM-DD")),
                )
                .changed()
            {
                state.library_filter.date_to = (!to.trim().is_empty()).then_some(to);
            }
        });
        ui.separator();
        ui.label(theme::section_text("Smart collections"));
        for collection in &state.catalog_smart_collections {
            if ui.button(&collection.name).clicked() {
                state.library_filter = collection.filter.clone();
                ui.close();
            }
        }
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut state.new_smart_collection_name)
                    .hint_text(crate::i18n::text("Name"))
                    .desired_width(140.0),
            );
            if ui
                .add_enabled(
                    !state.new_smart_collection_name.trim().is_empty(),
                    egui::Button::new(crate::i18n::text("Save filters")),
                )
                .clicked()
            {
                state.smart_collection_save_requested = true;
                ui.close();
            }
        });
    });
}

/// Active filters as removable chips, with the visible count.
fn filter_chips(ui: &mut egui::Ui, state: &mut AppState, shown: usize) {
    let filter = state.library_filter.clone();
    type Remove = Box<dyn Fn(&mut sp_catalog::LibraryFilter)>;
    let mut chips: Vec<(String, Remove)> = Vec::new();
    if let Some(keyword) = &filter.keyword {
        chips.push((
            crate::i18n::format("Keyword: {0}", std::slice::from_ref(keyword)),
            Box::new(|f| f.keyword = None),
        ));
    }
    if !filter.text.trim().is_empty() {
        chips.push((
            format!("“{}”", filter.text.trim()),
            Box::new(|f| f.text.clear()),
        ));
    }
    if filter.minimum_rating > 0 {
        chips.push((
            format!("★ ≥ {}", filter.minimum_rating),
            Box::new(|f| f.minimum_rating = 0),
        ));
    }
    if let Some(flag) = filter.flag {
        let name = match flag {
            sp_catalog::Flag::Pick => "Picked",
            sp_catalog::Flag::Reject => "Rejected",
            sp_catalog::Flag::None => "Unflagged",
        };
        chips.push((name.into(), Box::new(|f| f.flag = None)));
    }
    if let Some(label) = filter.color_label {
        let name = LABELS
            .iter()
            .find(|(l, _)| *l == label)
            .map_or("No label", |(_, n)| n);
        chips.push((
            crate::i18n::format("{0} label", &[name.to_string()]),
            Box::new(|f| f.color_label = None),
        ));
    }
    if let Some(kind) = &filter.file_type {
        chips.push((
            {
                let __l10n_args = (&(kind.to_ascii_uppercase()),);
                crate::i18n::format("Type {0}", std::slice::from_ref(__l10n_args.0))
            },
            Box::new(|f| f.file_type = None),
        ));
    }
    if let Some(camera) = &filter.camera_model {
        chips.push((camera.clone(), Box::new(|f| f.camera_model = None)));
    }
    if let Some(lens) = &filter.lens_model {
        chips.push((lens.clone(), Box::new(|f| f.lens_model = None)));
    }
    if filter.iso_min.is_some() || filter.iso_max.is_some() {
        chips.push((
            format!(
                "ISO {}–{}",
                filter.iso_min.unwrap_or(0),
                filter.iso_max.map_or("∞".into(), |v| v.to_string())
            ),
            Box::new(|f| {
                f.iso_min = None;
                f.iso_max = None;
            }),
        ));
    }
    if filter.date_from.is_some() || filter.date_to.is_some() {
        chips.push((
            format!(
                "{} → {}",
                filter.date_from.clone().unwrap_or_default(),
                filter.date_to.clone().unwrap_or_default()
            ),
            Box::new(|f| {
                f.date_from = None;
                f.date_to = None;
            }),
        ));
    }
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 5.0;
        let mut remove = None;
        for (index, (text, _)) in chips.iter().enumerate() {
            let resp = ui.add(
                egui::Button::new(
                    egui::RichText::new(format!("{text}   ×"))
                        .size(11.5)
                        .color(theme::TEXT_PRIMARY),
                )
                .fill(theme::ACCENT.gamma_multiply(0.16))
                .stroke(egui::Stroke::new(1.0, theme::ACCENT.gamma_multiply(0.5)))
                .corner_radius(11.0),
            );
            if resp
                .on_hover_text(crate::i18n::text("Remove this filter"))
                .clicked()
            {
                remove = Some(index);
            }
        }
        if let Some(index) = remove {
            (chips[index].1)(&mut state.library_filter);
        }
        let total = state.catalog_photos.len();
        ui.label(
            egui::RichText::new(crate::i18n::text(
                &(if chips.is_empty() {
                    crate::i18n::format("{0} photos", &[total.to_string()])
                } else {
                    crate::i18n::format(
                        "Showing {0} of {1}",
                        &[shown.to_string(), total.to_string()],
                    )
                }),
            ))
            .size(11.5)
            .color(theme::TEXT_SECONDARY),
        );
        if !chips.is_empty() && ui.small_button(crate::i18n::text("Clear")).clicked() {
            state.library_filter = sp_catalog::LibraryFilter::default();
        }
    });
}

/// Selection action bar: shown only while photos are selected.
fn selection_bar(ui: &mut egui::Ui, state: &mut AppState) {
    let count = state.selected_photos.len();
    if count == 0 {
        return;
    }
    egui::Frame::new()
        .fill(theme::BG_CARD)
        .stroke(egui::Stroke::new(1.0, theme::ACCENT.gamma_multiply(0.5)))
        .corner_radius(theme::RADIUS_PANEL)
        .inner_margin(egui::Margin::symmetric(10, 5))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(crate::i18n::format("{0} selected", &[count.to_string()]))
                        .strong(),
                );
                if state.catalog_scanning || state.catalog_tool_busy {
                    ui.disable();
                }
                let rename = ui.button(crate::i18n::text("Rename…"));
                egui::Popup::menu(&rename).show(|ui| rename_popover(ui, state));
                if ui.button(crate::i18n::text("Move…")).clicked() {
                    state.file_move_requested = true;
                }
                if ui.button(crate::i18n::text("Trash…")).clicked() {
                    state.file_trash_confirm_open = true;
                }
                if (2..=4).contains(&count)
                    && ui
                        .button(crate::i18n::text("Compare"))
                        .on_hover_text(crate::i18n::text("Side by side · C"))
                        .clicked()
                {
                    state.culling_requested = true;
                }
                if state.file_move_undo_available
                    && ui.button(crate::i18n::text("Undo move")).clicked()
                {
                    state.file_move_undo_requested = true;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .button(crate::i18n::text("×"))
                        .on_hover_text(crate::i18n::text("Clear selection · Esc"))
                        .clicked()
                    {
                        state.selected_photos.clear();
                    }
                    ui.label(
                        egui::RichText::new(crate::i18n::text("0–5 rate · P/X flag · 6–9 label"))
                            .size(10.5)
                            .color(theme::TEXT_FAINT),
                    );
                });
            });
        });
}

fn rename_popover(ui: &mut egui::Ui, state: &mut AppState) {
    ui.set_width(320.0);
    let mut sources: Vec<_> = state
        .catalog_photos
        .iter()
        .filter(|photo| state.selected_photos.contains(&photo.path))
        .map(|photo| (photo.path.clone(), photo.captured_at.clone()))
        .collect();
    sources.sort_by(|a, b| a.0.cmp(&b.0));
    ui.label(theme::section_text(&{
        let __l10n_args = (&(sources.len()),);
        crate::i18n::format("Rename {0} photos", &[__l10n_args.0.to_string()])
    }));
    ui.add(
        egui::TextEdit::singleline(&mut state.file_rename_pattern)
            .hint_text(crate::i18n::text("{date}-{name}-{seq}"))
            .desired_width(f32::INFINITY),
    );
    ui.horizontal(|ui| {
        for token in ["{date}", "{name}", "{seq}"] {
            if ui.small_button(token).clicked() {
                if !state.file_rename_pattern.is_empty()
                    && !state.file_rename_pattern.ends_with('-')
                {
                    state.file_rename_pattern.push('-');
                }
                state.file_rename_pattern.push_str(token);
            }
        }
    });
    let pattern = if state.file_rename_pattern.trim().is_empty() {
        "{date}-{name}-{seq}".to_owned()
    } else {
        state.file_rename_pattern.clone()
    };
    egui::Frame::new()
        .fill(theme::BG_VIEWPORT)
        .corner_radius(theme::RADIUS_CONTROL)
        .inner_margin(6)
        .show(ui, |ui| {
            for (index, (source, captured)) in sources.iter().take(3).enumerate() {
                let old = source.file_name().and_then(|n| n.to_str()).unwrap_or("?");
                let new = sp_catalog::workflow::renamed_file_name(
                    source,
                    &pattern,
                    index + 1,
                    captured.as_deref(),
                );
                ui.label(
                    egui::RichText::new(format!("{old}  →  {new}"))
                        .monospace()
                        .size(10.5),
                );
            }
            if sources.len() > 3 {
                ui.label(
                    egui::RichText::new({
                        let __l10n_args = (&(sources.len() - 3),);
                        crate::i18n::format("…and {0} more", &[__l10n_args.0.to_string()])
                    })
                    .size(10.5)
                    .color(theme::TEXT_FAINT),
                );
            }
        });
    if ui
        .add(
            egui::Button::new(
                egui::RichText::new(crate::i18n::text("Rename"))
                    .color(theme::ON_ACCENT)
                    .strong(),
            )
            .fill(theme::ACCENT),
        )
        .clicked()
    {
        state.file_rename_pattern = pattern;
        state.file_rename_requested = true;
        ui.close();
    }
}

// ------------------------------------------------------------------ grid

/// Select `path` in the grid: plain replaces, ⌘ toggles, ⇧ extends from
/// the anchor in grid order.
pub fn grid_select(state: &mut AppState, path: &std::path::Path, command: bool, shift: bool) {
    let path = path.to_path_buf();
    if shift {
        let order = &state.library_visible;
        let anchor = state.grid_anchor.clone().unwrap_or_else(|| path.clone());
        if let (Some(a), Some(b)) = (
            order.iter().position(|p| *p == anchor),
            order.iter().position(|p| *p == path),
        ) {
            if !command {
                state.selected_photos.clear();
            }
            let (lo, hi) = (a.min(b), a.max(b));
            for p in &order[lo..=hi] {
                state.selected_photos.insert(p.clone());
            }
        }
    } else if command {
        if !state.selected_photos.remove(&path) {
            state.selected_photos.insert(path.clone());
        }
        state.grid_anchor = Some(path.clone());
    } else {
        state.selected_photos.clear();
        state.selected_photos.insert(path.clone());
        state.grid_anchor = Some(path.clone());
    }
    state.grid_focus = Some(path);
}

pub fn library_grid(
    ui: &mut egui::Ui,
    state: &mut AppState,
    thumbs: &std::collections::HashMap<std::path::PathBuf, egui::TextureHandle>,
) {
    filter_bar(ui, state);
    if !state.preview_quality.is_empty() {
        ui.horizontal(|ui| {
            ui.checkbox(&mut state.detail_sort,crate::i18n::text("Sort by preview detail")).on_hover_text(crate::i18n::text("Advisory edge detail from source previews; camera sharpening and subject texture affect the score. Check focus at 100%."));
            if let Some(reference)=&state.similarity_reference {ui.label({ let __l10n_args = (&(reference.file_name().unwrap_or_default().to_string_lossy()),); crate::i18n::format("Similar to {0} · analyzed photos only", &[__l10n_args.0.to_string()]) });if ui.small_button(crate::i18n::text("Clear similarity")).clicked(){state.similarity_reference=None;}}
        });
    }
    let photos = filtered_photos(state);
    filter_chips(ui, state, photos.len());
    state.library_visible = photos.iter().map(|photo| photo.path.clone()).collect();
    if state.file_trash_confirm_open {
        let mut open = true;
        let mut confirm = false;
        let mut cancel = false;
        let count = state.selected_photos.len().max(1);
        egui::Window::new({ let __l10n_args = (&(if count == 1 { "" } else { "s" }),); crate::i18n::format("Move {0} photo{1} to the trash?", &[count.to_string(), __l10n_args.0.to_string()]) })
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .show(ui.ctx(), |ui| {
                ui.label(crate::i18n::text("The originals and their ShopPhoto/XMP sidecars will be moved to the operating system trash."));
                ui.horizontal(|ui| {
                    if ui.button(crate::i18n::text("Cancel")).clicked() {
                        cancel = true;
                    }
                    if ui.button(crate::i18n::text("Move to trash")).clicked() {
                        confirm = true;
                    }
                });
            });
        if confirm {
            state.file_trash_requested = true;
            open = false;
        } else if cancel {
            open = false;
        }
        state.file_trash_confirm_open = open;
    }
    ui.add_space(4.0);

    // Reserve room for the selection bar at the bottom.
    let bar_h = if state.selected_photos.is_empty() {
        0.0
    } else {
        40.0
    };
    let grid_rect =
        egui::Rect::from_min_max(ui.cursor().min, ui.max_rect().max - egui::vec2(0.0, bar_h));
    let mut grid_ui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(grid_rect)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    library_cells(&mut grid_ui, state, thumbs, &photos);
    if bar_h > 0.0 {
        let bar_rect = egui::Rect::from_min_max(
            egui::pos2(grid_rect.left(), grid_rect.bottom() + 4.0),
            ui.max_rect().max,
        );
        let mut bar_ui = ui.new_child(egui::UiBuilder::new().max_rect(bar_rect));
        selection_bar(&mut bar_ui, state);
    }
}

fn library_cells(
    ui: &mut egui::Ui,
    state: &mut AppState,
    thumbs: &std::collections::HashMap<std::path::PathBuf, egui::TextureHandle>,
    photos: &[sp_catalog::CatalogPhoto],
) {
    if photos.is_empty() {
        ui.centered_and_justified(|ui| {
            ui.label(
                egui::RichText::new(crate::i18n::text(if state.catalog_scanning {
                    "Scanning folder…"
                } else if state.catalog_photos.is_empty() {
                    "Add a folder to begin"
                } else {
                    "No photos match the current filters"
                }))
                .color(theme::TEXT_SECONDARY),
            );
        });
        return;
    }
    let cell_w = 166.0;
    let cell_h = 156.0;
    let columns = (ui.available_width() / cell_w).floor().max(1.0) as usize;
    ui.data_mut(|d| d.insert_temp(egui::Id::new("library-columns"), columns));
    let rows = photos.len().div_ceil(columns);
    let focus_row = state
        .grid_focus
        .as_ref()
        .and_then(|focus| photos.iter().position(|p| &p.path == focus))
        .map(|index| index / columns);
    let scroll_to_focus = ui
        .data_mut(|d| d.remove_temp::<bool>(egui::Id::new("grid-scroll-to-focus")))
        .unwrap_or(false);
    let mut scroll = egui::ScrollArea::vertical().auto_shrink([false, false]);
    if let (true, Some(row)) = (scroll_to_focus, focus_row) {
        let viewport_h = ui.available_height();
        let offset = ui
            .data(|d| d.get_temp::<f32>(egui::Id::new("grid-scroll-offset")))
            .unwrap_or(0.0);
        let top = row as f32 * cell_h;
        if top < offset {
            scroll = scroll.vertical_scroll_offset(top);
        } else if top + cell_h > offset + viewport_h {
            scroll = scroll.vertical_scroll_offset(top + cell_h - viewport_h);
        }
    }
    let output = scroll.show_rows(ui, cell_h, rows, |ui, row_range| {
        for row in row_range {
            ui.horizontal(|ui| {
                for column in 0..columns {
                    let index = row * columns + column;
                    let Some(photo) = photos.get(index) else {
                        break;
                    };
                    grid_cell(
                        ui,
                        state,
                        thumbs,
                        photo,
                        egui::vec2(cell_w - 8.0, cell_h - 8.0),
                    );
                }
            });
        }
    });
    ui.data_mut(|d| d.insert_temp(egui::Id::new("grid-scroll-offset"), output.state.offset.y));
}

fn grid_cell(
    ui: &mut egui::Ui,
    state: &mut AppState,
    thumbs: &std::collections::HashMap<std::path::PathBuf, egui::TextureHandle>,
    photo: &sp_catalog::CatalogPhoto,
    size: egui::Vec2,
) {
    if !state.visible_thumbnail_paths.contains(&photo.path) {
        state.visible_thumbnail_paths.push(photo.path.clone());
    }
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click_and_drag());
    let selected = state.selected_photos.contains(&photo.path);
    let focused = state.grid_focus.as_ref() == Some(&photo.path);
    let hovered = response.hovered();
    let painter = ui.painter_at(rect.expand(3.0));
    painter.rect_filled(
        rect,
        6.0,
        if selected {
            theme::ACCENT.gamma_multiply(0.18)
        } else if hovered {
            theme::BG_HOVER
        } else {
            theme::BG_CARD
        },
    );
    let image_rect = egui::Rect::from_min_max(
        rect.min + egui::vec2(5.0, 5.0),
        egui::pos2(rect.max.x - 5.0, rect.max.y - 30.0),
    );
    let rejected = photo.annotations.flag == sp_catalog::Flag::Reject;
    if let Some(texture) = thumbs.get(&photo.path) {
        painter.image(
            texture.id(),
            contain_rect(texture.size_vec2(), image_rect),
            egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
            if rejected {
                egui::Color32::from_gray(80)
            } else {
                egui::Color32::WHITE
            },
        );
    } else if state.thumbnail_failures.contains_key(&photo.path) {
        paint_no_preview(
            &painter,
            image_rect,
            &photo.path,
            state
                .thumbnail_failures
                .get(&photo.path)
                .is_some_and(|f| f.missing),
            false,
        );
    } else {
        if !state.thumbnail_requests.contains(&photo.path) {
            state.thumbnail_requests.push(photo.path.clone());
        }
        painter.text(
            image_rect.center(),
            egui::Align2::CENTER_CENTER,
            photo.file_type.to_ascii_uppercase(),
            egui::FontId::proportional(11.0),
            theme::TEXT_FAINT,
        );
    }
    paint_flag(
        &painter,
        image_rect.left_top() + egui::vec2(9.0, 9.0),
        photo.annotations.flag,
        7.5,
    );
    if photo.edited {
        painter.circle_filled(
            image_rect.right_top() + egui::vec2(-7.0, 7.0),
            3.0,
            egui::Color32::WHITE,
        );
        painter.circle_stroke(
            image_rect.right_top() + egui::vec2(-7.0, 7.0),
            3.0,
            egui::Stroke::new(1.0, egui::Color32::from_black_alpha(140)),
        );
    }
    if photo.companion_of.is_some() {
        pill_text(
            &painter,
            image_rect.right_bottom() + egui::vec2(-4.0, -4.0),
            "TIFF",
        );
    }
    if let Some(operation) = &photo.derived_operation {
        pill_text(
            &painter,
            image_rect.right_bottom() + egui::vec2(-4.0, -4.0),
            operation,
        );
    }
    if !photo.virtual_copies.is_empty() {
        pill_text(
            &painter,
            image_rect.left_bottom() + egui::vec2(80.0, -4.0),
            &{
                let __l10n_args = (&(photo.virtual_copies.len() + 1),);
                crate::i18n::format("{0} versions", &[__l10n_args.0.to_string()])
            },
        );
    }
    if photo.stack_size > 1 {
        pill_text(&painter, image_rect.left_top() + egui::vec2(50.0, 14.0), &{
            let __l10n_args = (&(photo.stack_size),);
            crate::i18n::format("Stack {0}", &[__l10n_args.0.to_string()])
        });
    }
    // Footer: clickable stars (on hover or when rated) and the file name.
    let stars_origin = egui::pos2(rect.left() + 7.0, rect.bottom() - 16.0);
    let star_w = 12.0;
    let stars_rect = egui::Rect::from_min_size(
        stars_origin - egui::vec2(0.0, 8.0),
        egui::vec2(star_w * 5.0, 16.0),
    );
    let star_hover = ui
        .input(|i| i.pointer.hover_pos())
        .filter(|p| hovered && stars_rect.contains(*p))
        .map(|p| ((p.x - stars_rect.left()) / star_w).floor() as u8 + 1);
    let shown = star_hover.unwrap_or(photo.annotations.rating);
    if hovered || selected || photo.annotations.rating > 0 {
        for i in 0..5u8 {
            painter.text(
                stars_origin + egui::vec2(f32::from(i) * star_w, 0.0),
                egui::Align2::LEFT_CENTER,
                "★",
                egui::FontId::proportional(11.5),
                if i < shown {
                    theme::STAR
                } else {
                    theme::TEXT_FAINT.gamma_multiply(0.45)
                },
            );
        }
    }
    let name = photo
        .path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");
    // Stars take the left of the footer only while they are painted; the
    // name gets the rest and is shortened in the middle so the start of the
    // name and its extension both stay readable.
    let name_left = if hovered || selected || photo.annotations.rating > 0 {
        stars_rect.right() + 6.0
    } else {
        rect.left() + 7.0
    };
    let name_font = egui::FontId::proportional(10.0);
    let shown_name = elide_middle(ui, name, &name_font, rect.right() - 6.0 - name_left);
    painter.text(
        egui::pos2(rect.right() - 6.0, stars_origin.y),
        egui::Align2::RIGHT_CENTER,
        shown_name,
        name_font,
        theme::TEXT_FAINT,
    );
    if let Some(color) = theme::label_color(photo.color_label) {
        painter.rect_filled(
            egui::Rect::from_min_max(
                egui::pos2(rect.left() + 6.0, rect.bottom() - 4.0),
                egui::pos2(rect.right() - 6.0, rect.bottom() - 1.5),
            ),
            1.0,
            color,
        );
    }
    if selected {
        painter.rect_stroke(
            rect,
            6.0,
            egui::Stroke::new(1.5, theme::ACCENT.gamma_multiply(0.8)),
            egui::StrokeKind::Inside,
        );
    }
    if focused {
        painter.rect_stroke(
            rect.expand(2.0),
            7.0,
            egui::Stroke::new(2.0, theme::ACCENT),
            egui::StrokeKind::Outside,
        );
    }

    if response.drag_started() {
        state.dragged_photo = Some(photo.path.clone());
    }
    response.context_menu(|ui| {
        if let Some(quality) = state.preview_quality.get(&photo.path) {
            ui.label({ let __l10n_args = (&(quality.detail), &(quality.highlights_percent), &(quality.shadows_percent),); crate::i18n::format("Preview detail {0} · whites {1}% · blacks {2}%", &[format!("{:.0}", __l10n_args.0), format!("{:.1}", __l10n_args.1), format!("{:.1}", __l10n_args.2)]) });
            if let Some(faces)=quality.faces {
                ui.label({ let __l10n_args = (&(faces.faces), &(faces.eye_regions), &(faces.weakest_eye_detail.map_or_else(||"unavailable".into(),|v|format!("{v:.0}"))),); crate::i18n::format("{0} faces · {1} eye regions · weakest eye detail {2}", &[__l10n_args.0.to_string(), __l10n_args.1.to_string(), __l10n_args.2.to_string()]) });
                ui.small(crate::i18n::text("Preview detections can miss faces or eyes. No blink or closed-eye verdict; inspect at 100%."));
            }
            if ui
                .button(crate::i18n::text("Show similar analyzed photos"))
                .clicked()
            {
                state.similarity_reference = Some(photo.path.clone());
                ui.close();
            }
        }
        if let Some(stack) = photo.stack_id {
            if ui
                .button(if state.expanded_stacks.contains(&stack) {
                    "Collapse stack"
                } else {
                    "Expand stack"
                })
                .clicked()
            {
                if !state.expanded_stacks.remove(&stack) {
                    state.expanded_stacks.insert(stack);
                }
                ui.close();
            }
            if ui
                .add_enabled(
                    !state.catalog_tool_busy && !state.catalog_scanning,
                    egui::Button::new(crate::i18n::text("Use as stack cover")),
                )
                .clicked()
            {
                state.grid_focus = Some(photo.path.clone());
                state.catalog_tool_requested =
                    Some(crate::app::catalog_tools::Action::SetStackCover(stack));
                ui.close();
            }
            if ui
                .add_enabled(
                    !state.catalog_tool_busy && !state.catalog_scanning,
                    egui::Button::new(crate::i18n::text("Unstack photos")),
                )
                .clicked()
            {
                state.catalog_tool_requested =
                    Some(crate::app::catalog_tools::Action::DissolveStack(stack));
                ui.close();
            }
        } else if state.selected_photos.len() >= 2
            && ui
                .add_enabled(
                    !state.catalog_tool_busy && !state.catalog_scanning,
                    egui::Button::new(crate::i18n::text("Stack selected photos")),
                )
                .clicked()
        {
            state.catalog_tool_requested = Some(crate::app::catalog_tools::Action::StackSelected);
            ui.close();
        }
        let derivatives: Vec<_> = state
            .catalog_photos
            .iter()
            .filter(|candidate| {
                candidate.companion_of.as_ref() == Some(&photo.path)
                    || candidate.derived_sources.contains(&photo.path)
            })
            .map(|candidate| candidate.path.clone())
            .collect();
        if !derivatives.is_empty() {
            ui.menu_button(crate::i18n::text("Derived photos"), |ui| {
                for derivative in derivatives {
                    if ui
                        .button(derivative.file_name().unwrap_or_default().to_string_lossy())
                        .clicked()
                    {
                        state.filmstrip_clicked = Some(derivative.clone());
                        state.mode = Mode::Develop;
                        ui.close();
                    }
                }
            });
        }
        if ui.button(crate::i18n::text("Open in Develop")).clicked() {
            state.filmstrip_clicked = Some(photo.path.clone());
            state.mode = Mode::Develop;
            ui.close();
        }
        if let Some(source) = &photo.companion_of {
            if ui
                .button(crate::i18n::text("Open original photo"))
                .clicked()
            {
                state.filmstrip_clicked = Some(source.clone());
                state.mode = Mode::Develop;
                ui.close();
            }
        }
        if !photo.derived_sources.is_empty() {
            ui.menu_button(crate::i18n::text("Source photos"), |ui| {
                for source in &photo.derived_sources {
                    if ui
                        .button(source.file_name().unwrap_or_default().to_string_lossy())
                        .on_hover_text(source.display().to_string())
                        .clicked()
                    {
                        state.filmstrip_clicked = Some(source.clone());
                        state.mode = Mode::Develop;
                        ui.close();
                    }
                }
            });
        }
        if !photo.virtual_copies.is_empty() {
            ui.menu_button(crate::i18n::text("Versions"), |ui| {
                if ui.button(crate::i18n::text("Primary")).clicked() {
                    state.virtual_copy_open_requested = Some((photo.path.clone(), String::new()));
                    ui.close();
                }
                for copy in &photo.virtual_copies {
                    if ui.button(&copy.name).clicked() {
                        state.virtual_copy_open_requested =
                            Some((photo.path.clone(), copy.id.clone()));
                        ui.close();
                    }
                }
            });
        }
        ui.separator();
        ui.label(theme::section_text("Add to collection"));
        for collection in &state.catalog_collections {
            if ui.button(&collection.name).clicked() {
                state.collection_add_requested = Some((collection.id, photo.path.clone()));
                ui.close();
            }
        }
    });
    if response.double_clicked() {
        state.filmstrip_clicked = Some(photo.path.clone());
        state.mode = Mode::Develop;
    } else if response.clicked() {
        if let Some(star) = star_hover {
            let targets: Vec<_> = if selected {
                state.selected_photos.iter().cloned().collect()
            } else {
                vec![photo.path.clone()]
            };
            let rating = if targets.len() == 1 && photo.annotations.rating == star {
                0
            } else {
                star
            };
            state.library_annotate = Some((crate::state::Annotate::Rating(rating), targets));
        } else {
            let (command, shift) = ui.input(|i| (i.modifiers.command, i.modifiers.shift));
            grid_select(state, &photo.path, command, shift);
        }
    }
    response.on_hover_text({
        let __l10n_args = (&(photo.path.display()),);
        crate::i18n::format(
            "{0}\nDouble-click or Return to open in Develop",
            &[__l10n_args.0.to_string()],
        )
    });
}

fn pill_text(painter: &egui::Painter, right_bottom: egui::Pos2, text: &str) {
    let galley = painter.layout_no_wrap(
        text.to_owned(),
        egui::FontId::proportional(9.0),
        theme::TEXT_PRIMARY,
    );
    let rect =
        egui::Align2::RIGHT_BOTTOM.anchor_size(right_bottom, galley.size() + egui::vec2(8.0, 3.0));
    painter.rect_filled(rect, 3.0, egui::Color32::from_black_alpha(170));
    painter.galley(
        rect.center() - galley.size() / 2.0,
        galley,
        theme::TEXT_PRIMARY,
    );
}

/// Tile for a file whose preview could not be decoded: a painted warning
/// mark (egui's default fonts lack ⚠) plus "No preview" or "File missing"
/// and the file type, instead of a loading shimmer that never ends.
fn paint_no_preview(
    painter: &egui::Painter,
    rect: egui::Rect,
    path: &std::path::Path,
    missing: bool,
    compact: bool,
) {
    let kind = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_uppercase();
    let size = if compact { 9.0 } else { 12.0 };
    let center = rect.center() - egui::vec2(0.0, if compact { 10.0 } else { 14.0 });
    theme::paint_warning_icon(painter, center, size * 1.2);
    let text_y = center.y + size * 0.5 + if compact { 9.0 } else { 12.0 };
    painter.text(
        egui::pos2(rect.center().x, text_y),
        egui::Align2::CENTER_CENTER,
        crate::i18n::text(if missing {
            "File missing"
        } else {
            "No preview"
        }),
        egui::FontId::proportional(if compact { 9.5 } else { 11.0 }),
        theme::TEXT_SECONDARY,
    );
    if !kind.is_empty() {
        painter.text(
            egui::pos2(rect.center().x, text_y + if compact { 11.0 } else { 14.0 }),
            egui::Align2::CENTER_CENTER,
            kind,
            egui::FontId::proportional(if compact { 8.5 } else { 10.0 }),
            theme::TEXT_FAINT,
        );
    }
}

/// File name, plus why its thumbnail failed when it did.
fn thumbnail_hover_text(state: &AppState, path: &std::path::Path) -> String {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_owned();
    match state.thumbnail_failures.get(path) {
        Some(failure) if failure.missing => {
            let reason = crate::i18n::text(
                "The file was moved or deleted outside ShopPhoto. Refresh folders to update the library.",
            );
            format!("{name}\n{reason}")
        }
        Some(failure) => {
            let reason =
                crate::i18n::format("No preview: {0}", std::slice::from_ref(&failure.error));
            format!("{name}\n{reason}")
        }
        None => name,
    }
}

/// Shorten `text` to `max_width` by replacing its middle with "…", keeping
/// the extension visible ("ChatGPT Im…PM (1).png").
pub(super) fn elide_middle(
    ui: &egui::Ui,
    text: &str,
    font: &egui::FontId,
    max_width: f32,
) -> String {
    let width = |s: &str| {
        ui.painter()
            .layout_no_wrap(s.to_owned(), font.clone(), egui::Color32::WHITE)
            .size()
            .x
    };
    if max_width <= 0.0 || width(text) <= max_width {
        return text.to_owned();
    }
    let chars: Vec<char> = text.chars().collect();
    // Keep the extension and a few characters before it on the right.
    let tail_keep = text
        .rfind('.')
        .map(|dot| text[dot..].chars().count() + 3)
        .unwrap_or(4)
        .min(chars.len() / 2);
    let mut head = chars.len().saturating_sub(tail_keep);
    while head > 0 {
        let candidate: String = chars[..head]
            .iter()
            .chain(std::iter::once(&'…'))
            .chain(chars[chars.len() - tail_keep..].iter())
            .collect();
        if width(&candidate) <= max_width {
            return candidate;
        }
        head -= 1;
    }
    let tail: String = chars[chars.len() - tail_keep..].iter().collect();
    format!("…{tail}")
}
