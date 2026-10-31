//! The image viewport: before/after comparison, the floating view toolbar
//! and navigator, tool banners, and the geometry, retouch and crop overlays.

use super::*;
use crate::state::CompareMode;

pub struct ViewportTextures<'a> {
    pub hdr_blit: bool,
    pub developed: Option<(egui::TextureId, egui::Vec2)>,
    pub developed_tile: Option<(egui::TextureId, sp_pipeline::renderer::TileInfo)>,
    /// The comparison render: untouched sensor data (denoise compare) or
    /// the "before" edit (edit compare).
    pub before_denoise: Option<(egui::TextureId, egui::Vec2)>,
    pub before_denoise_tile: Option<(egui::TextureId, sp_pipeline::renderer::TileInfo)>,
    pub preview: Option<&'a egui::TextureHandle>,
    /// Side-by-side culling cells; non-empty replaces the normal view.
    pub culling: &'a [crate::app::CullCell],
}

/// Everything needed to paint one view of the image.
#[derive(Clone, Copy)]
struct Sources {
    hdr_blit: bool,
    proxy: (egui::TextureId, egui::Vec2),
    tile: Option<(egui::TextureId, sp_pipeline::renderer::TileInfo)>,
    compare: Option<(egui::TextureId, egui::Vec2)>,
    compare_tile: Option<(egui::TextureId, sp_pipeline::renderer::TileInfo)>,
}

/// Label for the "before" side of a comparison.
fn before_label(state: &AppState) -> String {
    if state.ai_compare_original || state.ai_compare_split {
        "Before denoise".to_owned()
    } else {
        state
            .compare_before
            .as_ref()
            .map_or_else(|| "Before".to_owned(), |(name, _)| name.clone())
    }
}

/// A small translucent label pill painted over the photo.
fn pill(
    painter: &egui::Painter,
    anchor: egui::Pos2,
    align: egui::Align2,
    text: &str,
    fill: egui::Color32,
    color: egui::Color32,
) {
    let galley =
        painter.layout_no_wrap(text.to_uppercase(), egui::FontId::proportional(10.5), color);
    let size = galley.size() + egui::vec2(16.0, 6.0);
    let rect = align.anchor_size(anchor, size);
    painter.rect_filled(rect, size.y / 2.0, fill);
    painter.galley(rect.center() - galley.size() / 2.0, galley, color);
}

pub fn viewport(ui: &mut egui::Ui, state: &mut AppState, textures: ViewportTextures<'_>) {
    let ViewportTextures {
        hdr_blit,
        developed: gpu_tex,
        developed_tile: tile,
        before_denoise: compare_gpu_tex,
        before_denoise_tile: compare_tile,
        preview,
        culling,
    } = textures;
    if state.culling.is_some() {
        culling_view(ui, state, culling);
        return;
    }
    if state.open_image.is_none() {
        let rect = ui.available_rect_before_wrap();
        state.viewport_size = (rect.width(), rect.height());
        // A failed decode (or deleting the active photo) can leave the
        // previous GPU texture resident. It no longer belongs to a document.
        empty_viewport(ui, state, rect, None);
        return;
    }
    if state.decode_in_flight {
        let rect = ui.available_rect_before_wrap();
        state.viewport_size = (rect.width(), rect.height());
        if preview.is_some() {
            empty_viewport(ui, state, rect, preview);
        } else {
            ui.centered_and_justified(|ui| {
                ui.spinner();
            });
        }
        return;
    }

    let full_rect = ui.available_rect_before_wrap();

    // Fallbacks when no GPU render exists yet.
    let Some((proxy_id, proxy_size)) = gpu_tex else {
        state.viewport_size = (full_rect.width(), full_rect.height());
        empty_viewport(ui, state, full_rect, preview);
        return;
    };
    let sources = Sources {
        hdr_blit,
        proxy: (proxy_id, proxy_size),
        tile,
        compare: compare_gpu_tex,
        compare_tile,
    };
    let ai_compare = state.ai_compare_original || state.ai_compare_split;
    let edit_compare = state.compare_mode != CompareMode::Off && !ai_compare;
    let compare_ready = compare_gpu_tex.is_some();
    let side_by_side = edit_compare
        && state.compare_mode == CompareMode::SideBySide
        && compare_ready
        && !state.crop_mode;
    let gap = 4.0;
    let (before_rect, panel_rect) = if side_by_side {
        let half = (full_rect.width() - gap) / 2.0;
        (
            Some(egui::Rect::from_min_size(
                full_rect.min,
                egui::vec2(half, full_rect.height()),
            )),
            egui::Rect::from_min_size(
                full_rect.min + egui::vec2(half + gap, 0.0),
                egui::vec2(half, full_rect.height()),
            ),
        )
    } else {
        (None, full_rect)
    };
    state.viewport_size = (panel_rect.width(), panel_rect.height());

    let compare_only = state.ai_compare_original && !state.ai_compare_split && compare_ready;
    let Some((aw, ah)) = state.active_size else {
        return;
    };
    let Some((z, fit)) = state.effective_zoom() else {
        return;
    };
    if state.zoom.is_some() {
        state.settle_view_center();
    }

    // Input: drag pans (when zoomed), scroll or pinch zooms at the cursor.
    let resp = ui.allocate_rect(full_rect, egui::Sense::click_and_drag());
    if state.crop_mode {
        crop_overlay(ui, state, full_rect, proxy_id, aw, ah);
        crop_hud(ui.ctx(), state, full_rect, (aw, ah));
        active_tool_banner(ui.ctx(), state, full_rect);
        return;
    }
    if state.crop_saved.is_some() {
        // Crop mode was left from the Geometry card: that is a commit.
        state.crop_saved = None;
    }
    let split_active = compare_ready
        && (state.ai_compare_split || (edit_compare && state.compare_mode == CompareMode::Split));
    let pointer_near_split = split_active
        && resp.interact_pointer_pos().is_some_and(|position| {
            let divider = egui::lerp(
                panel_rect.left()..=panel_rect.right(),
                state.ai_compare_position.clamp(0.05, 0.95),
            );
            (position.x - divider).abs() <= 18.0
        });
    let authoring_active = state.retouch_tool != RetouchTool::None
        || state.mask_point_pick
        || state.wb_picker
        || state.film_base_picker
        || state.sample_picker
        || state.sample_drag.is_some()
        || state.geometry_tool != GeometryTool::None
        || state.color_range_picker.is_some()
        || state.curve_picker
        || state.defringe_picker.is_some();
    if resp.dragged() && state.zoom.is_some() && !pointer_near_split && !authoring_active {
        let d = resp.drag_delta();
        state.view_center.0 -= d.x / (z * aw as f32);
        state.view_center.1 -= d.y / (z * ah as f32);
        state.view_center.0 = state.view_center.0.clamp(0.0, 1.0);
        state.view_center.1 = state.view_center.1.clamp(0.0, 1.0);
    }
    // The half under the pointer anchors zoom in side-by-side mode.
    let anchor_rect = |pos: egui::Pos2| match before_rect {
        Some(left) if left.contains(pos) => left,
        _ => panel_rect,
    };
    if resp.hovered() {
        // Pinch and ⌘/Ctrl+scroll both arrive as egui's zoom_delta; plain
        // two-finger scroll pans the image while zoomed.
        let (zoom_mul, scroll, pointer) = ui
            .ctx()
            .input(|i| (i.zoom_delta(), i.smooth_scroll_delta, i.pointer.hover_pos()));
        if (zoom_mul - 1.0).abs() > 1e-4 {
            let new_z = (z * zoom_mul).clamp(fit * 0.5, 6.0);
            let snapped_to_fit = (new_z - fit).abs() / fit < 0.03 && zoom_mul < 1.0;
            if !snapped_to_fit {
                if let Some(pos) = pointer {
                    let off = pos - anchor_rect(pos).center();
                    state.view_center.0 += off.x * (1.0 / z - 1.0 / new_z) / aw as f32;
                    state.view_center.1 += off.y * (1.0 / z - 1.0 / new_z) / ah as f32;
                    state.view_center.0 = state.view_center.0.clamp(0.0, 1.0);
                    state.view_center.1 = state.view_center.1.clamp(0.0, 1.0);
                }
                state.zoom = Some(new_z);
            } else {
                state.zoom = None;
            }
        } else if scroll.length_sq() > 0.0 && state.zoom.is_some() {
            state.view_center.0 -= scroll.x / (z * aw as f32);
            state.view_center.1 -= scroll.y / (z * ah as f32);
            state.view_center.0 = state.view_center.0.clamp(0.0, 1.0);
            state.view_center.1 = state.view_center.1.clamp(0.0, 1.0);
        }
    }
    if resp.double_clicked() && !authoring_active {
        if state.zoom.is_none() {
            // Fit → 1:1 lands on the double-clicked detail, not the frame
            // center.
            if let Some(pos) = resp.interact_pointer_pos() {
                let off = pos - anchor_rect(pos).center();
                state.view_center.0 = (0.5 + off.x / (z * aw as f32)).clamp(0.0, 1.0);
                state.view_center.1 = (0.5 + off.y / (z * ah as f32)).clamp(0.0, 1.0);
            }
        }
        state.toggle_zoom();
    }

    let painter = ui.painter_at(full_rect);
    let split = split_active.then_some(state.ai_compare_position.clamp(0.05, 0.95));
    let Some((draw_rect, displayed_uv)) = paint_view(
        ui,
        &painter,
        state,
        panel_rect,
        sources,
        compare_only,
        split,
        false,
    ) else {
        return;
    };
    if let Some(left) = before_rect {
        paint_view(ui, &painter, state, left, sources, true, None, true);
        let label = before_label(state);
        pill(
            &painter,
            left.left_top() + egui::vec2(10.0, 10.0),
            egui::Align2::LEFT_TOP,
            &label,
            theme::BG_HUD,
            theme::TEXT_PRIMARY,
        );
        pill(
            &painter,
            panel_rect.left_top() + egui::vec2(10.0, 10.0),
            egui::Align2::LEFT_TOP,
            "After",
            theme::BG_HUD,
            theme::TEXT_PRIMARY,
        );
    } else if edit_compare && !compare_ready {
        pill(
            &painter,
            panel_rect.center_top() + egui::vec2(0.0, 12.0),
            egui::Align2::CENTER_TOP,
            "Preparing comparison…",
            theme::BG_HUD,
            theme::TEXT_SECONDARY,
        );
    }

    geometry_overlay(ui, state, &resp, &painter, draw_rect, displayed_uv);
    retouch_overlay(ui, state, &resp, &painter, draw_rect, displayed_uv);
    sample_overlay(ui, state, &resp, &painter, draw_rect, displayed_uv);

    let coordinates = ViewportCoordinates::new(state, displayed_uv);
    let pick_uv = |pos: egui::Pos2| {
        let uv = coordinates.image_uv([
            (pos.x - draw_rect.left()) / draw_rect.width(),
            (pos.y - draw_rect.top()) / draw_rect.height(),
        ]);
        (uv[0].clamp(0.0, 1.0), uv[1].clamp(0.0, 1.0))
    };
    let picked = resp
        .clicked()
        .then(|| resp.interact_pointer_pos())
        .flatten()
        .filter(|pos| draw_rect.contains(*pos));

    if state.film_base_picker {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
        if let Some(position) = picked {
            state.film_base_sample_request = Some(coordinates.canonical_uv([
                (position.x - draw_rect.left()) / draw_rect.width(),
                (position.y - draw_rect.top()) / draw_rect.height(),
            ]));
            state.film_base_picker = false;
        }
    }
    if state.wb_picker {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
        if let Some(pos) = picked {
            state.wb_sample_request = Some(pick_uv(pos));
            state.wb_picker = false;
        }
    }

    if let Some(index) = state.color_range_picker {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
        if let Some(pos) = picked {
            let (u, v) = pick_uv(pos);
            state.color_range_sample_request = Some((index, u, v));
            state.color_range_picker = None;
        }
    }

    if state.curve_picker {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
        if resp.drag_started() || resp.clicked() {
            if let Some(pos) = resp
                .interact_pointer_pos()
                .filter(|position| draw_rect.contains(*position))
            {
                let (u, v) = pick_uv(pos);
                state.curve_sample_request = Some((state.curve_channel, u, v));
                state.curve_pipette_active = None;
            }
        }
        if resp.dragged() {
            if let Some(active) = state.curve_pipette_active {
                let next_y =
                    (active.initial_y - resp.drag_delta().y / draw_rect.height()).clamp(0.0, 1.0);
                let curve = match active.channel {
                    crate::state::CurveChannel::Luma => &mut state.edit.curve.luma,
                    crate::state::CurveChannel::Red => &mut state.edit.curve.red,
                    crate::state::CurveChannel::Green => &mut state.edit.curve.green,
                    crate::state::CurveChannel::Blue => &mut state.edit.curve.blue,
                };
                if let Some(point) = curve
                    .points
                    .iter_mut()
                    .min_by(|a, b| (a.x - active.x).abs().total_cmp(&(b.x - active.x).abs()))
                {
                    point.y = next_y;
                }
            }
        }
        if resp.drag_stopped() {
            state.curve_pipette_active = None;
        }
    }

    if let Some(target) = state.defringe_picker {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
        if let Some(pos) = picked {
            let (u, v) = pick_uv(pos);
            state.defringe_sample_request = Some((target, u, v));
            state.defringe_picker = None;
        }
    }

    if let Some(split) = split.filter(|_| compare_ready) {
        let divider_x = egui::lerp(draw_rect.left()..=draw_rect.right(), split);
        let divider_response = ui.interact(
            egui::Rect::from_min_max(
                egui::pos2(divider_x - 14.0, draw_rect.top()),
                egui::pos2(divider_x + 14.0, draw_rect.bottom()),
            ),
            ui.id().with("denoise-split-divider"),
            egui::Sense::drag(),
        );
        if divider_response.hovered() || divider_response.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        }
        if divider_response.dragged() {
            if let Some(position) = divider_response.interact_pointer_pos() {
                state.ai_compare_position =
                    ((position.x - draw_rect.left()) / draw_rect.width()).clamp(0.05, 0.95);
                ui.ctx().request_repaint();
            }
        }
        painter.line_segment(
            [
                egui::pos2(divider_x, draw_rect.top()),
                egui::pos2(divider_x, draw_rect.bottom()),
            ],
            egui::Stroke::new(2.0, egui::Color32::WHITE),
        );
        let grip = egui::Rect::from_center_size(
            egui::pos2(divider_x, draw_rect.center().y),
            egui::vec2(26.0, 18.0),
        );
        painter.rect_filled(grip, 9.0, egui::Color32::WHITE);
        for dir in [-1.0f32, 1.0] {
            let c = grip.center() + egui::vec2(dir * 5.0, 0.0);
            painter.add(egui::Shape::convex_polygon(
                vec![
                    c + egui::vec2(dir * 3.0, 0.0),
                    c + egui::vec2(-dir * 1.5, -3.5),
                    c + egui::vec2(-dir * 1.5, 3.5),
                ],
                egui::Color32::from_gray(30),
                egui::Stroke::NONE,
            ));
        }
        let label = before_label(state);
        pill(
            &painter,
            draw_rect.left_top() + egui::vec2(10.0, 10.0),
            egui::Align2::LEFT_TOP,
            &label,
            theme::BG_HUD,
            theme::TEXT_PRIMARY,
        );
        pill(
            &painter,
            draw_rect.right_top() + egui::vec2(-10.0, 10.0),
            egui::Align2::RIGHT_TOP,
            "After",
            theme::BG_HUD,
            theme::TEXT_PRIMARY,
        );
    }

    if state.show_original {
        pill(
            &painter,
            full_rect.center_top() + egui::vec2(0.0, 12.0),
            egui::Align2::CENTER_TOP,
            "Before  ·  \\",
            theme::WARNING,
            theme::ON_ACCENT,
        );
    }

    view_hud(ui.ctx(), state, full_rect, compare_ready, z);
    navigator(ui.ctx(), state, full_rect, proxy_id, (aw, ah));
    active_tool_banner(ui.ctx(), state, full_rect);
}

fn empty_viewport(
    ui: &mut egui::Ui,
    state: &mut AppState,
    panel_rect: egui::Rect,
    preview: Option<&egui::TextureHandle>,
) {
    if let Some(tex) = preview {
        let avail = ui.available_size();
        let tex_size = tex.size_vec2();
        let scale = (avail.x / tex_size.x).min(avail.y / tex_size.y).min(1.0);
        ui.centered_and_justified(|ui| {
            ui.add(egui::Image::new(egui::load::SizedTexture::new(
                tex.id(),
                tex_size * scale,
            )));
        });
        return;
    }
    let card_rect = egui::Rect::from_center_size(panel_rect.center(), egui::vec2(360.0, 190.0));
    let mut card_ui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(card_rect)
            .layout(egui::Layout::top_down(egui::Align::Center)),
    );
    theme::card_frame().show(&mut card_ui, |ui| {
        ui.set_width(336.0);
        ui.vertical_centered(|ui| {
            ui.add_space(theme::SPACE);
            ui.label(
                egui::RichText::new(crate::i18n::text("Open your first photo"))
                    .size(17.0)
                    .strong()
                    .color(theme::TEXT_PRIMARY),
            );
            ui.label(
                egui::RichText::new(supported_formats_line())
                    .size(11.5)
                    .color(theme::TEXT_SECONDARY),
            );
            ui.add_space(theme::SPACE);
            if ui
                .add_sized(
                    [154.0, 32.0],
                    egui::Button::new(
                        egui::RichText::new({
                            let __l10n_args = (&(crate::app::customize::label_for(
                                &state.customize.bindings,
                                "Open photo…",
                                "⌘O",
                            )),);
                            crate::i18n::format(
                                "Open photo…   {0}",
                                std::slice::from_ref(__l10n_args.0),
                            )
                        })
                        .strong()
                        .color(theme::ON_ACCENT),
                    )
                    .fill(theme::ACCENT)
                    .corner_radius(16.0),
                )
                .clicked()
            {
                state.open_dialog_requested = true;
            }
            ui.label(
                egui::RichText::new(crate::i18n::text("or drop a file anywhere in this window"))
                    .size(10.5)
                    .color(theme::TEXT_FAINT),
            );
            ui.add_space(theme::SPACE);
        });
    });
}

#[derive(Clone, Copy)]
struct ViewportCoordinates {
    geometry: Option<sp_core::geometry::GeometryMap>,
    lens: sp_core::geometry::LensMap,
    uv: [f32; 4],
}

impl ViewportCoordinates {
    fn new(state: &AppState, uv: [f32; 4]) -> Self {
        Self {
            geometry: if state.zoom.is_none() && !state.crop_mode {
                state
                    .active_size
                    .map(|(w, h)| sp_core::geometry::GeometryMap::new([w, h], state.edit.geometry))
            } else {
                None
            },
            lens: state.lens_coordinate_map,
            uv,
        }
    }
    fn image_uv(self, display: [f32; 2]) -> [f32; 2] {
        self.geometry.map_or(
            [
                self.uv[0] + display[0] * (self.uv[2] - self.uv[0]),
                self.uv[1] + display[1] * (self.uv[3] - self.uv[1]),
            ],
            |map| map.source_uv(display),
        )
    }
    fn canonical_uv(self, display: [f32; 2]) -> [f32; 2] {
        self.lens.source_uv(self.image_uv(display))
    }
    fn display_uv(self, canonical: [f32; 2]) -> [f32; 2] {
        let image = self.lens.output_uv(canonical);
        self.geometry.map_or_else(
            || {
                [
                    (image[0] - self.uv[0]) / (self.uv[2] - self.uv[0]),
                    (image[1] - self.uv[1]) / (self.uv[3] - self.uv[1]),
                ]
            },
            |map| map.output_uv(image),
        )
    }
}

/// Tessellate the inverse mapping and clip each triangle at source boundaries.
/// This keeps rotated/perspective image corners transparent instead of stretching
/// the sampler's edge pixels across the empty canvas.
fn mapped_texture(
    painter: &egui::Painter,
    rect: egui::Rect,
    texture: egui::TextureId,
    map: impl Fn([f32; 2]) -> [f32; 2],
    color: egui::Color32,
) {
    let mut mesh = egui::Mesh::with_texture(texture);
    let segments = 48;
    for y in 0..segments {
        for x in 0..segments {
            let vertex = |x, y| {
                let display = [x as f32 / segments as f32, y as f32 / segments as f32];
                let uv = map(display);
                egui::epaint::Vertex {
                    pos: rect.min
                        + egui::vec2(display[0] * rect.width(), display[1] * rect.height()),
                    uv: egui::pos2(uv[0], uv[1]),
                    color,
                }
            };
            let corners = [
                vertex(x, y),
                vertex(x + 1, y),
                vertex(x + 1, y + 1),
                vertex(x, y + 1),
            ];
            if corners.iter().all(|vertex| {
                (0.0..=1.0).contains(&vertex.uv.x) && (0.0..=1.0).contains(&vertex.uv.y)
            }) {
                let start = mesh.vertices.len() as u32;
                mesh.vertices.extend(corners);
                mesh.indices
                    .extend([start, start + 1, start + 2, start, start + 2, start + 3]);
                continue;
            }
            for indices in [[0, 1, 2], [0, 2, 3]] {
                let mut polygon = indices.map(|index| corners[index]).to_vec();
                for (axis, bound, above) in [
                    (0, 0.0, true),
                    (0, 1.0, false),
                    (1, 0.0, true),
                    (1, 1.0, false),
                ] {
                    let input = std::mem::take(&mut polygon);
                    let Some(mut previous) = input.last().copied() else {
                        break;
                    };
                    let coord = |v: egui::epaint::Vertex| if axis == 0 { v.uv.x } else { v.uv.y };
                    let inside = |v| {
                        if above {
                            coord(v) >= bound
                        } else {
                            coord(v) <= bound
                        }
                    };
                    for current in input {
                        if inside(previous) != inside(current) {
                            let t = (bound - coord(previous)) / (coord(current) - coord(previous));
                            polygon.push(egui::epaint::Vertex {
                                pos: previous.pos + (current.pos - previous.pos) * t,
                                uv: previous.uv + (current.uv - previous.uv) * t,
                                color,
                            });
                        }
                        if inside(current) {
                            polygon.push(current);
                        }
                        previous = current;
                    }
                }
                if polygon.len() >= 3 {
                    let start = mesh.vertices.len() as u32;
                    for i in 1..polygon.len() - 1 {
                        mesh.indices
                            .extend([start, start + i as u32, start + i as u32 + 1]);
                    }
                    mesh.vertices.extend(polygon);
                }
            }
        }
    }
    painter.add(egui::Shape::mesh(mesh));
}

/// Paint one view of the image into `rect`: the 1:1 tile when it covers
/// the visible region, else the proxy. `compare_only` paints the comparison
/// render instead of the develop output; `split` overlays both with a
/// divider. Returns the drawn rect and the displayed active-image UVs.
#[allow(clippy::too_many_arguments)]
fn paint_view(
    ui: &egui::Ui,
    painter: &egui::Painter,
    state: &AppState,
    rect: egui::Rect,
    sources: Sources,
    compare_only: bool,
    split: Option<f32>,
    second_view: bool,
) -> Option<(egui::Rect, [f32; 4])> {
    let (aw, ah) = state.active_size?;
    let (z, _) = state.effective_zoom()?;
    let (vx, vy, vw, vh) = state.visible_region()?;
    let (proxy_id, proxy_size) = if compare_only {
        sources.compare?
    } else {
        sources.proxy
    };
    let display_tile = if compare_only {
        sources.compare_tile
    } else {
        sources.tile
    };
    let painter = painter.with_clip_rect(rect.intersect(painter.clip_rect()));

    // Screen rect the visible region occupies (centered when smaller).
    let screen_w = vw as f32 * z;
    let screen_h = vh as f32 * z;
    let draw_rect = egui::Rect::from_center_size(
        rect.center(),
        egui::vec2(screen_w.min(rect.width()), screen_h.min(rect.height())),
    );

    // Prefer a full-res tile that contains the current viewport. Tiles carry
    // an overscanned apron, so panning and zooming can reuse them without a
    // distracting one-frame fallback to the lower-resolution proxy.
    let contains = |info: &sp_pipeline::renderer::TileInfo| {
        info.x <= vx && info.y <= vy && info.x + info.w >= vx + vw && info.y + info.h >= vy + vh
    };
    // With a comparison on screen both tiles must describe the same region,
    // otherwise the before/after halves would come from different sources.
    let compare_active = sources.compare.is_some() && (compare_only || split.is_some());
    let tiles_consistent = !compare_active
        || sources
            .tile
            .zip(sources.compare_tile)
            .is_some_and(|((_, a), (_, b))| a == b);
    let usable_tile = display_tile.filter(|_| tiles_consistent);
    let usable_compare_tile = sources
        .compare_tile
        .filter(|_| tiles_consistent && compare_active);
    let paint = TilePaint {
        hdr_blit: sources.hdr_blit,
        draw_rect,
        split,
        compare_only,
        compare: usable_compare_tile,
        second_view,
    };
    if state.zoom.is_none() && !state.crop_mode {
        let map = sp_core::geometry::GeometryMap::new([aw, ah], state.edit.geometry);
        let size = egui::vec2(map.output_size[0] as f32, map.output_size[1] as f32);
        let scale = (rect.width() / size.x).min(rect.height() / size.y).min(1.0);
        let target = egui::Rect::from_center_size(rect.center(), size * scale);
        if sources.hdr_blit {
            painter.add(eframe::egui_wgpu::Callback::new_paint_callback(
                target,
                crate::hdr_view::HdrBlitCallback {
                    source: crate::hdr_view::HdrSource::Proxy,
                    uv_min: [0.0, 0.0],
                    uv_max: [1.0, 1.0],
                    compare_split: split.filter(|_| sources.compare.is_some()),
                    show_compare_only: compare_only,
                    second_view,
                    geometry: Some(map),
                },
            ));
        } else {
            painter.rect_filled(target, 0.0, theme::BG_VIEWPORT);
            mapped_texture(
                &painter,
                target,
                proxy_id,
                |uv| map.source_uv(uv),
                egui::Color32::WHITE,
            );
            if let (Some(split), Some((id, _)), false) = (split, sources.compare, compare_only) {
                let clip = egui::Rect::from_min_max(
                    target.min,
                    egui::pos2(
                        egui::lerp(target.left()..=target.right(), split),
                        target.bottom(),
                    ),
                );
                mapped_texture(
                    &painter.with_clip_rect(clip.intersect(painter.clip_rect())),
                    target,
                    id,
                    |uv| map.source_uv(uv),
                    egui::Color32::WHITE,
                );
            }
        }
        return Some((target, [0.0, 0.0, 1.0, 1.0]));
    }
    let displayed_uv: [f32; 4];
    if let Some((tile_id, info)) = usable_tile.filter(|(_, info)| contains(info)) {
        paint.paint(
            ui,
            &painter,
            draw_rect,
            (tile_id, info),
            (vx, vy, vx + vw, vy + vh),
        );
        displayed_uv = [
            vx as f32 / aw as f32,
            vy as f32 / ah as f32,
            (vx + vw) as f32 / aw as f32,
            (vy + vh) as f32 / ah as f32,
        ];
    } else {
        // Proxy with UV sub-rect = visible region (approximate while the
        // tile renders, exact in fit view). Apply flip; crop applies in
        // fit view only (1:1 inspects the full frame).
        let geom = &state.edit.geometry;
        let (mut u0, mut v0, mut u1, mut v1) = (
            vx as f32 / aw as f32,
            vy as f32 / ah as f32,
            (vx + vw) as f32 / aw as f32,
            (vy + vh) as f32 / ah as f32,
        );
        let crop = geom.crop;
        let cropped = crop != [0.0, 0.0, 1.0, 1.0];
        let mut target = draw_rect;
        if state.zoom.is_none() && cropped {
            let (cw, ch) = (
                (crop[2] - crop[0]) * aw as f32,
                (crop[3] - crop[1]) * ah as f32,
            );
            let fit_c = (rect.width() / cw).min(rect.height() / ch).min(1.0);
            target =
                egui::Rect::from_center_size(rect.center(), egui::vec2(cw * fit_c, ch * fit_c));
            (u0, v0, u1, v1) = (crop[0], crop[1], crop[2], crop[3]);
        }
        if state.zoom.is_none() && geom.flip_horizontal {
            std::mem::swap(&mut u0, &mut u1);
        }
        let uv = egui::Rect::from_min_max(egui::pos2(u0, v0), egui::pos2(u1, v1));
        displayed_uv = [u0, v0, u1, v1];
        if state.zoom.is_none() && geom.rotate_deg != 0.0 {
            // Straighten preview rotates via egui's image path (SDR — the
            // HDR blit callback draws axis-aligned only).
            let angle = geom.rotate_deg.to_radians();
            egui::Image::new(egui::load::SizedTexture::new(proxy_id, target.size()))
                .uv(uv)
                .rotate(angle, egui::vec2(0.5, 0.5))
                .paint_at(ui, target);
        } else if sources.hdr_blit {
            // Metal EDR: draw the scRGB float texture straight into the
            // float swapchain; highlights above SDR white display for real.
            painter.add(eframe::egui_wgpu::Callback::new_paint_callback(
                target,
                crate::hdr_view::HdrBlitCallback {
                    source: crate::hdr_view::HdrSource::Proxy,
                    uv_min: [u0, v0],
                    uv_max: [u1, v1],
                    compare_split: split.filter(|_| sources.compare.is_some()),
                    show_compare_only: compare_only,
                    second_view,
                    geometry: None,
                },
            ));
        } else {
            painter.image(proxy_id, target, uv, egui::Color32::WHITE);
            if let (Some(split), Some((compare_id, _)), false) =
                (split, sources.compare, compare_only)
            {
                let clip = egui::Rect::from_min_max(
                    target.min,
                    egui::pos2(
                        egui::lerp(target.left()..=target.right(), split),
                        target.bottom(),
                    ),
                );
                painter
                    .with_clip_rect(clip.intersect(painter.clip_rect()))
                    .image(compare_id, target, uv, egui::Color32::WHITE);
            }
        }
        if state.zoom.is_some() {
            // Stale tile over the part of the view it still covers.
            if let Some((tile_id, info)) = usable_tile {
                let (x0, y0) = (vx.max(info.x), vy.max(info.y));
                let (x1, y1) = (
                    (vx + vw).min(info.x + info.w),
                    (vy + vh).min(info.y + info.h),
                );
                if x1 > x0 && y1 > y0 {
                    let sx = draw_rect.width() / vw as f32;
                    let sy = draw_rect.height() / vh as f32;
                    let part = egui::Rect::from_min_max(
                        draw_rect.min + egui::vec2((x0 - vx) as f32 * sx, (y0 - vy) as f32 * sy),
                        draw_rect.min + egui::vec2((x1 - vx) as f32 * sx, (y1 - vy) as f32 * sy),
                    );
                    paint.paint(ui, &painter, part, (tile_id, info), (x0, y0, x1, y1));
                }
            }
            // A tile for this region is wanted but not on screen yet: make
            // sure the frame that renders it follows even when no further
            // input arrives (a pinch that ends outside the apron).
            let proxy_density = proxy_size.x / aw as f32;
            if z > proxy_density * 1.2 {
                ui.ctx().request_repaint();
            }
        }
    }
    Some((draw_rect, displayed_uv))
}

// ------------------------------------------------------------ view toolbar

fn hud_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(theme::BG_HUD)
        .stroke(egui::Stroke::new(1.0, egui::Color32::from_white_alpha(18)))
        .corner_radius(10.0)
        .inner_margin(egui::Margin::symmetric(6, 4))
        .shadow(egui::Shadow {
            offset: [0, 4],
            blur: 16,
            spread: 0,
            color: egui::Color32::from_black_alpha(110),
        })
}

fn hud_toggle(ui: &mut egui::Ui, text: &str, on: bool) -> egui::Response {
    ui.add(
        egui::Button::new(egui::RichText::new(text).size(11.5).color(if on {
            theme::TEXT_PRIMARY
        } else {
            theme::TEXT_SECONDARY
        }))
        .fill(if on {
            theme::ACCENT.gamma_multiply(0.3)
        } else {
            egui::Color32::TRANSPARENT
        })
        .stroke(egui::Stroke::new(
            1.0,
            if on {
                theme::ACCENT.gamma_multiply(0.7)
            } else {
                egui::Color32::TRANSPARENT
            },
        ))
        .corner_radius(6.0)
        .min_size(egui::vec2(0.0, 22.0)),
    )
}

/// Clipping toggle painted as a corner triangle (shadows left, highlights
/// right), matching the histogram's indicators.
fn clip_toggle(ui: &mut egui::Ui, highlights: bool, on: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(24.0, 22.0), egui::Sense::click());
    let painter = ui.painter();
    if on {
        painter.rect(
            rect,
            6.0,
            theme::ACCENT.gamma_multiply(0.3),
            egui::Stroke::new(1.0, theme::ACCENT.gamma_multiply(0.7)),
            egui::StrokeKind::Inside,
        );
    } else if resp.hovered() {
        painter.rect_filled(rect, 6.0, theme::BG_HOVER);
    }
    let c = rect.center();
    let s = 5.5;
    let (tri, color) = if highlights {
        (
            vec![
                c + egui::vec2(s, -s),
                c + egui::vec2(s, s),
                c + egui::vec2(-s, -s),
            ],
            theme::CLIP_HIGHLIGHT,
        )
    } else {
        (
            vec![
                c + egui::vec2(-s, -s),
                c + egui::vec2(s, s),
                c + egui::vec2(-s, s),
            ],
            theme::CLIP_SHADOW,
        )
    };
    painter.add(egui::Shape::convex_polygon(
        tri,
        if on || resp.hovered() {
            color
        } else {
            color.gamma_multiply(0.6)
        },
        egui::Stroke::NONE,
    ));
    resp
}

fn hud_separator(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(1.0, 18.0), egui::Sense::hover());
    ui.painter()
        .rect_filled(rect, 0.0, egui::Color32::from_white_alpha(28));
}

/// Floating toolbar at the bottom of the viewport: zoom presets, clipping
/// overlays, before/after modes and the soft-proof state.
fn view_hud(
    ctx: &egui::Context,
    state: &mut AppState,
    rect: egui::Rect,
    compare_ready: bool,
    z: f32,
) {
    let compact = rect.width() < 620.0;
    egui::Area::new(egui::Id::new("viewport-hud"))
        .order(egui::Order::Middle)
        .pivot(egui::Align2::CENTER_BOTTOM)
        .fixed_pos(rect.center_bottom() - egui::vec2(0.0, 10.0))
        .constrain_to(rect)
        .show(ctx, |ui| {
            hud_frame().show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 3.0;
                    let fill_z = state.active_size.map(|(aw, ah)| {
                        (state.viewport_size.0 / aw as f32).max(state.viewport_size.1 / ah as f32)
                    });
                    let presets: &[(&str, Option<f32>)] = if compact {
                        &[("Fit", None), ("100%", Some(1.0))]
                    } else {
                        &[
                            ("Fit", None),
                            ("Fill", fill_z),
                            ("50%", Some(0.5)),
                            ("100%", Some(1.0)),
                            ("200%", Some(2.0)),
                        ]
                    };
                    let mut matched = false;
                    for (label, target) in presets {
                        let on = match (state.zoom, target) {
                            (None, None) if *label == "Fit" => true,
                            (Some(current), Some(target)) => (current - target).abs() < 1e-3,
                            _ => false,
                        };
                        matched |= on;
                        let resp = hud_toggle(ui, label, on);
                        if resp.clicked() {
                            state.zoom = *target;
                            if target.is_none() {
                                state.view_center = (0.5, 0.5);
                            }
                        }
                        if *label == "Fit" {
                            resp.on_hover_text(crate::i18n::text("Fit ↔ 100% · Z"));
                        }
                    }
                    if !matched {
                        ui.label(
                            egui::RichText::new(format!("{:.0}%", z * 100.0))
                                .size(11.5)
                                .color(theme::TEXT_PRIMARY),
                        );
                    }
                    hud_separator(ui);
                    let shadows = clip_toggle(ui, false, state.clipping_mode & 1 != 0);
                    if shadows.clicked() {
                        state.clipping_mode ^= 1;
                    }
                    shadows.on_hover_text(crate::i18n::text("Shadow clipping · J"));
                    let highlights = clip_toggle(ui, true, state.clipping_mode & 2 != 0);
                    if highlights.clicked() {
                        state.clipping_mode ^= 2;
                    }
                    highlights.on_hover_text(crate::i18n::text("Highlight clipping · O"));
                    hud_separator(ui);
                    if hud_toggle(ui, "Before", state.show_original)
                        .on_hover_text(crate::i18n::text("Show the original · \\"))
                        .clicked()
                    {
                        state.show_original = !state.show_original;
                        state.compare_mode = CompareMode::Off;
                    }
                    let split = state.compare_mode == CompareMode::Split;
                    if hud_toggle(ui, "Split", split)
                        .on_hover_text(crate::i18n::text(
                            "Before/after with a draggable divider · Y cycles modes",
                        ))
                        .clicked()
                    {
                        state.compare_mode = if split {
                            CompareMode::Off
                        } else {
                            CompareMode::Split
                        };
                        state.show_original = false;
                    }
                    let sbs = state.compare_mode == CompareMode::SideBySide;
                    if hud_toggle(ui, if compact { "2-up" } else { "Side by side" }, sbs)
                        .on_hover_text(crate::i18n::text(
                            "Before and after next to each other · Y cycles modes",
                        ))
                        .clicked()
                    {
                        state.compare_mode = if sbs {
                            CompareMode::Off
                        } else {
                            CompareMode::SideBySide
                        };
                        state.show_original = false;
                    }
                    if state.compare_mode != CompareMode::Off {
                        if let Some((name, _)) = state.compare_before.clone() {
                            if hud_toggle(ui, &format!("vs {name}  ×"), false)
                                .on_hover_text(crate::i18n::text(
                                    "Compare with the original instead",
                                ))
                                .clicked()
                            {
                                state.compare_before = None;
                            }
                        } else if !compare_ready {
                            ui.spinner();
                        }
                    }
                    if state.proof_space != sp_core::edit::ProofSpace::DisplayNative {
                        hud_separator(ui);
                        let name = match state.proof_space {
                            sp_core::edit::ProofSpace::Srgb => "sRGB".to_owned(),
                            sp_core::edit::ProofSpace::DisplayP3 => "P3".to_owned(),
                            sp_core::edit::ProofSpace::Rec2020 => "Rec.2020".to_owned(),
                            _ => state
                                .selected_custom_proof
                                .clone()
                                .unwrap_or_else(|| "ICC".to_owned()),
                        };
                        if ui
                            .add(
                                egui::Button::new(
                                    egui::RichText::new(crate::i18n::format(
                                        "Proof · {0}",
                                        std::slice::from_ref(&name),
                                    ))
                                    .size(11.0)
                                    .color(theme::WARNING),
                                )
                                .frame(false),
                            )
                            .on_hover_text(crate::i18n::text(
                                "Soft proofing is on · click to turn it off",
                            ))
                            .clicked()
                        {
                            state.proof_space = sp_core::edit::ProofSpace::DisplayNative;
                        }
                    }
                });
            });
        });
}

/// Navigator: a thumbnail of the whole frame with the visible region
/// outlined, shown while zoomed past fit. Drag or click to pan.
fn navigator(
    ctx: &egui::Context,
    state: &mut AppState,
    rect: egui::Rect,
    proxy_id: egui::TextureId,
    (aw, ah): (u32, u32),
) {
    let Some((z, fit)) = state.effective_zoom() else {
        return;
    };
    if state.zoom.is_none() || z <= fit * 1.02 {
        return;
    }
    let Some((vx, vy, vw, vh)) = state.visible_region() else {
        return;
    };
    let width = 150.0f32.min(rect.width() * 0.3);
    let size = egui::vec2(width, width * ah as f32 / aw.max(1) as f32);
    egui::Area::new(egui::Id::new("viewport-navigator"))
        .order(egui::Order::Middle)
        .pivot(egui::Align2::RIGHT_TOP)
        .fixed_pos(rect.right_top() + egui::vec2(-10.0, 10.0))
        .show(ctx, |ui| {
            let (thumb, resp) = ui.allocate_exact_size(size, egui::Sense::click_and_drag());
            let painter = ui.painter();
            painter.rect_filled(thumb.expand(2.0), 6.0, theme::BG_HUD);
            painter.image(
                proxy_id,
                thumb,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::from_gray(210),
            );
            let region = egui::Rect::from_min_max(
                thumb.min
                    + egui::vec2(
                        vx as f32 / aw as f32 * thumb.width(),
                        vy as f32 / ah as f32 * thumb.height(),
                    ),
                thumb.min
                    + egui::vec2(
                        (vx + vw) as f32 / aw as f32 * thumb.width(),
                        (vy + vh) as f32 / ah as f32 * thumb.height(),
                    ),
            );
            let shade = egui::Color32::from_black_alpha(120);
            for r in [
                egui::Rect::from_min_max(thumb.min, egui::pos2(thumb.right(), region.top())),
                egui::Rect::from_min_max(egui::pos2(thumb.left(), region.bottom()), thumb.max),
                egui::Rect::from_min_max(
                    egui::pos2(thumb.left(), region.top()),
                    egui::pos2(region.left(), region.bottom()),
                ),
                egui::Rect::from_min_max(
                    egui::pos2(region.right(), region.top()),
                    egui::pos2(thumb.right(), region.bottom()),
                ),
            ] {
                if r.is_positive() {
                    painter.rect_filled(r, 0.0, shade);
                }
            }
            painter.rect_stroke(
                region,
                2.0,
                egui::Stroke::new(1.5, egui::Color32::WHITE),
                egui::StrokeKind::Middle,
            );
            painter.text(
                thumb.left_bottom() + egui::vec2(4.0, -3.0),
                egui::Align2::LEFT_BOTTOM,
                format!("{:.0}%", z * 100.0),
                egui::FontId::proportional(10.0),
                egui::Color32::WHITE,
            );
            if resp.dragged() || resp.clicked() {
                if let Some(pos) = resp.interact_pointer_pos() {
                    state.view_center = (
                        ((pos.x - thumb.left()) / thumb.width()).clamp(0.0, 1.0),
                        ((pos.y - thumb.top()) / thumb.height()).clamp(0.0, 1.0),
                    );
                    ctx.request_repaint();
                }
            }
            if resp.hovered() {
                ctx.set_cursor_icon(egui::CursorIcon::Grab);
            }
        });
}

// ------------------------------------------------------------ tool banner

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BannerAction {
    None,
    Cancel,
    Done,
}

/// The one instruction banner every canvas tool uses: a translucent pill at
/// the top of the viewport with a title, a hint and clickable Cancel/Done
/// (Esc / Return). Readable on any photo, unlike text painted on the image.
fn tool_banner(
    ctx: &egui::Context,
    rect: egui::Rect,
    title: &str,
    hint: &str,
    cancel: bool,
    done: bool,
) -> BannerAction {
    let mut action = BannerAction::None;
    if !ctx.egui_wants_keyboard_input() {
        ctx.input(|i| {
            if i.key_pressed(egui::Key::Escape) {
                action = if cancel {
                    BannerAction::Cancel
                } else {
                    BannerAction::Done
                };
            } else if done && i.key_pressed(egui::Key::Enter) {
                action = BannerAction::Done;
            }
        });
    }
    egui::Area::new(egui::Id::new("viewport-tool-banner"))
        .order(egui::Order::Middle)
        .pivot(egui::Align2::CENTER_TOP)
        .fixed_pos(rect.center_top() + egui::vec2(0.0, 10.0))
        .constrain_to(rect)
        .show(ctx, |ui| {
            hud_frame()
                .inner_margin(egui::Margin {
                    left: 12,
                    right: 6,
                    top: 5,
                    bottom: 5,
                })
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        let (dot, _) =
                            ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
                        ui.painter().circle_filled(dot.center(), 3.5, theme::ACCENT);
                        ui.label(
                            egui::RichText::new(crate::i18n::text(title))
                                .size(12.0)
                                .strong()
                                .color(theme::TEXT_PRIMARY),
                        );
                        ui.label(
                            egui::RichText::new(crate::i18n::text(hint))
                                .size(12.0)
                                .color(theme::TEXT_SECONDARY),
                        );
                        if cancel
                            && ui
                                .add(
                                    egui::Button::new(
                                        egui::RichText::new(crate::i18n::text("Cancel  Esc"))
                                            .size(11.5),
                                    )
                                    .corner_radius(6.0),
                                )
                                .clicked()
                        {
                            action = BannerAction::Cancel;
                        }
                        if done
                            && ui
                                .add(
                                    egui::Button::new(
                                        egui::RichText::new(crate::i18n::text(if cancel {
                                            "Done  Return"
                                        } else {
                                            "Done  Esc"
                                        }))
                                        .size(11.5)
                                        .strong()
                                        .color(theme::ON_ACCENT),
                                    )
                                    .fill(theme::ACCENT)
                                    .corner_radius(6.0),
                                )
                                .clicked()
                        {
                            action = BannerAction::Done;
                        }
                    });
                });
        });
    action
}

/// Status for the AI selection flow while no canvas tool is active: work in
/// progress, element choice, or what the tinted mask overlay means.
fn selection_banner(ctx: &egui::Context, state: &mut AppState, rect: egui::Rect) {
    enum Button {
        Create(String),
        HideMask,
    }
    let (title, hint, button) = if state.enhancement_busy {
        (
            crate::i18n::text("AI selection").to_owned(),
            crate::i18n::text("Working… the result is highlighted here when ready").to_owned(),
            None,
        )
    } else if state.mask_candidates_current() {
        let Some(candidates) = &state.mask_candidates else {
            return;
        };
        let selected: Vec<_> = candidates
            .items
            .iter()
            .filter(|item| item.selected)
            .map(|item| item.id.to_string())
            .collect();
        let button = (!selected.is_empty()).then(|| {
            (
                if selected.len() == 1 {
                    crate::i18n::text("Create 1 adjustment layer").to_owned()
                } else {
                    crate::i18n::format(
                        "Create {0} adjustment layers",
                        &[selected.len().to_string()],
                    )
                },
                Button::Create(format!(
                    "selected:{}:{}",
                    candidates.kind,
                    selected.join(",")
                )),
            )
        });
        (
            crate::i18n::text("Choose elements").to_owned(),
            crate::i18n::text("Click a numbered element to include or skip it").to_owned(),
            button,
        )
    } else if let Some(layer) = state
        .mask_show_overlay
        .then(|| {
            state
                .retouch_active_local
                .and_then(|index| state.edit.local_adjustments.get(index))
        })
        .flatten()
    {
        (
            crate::i18n::format("Mask: {0}", std::slice::from_ref(&layer.name)),
            crate::i18n::text("Pink area is selected").to_owned(),
            Some((crate::i18n::text("Hide mask").to_owned(), Button::HideMask)),
        )
    } else {
        return;
    };
    let mut clicked = false;
    egui::Area::new(egui::Id::new("viewport-selection-banner"))
        .order(egui::Order::Middle)
        .pivot(egui::Align2::CENTER_TOP)
        .fixed_pos(rect.center_top() + egui::vec2(0.0, 10.0))
        .constrain_to(rect)
        .show(ctx, |ui| {
            hud_frame()
                .inner_margin(egui::Margin {
                    left: 12,
                    right: 6,
                    top: 5,
                    bottom: 5,
                })
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        if state.enhancement_busy {
                            ui.spinner();
                        } else if matches!(button, Some((_, Button::HideMask))) {
                            // Legend swatch: the overlay colour means "selected".
                            let (swatch, _) = ui
                                .allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
                            ui.painter().rect_filled(
                                swatch,
                                3.0,
                                theme::MASK_OVERLAY.gamma_multiply(0.8),
                            );
                        } else {
                            let (dot, _) =
                                ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
                            ui.painter().circle_filled(dot.center(), 3.5, theme::ACCENT);
                        }
                        ui.label(
                            egui::RichText::new(title)
                                .size(12.0)
                                .strong()
                                .color(theme::TEXT_PRIMARY),
                        );
                        ui.label(
                            egui::RichText::new(hint)
                                .size(12.0)
                                .color(theme::TEXT_SECONDARY),
                        );
                        if let Some((label, _)) = &button {
                            clicked = ui
                                .add(
                                    egui::Button::new(
                                        egui::RichText::new(label)
                                            .size(11.5)
                                            .strong()
                                            .color(theme::ON_ACCENT),
                                    )
                                    .fill(theme::ACCENT)
                                    .corner_radius(6.0),
                                )
                                .clicked();
                        }
                    });
                });
        });
    match button {
        Some((_, Button::Create(request))) if clicked => {
            state.enhancement_requested = Some(request);
        }
        Some((_, Button::HideMask)) if clicked => state.mask_show_overlay = false,
        _ => {}
    }
}

/// Tint each detected element in its rail colour and number it, so the
/// viewport shows exactly what each choice covers. Clicking an element
/// toggles whether a layer is created for it.
fn candidate_overlay(
    ui: &egui::Ui,
    state: &mut AppState,
    response: &egui::Response,
    painter: &egui::Painter,
    draw_rect: egui::Rect,
    coordinates: ViewportCoordinates,
) {
    let canonical_at = |position: egui::Pos2| {
        coordinates.canonical_uv([
            (position.x - draw_rect.left()) / draw_rect.width(),
            (position.y - draw_rect.top()) / draw_rect.height(),
        ])
    };
    let to_screen = |uv: [f32; 2]| {
        let uv = coordinates.display_uv(uv);
        draw_rect.min + egui::vec2(uv[0] * draw_rect.width(), uv[1] * draw_rect.height())
    };
    let interactive = !state.mask_point_pick && state.retouch_tool == RetouchTool::None;
    let rail_hover = state.mask_candidate_hover;
    let Some(candidates) = &mut state.mask_candidates else {
        return;
    };
    let pointer_hit = response
        .hover_pos()
        .filter(|position| interactive && draw_rect.contains(*position))
        .and_then(|position| {
            let point = canonical_at(position);
            candidates
                .items
                .iter()
                .enumerate()
                .map(|(index, candidate)| (index, candidate.coverage_at(point)))
                .filter(|(_, coverage)| *coverage > 0.35)
                .max_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(index, _)| index)
        });
    let generation = candidates.generation;
    for (index, candidate) in candidates.items.iter_mut().enumerate() {
        let [x0, y0, x1, y1] = candidate.preview_rect();
        let texture = candidate
            .overlay
            .get_or_insert_with(|| {
                let image = egui::ColorImage::new(
                    candidate.preview.size,
                    candidate
                        .preview
                        .pixels
                        .iter()
                        .map(|pixel| egui::Color32::from_white_alpha(pixel.a()))
                        .collect(),
                );
                ui.ctx().load_texture(
                    format!("element-overlay-{generation}-{}", candidate.id),
                    image,
                    egui::TextureOptions::LINEAR,
                )
            })
            .id();
        let emphasised = pointer_hit == Some(index) || rail_hover == Some(index);
        let color = theme::candidate_color(index);
        let strength = match (candidate.selected, emphasised) {
            (true, true) => 0.75,
            (true, false) => 0.55,
            (false, true) => 0.45,
            (false, false) => 0.22,
        };
        mapped_texture(
            painter,
            draw_rect,
            texture,
            |uv| {
                let point = coordinates.canonical_uv(uv);
                [(point[0] - x0) / (x1 - x0), (point[1] - y0) / (y1 - y0)]
            },
            color.gamma_multiply(strength),
        );
        let [x, y, w, h] = candidate.bounds;
        let corners = [[x, y], [x + w, y], [x + w, y + h], [x, y + h]].map(to_screen);
        painter.add(egui::Shape::closed_line(
            corners.to_vec(),
            egui::Stroke::new(
                if candidate.selected || emphasised {
                    2.0
                } else {
                    1.0
                },
                if candidate.selected || emphasised {
                    color
                } else {
                    color.gamma_multiply(0.6)
                },
            ),
        ));
        let badge = corners[0] + egui::vec2(12.0, 12.0);
        if candidate.selected {
            painter.circle_filled(badge, 10.0, color);
        } else {
            painter.circle_filled(badge, 10.0, theme::BG_HUD);
            painter.circle_stroke(badge, 10.0, egui::Stroke::new(1.5, color));
        }
        painter.text(
            badge,
            egui::Align2::CENTER_CENTER,
            (index + 1).to_string(),
            egui::FontId::proportional(11.5),
            if candidate.selected {
                theme::ON_ACCENT
            } else {
                theme::TEXT_PRIMARY
            },
        );
        if emphasised {
            painter.text(
                badge + egui::vec2(16.0, 0.0),
                egui::Align2::LEFT_CENTER,
                crate::i18n::text(if candidate.selected {
                    "Included · click to skip"
                } else {
                    "Click to include"
                }),
                egui::FontId::proportional(12.0),
                theme::TEXT_PRIMARY,
            );
        }
    }
    if let Some(index) = pointer_hit {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        if response.clicked() {
            if let Some(candidate) = candidates.items.get_mut(index) {
                candidate.selected = !candidate.selected;
            }
        }
    }
}

/// Show the banner for whichever canvas tool is active and apply its
/// Cancel / Done.
fn active_tool_banner(ctx: &egui::Context, state: &mut AppState, rect: egui::Rect) {
    #[derive(Clone, Copy)]
    enum Tool {
        Crop,
        MaskPick,
        WhiteBalance,
        FilmBase,
        SamplePoint,
        ColorRange,
        Curve,
        Defringe,
        Geometry,
        Retouch,
    }
    let tool = if state.crop_mode {
        Tool::Crop
    } else if state.mask_point_pick {
        Tool::MaskPick
    } else if state.sample_picker {
        Tool::SamplePoint
    } else if state.film_base_picker {
        Tool::FilmBase
    } else if state.wb_picker {
        Tool::WhiteBalance
    } else if state.color_range_picker.is_some() {
        Tool::ColorRange
    } else if state.curve_picker {
        Tool::Curve
    } else if state.defringe_picker.is_some() {
        Tool::Defringe
    } else if state.geometry_tool != GeometryTool::None {
        Tool::Geometry
    } else if state.retouch_tool != RetouchTool::None {
        Tool::Retouch
    } else {
        selection_banner(ctx, state, rect);
        return;
    };
    let (title, hint, cancel, done): (String, String, bool, bool) = match tool {
        Tool::Crop => (
            "Crop".into(),
            "Drag corners or edges · X swaps the ratio".into(),
            true,
            true,
        ),
        Tool::MaskPick => {
            let (title, hint) = match state.mask_prompt_pick {
                None => ("Select object", "Click the object you want to select"),
                Some(true) => ("Add keep point", "Click a part that should be selected"),
                Some(false) => ("Add exclude point", "Click an area to leave out"),
            };
            (title.into(), hint.into(), true, false)
        }
        Tool::SamplePoint => (
            "Color sample".into(),
            "Click an area to measure · drag numbered pins to move them".into(),
            true,
            false,
        ),
        Tool::FilmBase => (
            "Film base".into(),
            "Click an evenly lit clear film border".into(),
            true,
            false,
        ),
        Tool::WhiteBalance => (
            "White balance".into(),
            "Click something that should be neutral grey".into(),
            true,
            false,
        ),
        Tool::ColorRange => (
            "Color range".into(),
            "Click the color to include".into(),
            true,
            false,
        ),
        Tool::Curve => (
            "Curve pipette".into(),
            "Click a tone, then drag up or down".into(),
            false,
            true,
        ),
        Tool::Defringe => {
            let target = match state.defringe_picker {
                Some(crate::state::DefringePickerTarget::Green) => "green",
                _ => "purple",
            };
            (
                "Defringe".into(),
                crate::i18n::format(
                    "Click the {0} fringe",
                    &[crate::i18n::text(target).to_string()],
                ),
                true,
                false,
            )
        }
        Tool::Geometry => match state.geometry_tool {
            GeometryTool::DistortionLine => (
                "Distortion calibration".into(),
                "Trace along a bent scene line".into(),
                false,
                true,
            ),
            _ => (
                "Guided upright".into(),
                "Drag along edges that should be straight".into(),
                false,
                true,
            ),
        },
        Tool::Retouch => {
            let name = match state.retouch_tool {
                RetouchTool::Heal => "Heal",
                RetouchTool::Clone => "Clone",
                RetouchTool::Brush => "Brush",
                RetouchTool::Radial => "Radial",
                RetouchTool::LinearGradient => "Gradient",
                RetouchTool::MaskAdd => "Mask +",
                RetouchTool::MaskSubtract => "Mask −",
                RetouchTool::None => "",
            };
            let hint = match state.retouch_tool {
                RetouchTool::Heal | RetouchTool::Clone => {
                    if state.spot_ui.pick_source {
                        "Click the sample source"
                    } else {
                        "Click or paint · Option/Alt-click samples · drag handles to refine"
                    }
                }
                RetouchTool::Radial => "Drag from the center outwards",
                RetouchTool::LinearGradient => "Drag along the gradient axis",
                _ => "Paint in the image",
            };
            (name.into(), hint.into(), false, true)
        }
    };
    match tool_banner(ctx, rect, &title, &hint, cancel, done) {
        BannerAction::None => {}
        action => {
            let cancelled = action == BannerAction::Cancel;
            match tool {
                Tool::Crop => {
                    if cancelled {
                        if let Some(saved) = state.crop_saved {
                            state.edit.geometry.crop = saved;
                        }
                    }
                    state.crop_saved = None;
                    state.crop_mode = false;
                }
                Tool::MaskPick => state.mask_point_pick = false,
                Tool::WhiteBalance => state.wb_picker = false,
                Tool::FilmBase => state.film_base_picker = false,
                Tool::SamplePoint => state.sample_picker = false,
                Tool::ColorRange => state.color_range_picker = None,
                Tool::Curve => {
                    state.curve_picker = false;
                    state.curve_pipette_active = None;
                }
                Tool::Defringe => state.defringe_picker = None,
                Tool::Geometry => {
                    state.geometry_tool = GeometryTool::None;
                    state.geometry_drag_origin = None;
                }
                Tool::Retouch => {
                    state.retouch_tool = RetouchTool::None;
                    state.retouch_active_spot = None;
                    state.retouch_drag_origin = None;
                }
            }
        }
    }
}

// --------------------------------------------------------------- overlays

pub(super) fn geometry_overlay(
    ui: &mut egui::Ui,
    state: &mut AppState,
    response: &egui::Response,
    painter: &egui::Painter,
    draw_rect: egui::Rect,
    displayed_uv: [f32; 4],
) {
    let mut coordinates = ViewportCoordinates::new(state, displayed_uv);
    coordinates.lens = Default::default();
    let to_screen = |uv: [f32; 2]| {
        let uv = coordinates.display_uv(uv);
        draw_rect.min + egui::vec2(uv[0] * draw_rect.width(), uv[1] * draw_rect.height())
    };
    let from_screen = |position: egui::Pos2| {
        coordinates
            .image_uv([
                (position.x - draw_rect.left()) / draw_rect.width(),
                (position.y - draw_rect.top()) / draw_rect.height(),
            ])
            .map(|v| v.clamp(0.0, 1.0))
    };
    for guide in &state.upright_guides {
        let color = match guide.kind {
            sp_core::edit::UprightGuideKind::Vertical => theme::ACCENT,
            sp_core::edit::UprightGuideKind::Horizontal => theme::SUCCESS,
        };
        painter.line_segment(
            [to_screen(guide.start), to_screen(guide.end)],
            egui::Stroke::new(2.0, color),
        );
    }
    for line in &state.distortion_lines {
        for pair in line.points.windows(2) {
            painter.line_segment(
                [to_screen(pair[0]), to_screen(pair[1])],
                egui::Stroke::new(1.5, theme::CLIP_HIGHLIGHT),
            );
        }
    }
    if state.geometry_tool == GeometryTool::None {
        return;
    }
    ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
    let pointer_uv = response
        .interact_pointer_pos()
        .filter(|position| draw_rect.contains(*position))
        .map(from_screen);
    if response.drag_started() {
        state.geometry_drag_origin = pointer_uv;
        if state.geometry_tool == GeometryTool::DistortionLine {
            if let Some(point) = pointer_uv {
                state
                    .distortion_lines
                    .push(sp_core::lens::StraightLineSample {
                        points: vec![point],
                    });
            }
        }
    }
    if response.dragged() && state.geometry_tool == GeometryTool::DistortionLine {
        if let (Some(point), Some(line)) = (pointer_uv, state.distortion_lines.last_mut()) {
            if line.points.last().is_none_or(|previous| {
                (previous[0] - point[0]).hypot(previous[1] - point[1]) > 0.003
            }) {
                line.points.push(point);
            }
        }
    }
    if response.drag_stopped() {
        if let (Some(start), Some(end)) = (state.geometry_drag_origin.take(), pointer_uv) {
            match state.geometry_tool {
                GeometryTool::UprightVertical | GeometryTool::UprightHorizontal => {
                    state.upright_guides.push(sp_core::edit::UprightGuide {
                        start,
                        end,
                        kind: if state.geometry_tool == GeometryTool::UprightVertical {
                            sp_core::edit::UprightGuideKind::Vertical
                        } else {
                            sp_core::edit::UprightGuideKind::Horizontal
                        },
                    });
                }
                GeometryTool::DistortionLine => {
                    if state
                        .distortion_lines
                        .last()
                        .is_some_and(|line| line.points.len() < 3)
                    {
                        state.distortion_lines.pop();
                    }
                }
                GeometryTool::None => {}
            }
        }
    }
}

fn sample_overlay(
    ui: &mut egui::Ui,
    state: &mut AppState,
    response: &egui::Response,
    painter: &egui::Painter,
    draw_rect: egui::Rect,
    displayed_uv: [f32; 4],
) {
    let coordinates = ViewportCoordinates::new(state, displayed_uv);
    let screen = |point: [f32; 2]| {
        let uv = coordinates.display_uv(point);
        draw_rect.min + egui::vec2(uv[0] * draw_rect.width(), uv[1] * draw_rect.height())
    };
    let canonical = |point: egui::Pos2| {
        coordinates
            .canonical_uv([
                (point.x - draw_rect.left()) / draw_rect.width(),
                (point.y - draw_rect.top()) / draw_rect.height(),
            ])
            .map(|v| v.clamp(0.0, 1.0))
    };
    for (index, point) in state.edit.sample_points.iter().enumerate() {
        let point = screen(point.position);
        painter.circle_filled(point, 9.0, theme::BG_HUD);
        painter.circle_stroke(point, 9.0, egui::Stroke::new(1.0, theme::ACCENT));
        painter.text(
            point,
            egui::Align2::CENTER_CENTER,
            format!("{}", index + 1),
            egui::FontId::proportional(10.0),
            theme::TEXT_PRIMARY,
        );
    }
    if state.retouch_tool != RetouchTool::None
        || state.geometry_tool != GeometryTool::None
        || state.wb_picker
        || state.film_base_picker
        || state.mask_point_pick
    {
        return;
    }
    if state.sample_picker {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
    }
    if ui.input(|input| input.pointer.primary_pressed()) {
        if let Some(pointer) = response.interact_pointer_pos() {
            state.sample_drag = state
                .edit
                .sample_points
                .iter()
                .enumerate()
                .rev()
                .find(|(_, point)| screen(point.position).distance(pointer) < 12.0)
                .map(|(index, _)| index);
        }
    }
    if let Some(index) = state.sample_drag {
        if response.dragged() {
            if let (Some(pointer), Some(sample)) = (
                response.interact_pointer_pos(),
                state.edit.sample_points.get_mut(index),
            ) {
                sample.position = canonical(pointer);
            }
        }
        if ui.input(|input| input.pointer.primary_released()) {
            state.sample_drag = None;
        }
        return;
    }
    if state.sample_picker && response.clicked() {
        if let Some(pointer) = response
            .interact_pointer_pos()
            .filter(|p| draw_rect.contains(*p))
        {
            state
                .edit
                .sample_points
                .push(sp_core::analysis::ColorSamplePoint {
                    name: {
                        let __l10n_args = (&(state.edit.sample_points.len() + 1),);
                        crate::i18n::format("Sample {0}", &[__l10n_args.0.to_string()])
                    },
                    position: canonical(pointer),
                });
            state.sample_picker = false;
        }
    }
}

pub(super) fn retouch_overlay(
    ui: &mut egui::Ui,
    state: &mut AppState,
    response: &egui::Response,
    painter: &egui::Painter,
    draw_rect: egui::Rect,
    displayed_uv: [f32; 4],
) {
    use sp_core::edit::LocalMask;
    let coordinates = ViewportCoordinates::new(state, displayed_uv);
    let to_screen = |uv: [f32; 2]| {
        let uv = coordinates.display_uv(uv);
        draw_rect.min + egui::vec2(uv[0] * draw_rect.width(), uv[1] * draw_rect.height())
    };
    let from_screen = |position: egui::Pos2| {
        coordinates
            .canonical_uv([
                (position.x - draw_rect.left()) / draw_rect.width(),
                (position.y - draw_rect.top()) / draw_rect.height(),
            ])
            .map(|v| v.clamp(0.0, 1.0))
    };
    let stroke = egui::Stroke::new(1.25, theme::ACCENT);
    let [width, height] = state
        .active_size
        .map_or([1.0, 1.0], |(w, h)| [w as f32, h as f32]);
    let brush_axes = |radius: f32| {
        [
            radius * width.min(height) / width,
            radius * width.min(height) / height,
        ]
    };
    let ellipse = |center: [f32; 2], radii: [f32; 2], stroke: egui::Stroke| {
        let points = (0..64)
            .map(|step| {
                let angle = step as f32 * std::f32::consts::TAU / 64.0;
                to_screen([
                    center[0] + radii[0] * angle.cos(),
                    center[1] + radii[1] * angle.sin(),
                ])
            })
            .collect();
        painter.add(egui::Shape::closed_line(points, stroke));
    };

    for (index, candidate) in state.spot_ui.dust_candidates.iter().enumerate() {
        let selected = state.spot_ui.dust_selected == Some(index);
        let color = if selected {
            egui::Color32::YELLOW
        } else {
            egui::Color32::from_rgb(255, 170, 50)
        };
        ellipse(
            candidate.position,
            brush_axes(candidate.radius),
            egui::Stroke::new(if selected { 2.5 } else { 1.0 }, color),
        );
        if selected {
            painter.text(
                to_screen(candidate.position),
                egui::Align2::LEFT_BOTTOM,
                {
                    let __l10n_args = (&(index + 1),);
                    crate::i18n::format("  Candidate {0}", &[__l10n_args.0.to_string()])
                },
                egui::FontId::proportional(12.0),
                color,
            );
        }
    }

    if state.retouch_active_local.is_some()
        && !ui.ctx().egui_wants_keyboard_input()
        && !state.customize.open
        && ui.input(|i| {
            crate::app::customize::pressed(
                &state.customize.bindings,
                crate::app::customize::Action::Mask,
                i,
            )
        })
    {
        state.mask_show_overlay = !state.mask_show_overlay;
    }
    let choosing_elements = state.mask_candidates_current();
    if choosing_elements {
        candidate_overlay(ui, state, response, painter, draw_rect, coordinates);
    } else if state.mask_show_overlay {
        if let Some(texture) = state.mask_overlay_texture {
            mapped_texture(
                painter,
                draw_rect,
                texture,
                |uv| coordinates.canonical_uv(uv),
                theme::MASK_OVERLAY.gamma_multiply(0.5),
            );
        }
    }

    for (index, (point, include)) in state.mask_prompts.iter().enumerate() {
        let center = to_screen(*point);
        let color = if *include {
            theme::SUCCESS
        } else {
            theme::ERROR
        };
        // Filled pins with a light ring stay readable on busy photos.
        painter.circle_filled(center, 8.0, color);
        painter.circle_stroke(center, 8.0, egui::Stroke::new(2.0, egui::Color32::WHITE));
        painter.text(
            center,
            egui::Align2::CENTER_CENTER,
            if *include { "+" } else { "−" },
            egui::FontId::proportional(13.0),
            egui::Color32::WHITE,
        );
        let label = center + egui::vec2(11.0, -11.0);
        painter.text(
            label + egui::vec2(1.0, 1.0),
            egui::Align2::LEFT_CENTER,
            (index + 1).to_string(),
            egui::FontId::proportional(11.0),
            egui::Color32::BLACK,
        );
        painter.text(
            label,
            egui::Align2::LEFT_CENTER,
            (index + 1).to_string(),
            egui::FontId::proportional(11.0),
            egui::Color32::WHITE,
        );
    }
    if state.mask_point_pick {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
        if response.clicked() {
            if let Some(position) = response
                .interact_pointer_pos()
                .filter(|p| draw_rect.contains(*p))
            {
                state.mask_point = from_screen(position);
                state.mask_point_pick = false;
                if let Some(include) = state.mask_prompt_pick.take() {
                    if state.mask_prompts.len() < 32 {
                        state.mask_prompts.push((state.mask_point, include));
                    }
                } else {
                    state.mask_prompts.clear();
                    state.enhancement_requested = Some("object".into());
                }
            }
        }
        return;
    }

    for (index, adjustment) in state.edit.local_adjustments.iter().enumerate() {
        if !adjustment.enabled {
            continue;
        }
        let selected = state.retouch_active_local == Some(index);
        let mask_stroke = egui::Stroke::new(
            if selected { 2.0 } else { 1.0 },
            if selected {
                theme::ACCENT
            } else {
                theme::TEXT_FAINT
            },
        );
        match &adjustment.mask {
            LocalMask::Paint { strokes, settings } => {
                if let Some(dab) = strokes.last().and_then(|stroke| stroke.dabs.last()) {
                    let pressure = if settings.pressure {
                        dab.pressure.max(0.05)
                    } else {
                        1.0
                    };
                    ellipse(dab.point, brush_axes(settings.size * pressure), mask_stroke);
                }
            }
            LocalMask::Brush { points, size, .. } => {
                for pair in points.windows(2) {
                    painter.line_segment([to_screen(pair[0]), to_screen(pair[1])], mask_stroke);
                }
                if let Some(point) = points.last() {
                    ellipse(*point, brush_axes(*size), mask_stroke);
                }
            }
            LocalMask::Radial { center, radius, .. } => {
                ellipse(*center, *radius, mask_stroke);
            }
            LocalMask::LinearGradient { start, end } => {
                painter.line_segment([to_screen(*start), to_screen(*end)], mask_stroke);
                painter.circle_filled(to_screen(*start), 3.0, mask_stroke.color);
                painter.circle_filled(to_screen(*end), 3.0, mask_stroke.color);
            }
            LocalMask::Alpha(_) | LocalMask::Composite { .. } | LocalMask::Modified { .. } => {}
            LocalMask::LuminanceRange { .. } => {}
            LocalMask::ColorRange { .. } => {}
        }
    }
    for (index, spot) in state.edit.spots.iter().enumerate() {
        let target = to_screen(spot.target);
        let source = to_screen(spot.source);
        ellipse(
            spot.target,
            brush_axes(spot.radius),
            if state.spot_ui.selected == Some(index) {
                egui::Stroke::new(2.5, theme::ACCENT)
            } else {
                stroke
            },
        );
        painter.circle_filled(target, 3.0, theme::ACCENT);
        painter.circle_filled(source, 3.0, theme::TEXT_FAINT);
        ellipse(
            spot.source,
            brush_axes(spot.radius),
            egui::Stroke::new(1.0, theme::TEXT_FAINT),
        );
        painter.line_segment([target, source], egui::Stroke::new(0.75, theme::TEXT_FAINT));
        for pair in spot.stroke.windows(2) {
            painter.line_segment([to_screen(pair[0]), to_screen(pair[1])], stroke);
        }
    }

    if state.retouch_tool == RetouchTool::None {
        return;
    }
    ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
    let pointer_uv = response
        .interact_pointer_pos()
        .filter(|position| draw_rect.contains(*position))
        .map(from_screen);
    if matches!(state.retouch_tool, RetouchTool::Heal | RetouchTool::Clone) {
        if (state.spot_ui.pick_source || ui.input(|i| i.modifiers.alt)) && response.clicked() {
            if let Some(uv) = pointer_uv {
                state.spot_ui.source = Some(uv);
                state.spot_ui.offset = None;
                state.spot_ui.pick_source = false;
                if let Some(spot) = state
                    .spot_ui
                    .selected
                    .and_then(|i| state.edit.spots.get_mut(i))
                {
                    spot.source = uv;
                }
            }
            return;
        }
        if response.drag_started() || response.clicked() {
            let position = ui
                .input(|i| i.pointer.press_origin())
                .or_else(|| response.interact_pointer_pos());
            if let Some(position) = position {
                let hit = state
                    .edit
                    .spots
                    .iter()
                    .enumerate()
                    .rev()
                    .find_map(|(index, spot)| {
                        if to_screen(spot.source).distance(position) <= 8.0 {
                            Some((index, true))
                        } else if to_screen(spot.target).distance(position) <= 8.0 {
                            Some((index, false))
                        } else {
                            None
                        }
                    });
                if let Some((index, source)) = hit {
                    state.spot_ui.selected = Some(index);
                    if response.drag_started() {
                        state.spot_ui.drag_handle = Some((index, source));
                    }
                    if response.clicked() {
                        return;
                    }
                }
            }
        }
        if let Some((index, source)) = state.spot_ui.drag_handle {
            if let (Some(uv), Some(spot)) = (pointer_uv, state.edit.spots.get_mut(index)) {
                if source {
                    spot.source = uv;
                } else {
                    let delta = [uv[0] - spot.target[0], uv[1] - spot.target[1]];
                    spot.target = uv;
                    for point in &mut spot.stroke {
                        point[0] = (point[0] + delta[0]).clamp(0.0, 1.0);
                        point[1] = (point[1] + delta[1]).clamp(0.0, 1.0);
                    }
                }
            }
            if response.drag_stopped() {
                state.spot_ui.drag_handle = None;
            }
            return;
        }
        if response.hovered()
            && !ui.ctx().egui_wants_keyboard_input()
            && ui.input(|i| i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace))
        {
            if let Some(index) = state
                .spot_ui
                .selected
                .take()
                .filter(|i| *i < state.edit.spots.len())
            {
                state.edit.spots.remove(index);
            }
            return;
        }
        if state.spot_ui.pick_source || ui.input(|i| i.modifiers.alt) {
            return;
        }
    }
    if matches!(
        state.retouch_tool,
        RetouchTool::Brush | RetouchTool::MaskSubtract
    ) {
        if let Some(LocalMask::Paint { strokes, settings }) = state
            .retouch_active_local
            .and_then(|index| state.edit.local_adjustments.get_mut(index))
            .map(|adjustment| &mut adjustment.mask)
        {
            let gesture_id = ui.id().with("local_brush_gesture");
            let previous = ui.data_mut(|data| data.get_temp::<[f32; 3]>(gesture_id));
            let device_pressure = ui.input(|input| {
                input.events.iter().rev().find_map(|event| match event {
                    egui::Event::Touch {
                        force: Some(force), ..
                    } => Some(*force),
                    _ => None,
                })
            });
            let pressure = device_pressure
                .unwrap_or_else(|| previous.map_or(1.0, |sample| sample[2]))
                .clamp(0.0, 1.0);
            if let Some(uv) = pointer_uv {
                let start = response.drag_started() || response.clicked();
                let smoothed = if start {
                    uv
                } else {
                    previous.map_or(uv, |last| {
                        let weight = 1.0 - settings.smoothing;
                        [
                            last[0] + (uv[0] - last[0]) * weight,
                            last[1] + (uv[1] - last[1]) * weight,
                        ]
                    })
                };
                if start {
                    strokes.push(sp_core::mask::PaintStroke::new(
                        uv,
                        device_pressure.unwrap_or(1.0),
                        state.retouch_tool == RetouchTool::MaskSubtract,
                    ));
                } else if response.dragged() || response.drag_stopped() {
                    if let Some(stroke) = strokes.last_mut() {
                        let point = if response.drag_stopped() {
                            uv
                        } else {
                            smoothed
                        };
                        let (width, height) = state.active_size.unwrap_or((1, 1));
                        stroke.extend(
                            point,
                            pressure,
                            *settings,
                            [width, height],
                            response.drag_stopped(),
                        );
                    }
                }
                if start || response.dragged() {
                    ui.data_mut(|data| {
                        data.insert_temp(gesture_id, [smoothed[0], smoothed[1], pressure])
                    });
                }
            }
            if response.drag_stopped() || response.clicked() {
                ui.data_mut(|data| data.remove::<[f32; 3]>(gesture_id));
                if state.spot_ui.refine_after_stroke {
                    state.spot_ui.refine_request = state.retouch_active_local;
                    state.spot_ui.refine_stroke_pending = true;
                }
            }
            return;
        }
    }
    if matches!(
        state.retouch_tool,
        RetouchTool::MaskAdd | RetouchTool::MaskSubtract
    ) {
        if let Some(uv) = pointer_uv {
            let subtract = state.retouch_tool == RetouchTool::MaskSubtract;
            if let Some(LocalMask::Alpha(mask)) = state
                .retouch_active_local
                .and_then(|index| state.edit.local_adjustments.get_mut(index))
                .map(|adjustment| &mut adjustment.mask)
            {
                if response.drag_started() || response.clicked() {
                    mask.repairs.push(sp_core::mask::MaskStroke {
                        points: vec![uv],
                        radius: state.retouch_radius,
                        feather: state.retouch_feather,
                        subtract,
                    });
                } else if response.dragged() {
                    if let Some(stroke) = mask.repairs.last_mut() {
                        if stroke.points.last().is_none_or(|p| {
                            (p[0] - uv[0]).hypot(p[1] - uv[1]) > stroke.radius * 0.15
                        }) {
                            stroke.points.push(uv);
                        }
                    }
                }
            }
        }
        if state.spot_ui.refine_after_stroke && (response.drag_stopped() || response.clicked()) {
            state.spot_ui.refine_request = state.retouch_active_local;
            state.spot_ui.refine_stroke_pending = true;
        }
        return;
    }
    if response.drag_started() {
        state.retouch_drag_origin = pointer_uv;
        if let Some(uv) = pointer_uv {
            match state.retouch_tool {
                RetouchTool::Brush => {
                    if let Some(LocalMask::Brush { points, .. }) = state
                        .retouch_active_local
                        .and_then(|index| state.edit.local_adjustments.get_mut(index))
                        .map(|adjustment| &mut adjustment.mask)
                    {
                        points.push(uv);
                    }
                }
                RetouchTool::Heal | RetouchTool::Clone => {
                    let radius = state.retouch_radius.max(0.002);
                    let correction = state.spot_ui.correction(
                        uv,
                        radius,
                        state.retouch_feather,
                        state.retouch_tool == RetouchTool::Clone,
                        true,
                    );
                    state.edit.spots.push(correction);
                    state.retouch_active_spot = Some(state.edit.spots.len() - 1);
                    state.spot_ui.selected = state.retouch_active_spot;
                }
                RetouchTool::Radial
                | RetouchTool::LinearGradient
                | RetouchTool::None
                | RetouchTool::MaskAdd
                | RetouchTool::MaskSubtract => {}
            }
        }
    }
    if response.dragged() {
        if let Some(uv) = pointer_uv {
            match state.retouch_tool {
                RetouchTool::Brush => {
                    if let Some(LocalMask::Brush { points, .. }) = state
                        .retouch_active_local
                        .and_then(|index| state.edit.local_adjustments.get_mut(index))
                        .map(|adjustment| &mut adjustment.mask)
                    {
                        let minimum_step = state.retouch_radius.max(0.002) * 0.15;
                        let add = points.last().is_none_or(|last| {
                            let dx = last[0] - uv[0];
                            let dy = last[1] - uv[1];
                            (dx * dx + dy * dy).sqrt() > minimum_step
                        });
                        if add {
                            points.push(uv);
                        }
                    }
                }
                RetouchTool::Radial => {
                    if let (Some(origin), Some(LocalMask::Radial { center, radius, .. })) = (
                        state.retouch_drag_origin,
                        state
                            .retouch_active_local
                            .and_then(|index| state.edit.local_adjustments.get_mut(index))
                            .map(|adjustment| &mut adjustment.mask),
                    ) {
                        *center = origin;
                        *radius = [
                            (uv[0] - origin[0]).abs().max(0.002),
                            (uv[1] - origin[1]).abs().max(0.002),
                        ];
                    }
                }
                RetouchTool::LinearGradient => {
                    if let (Some(origin), Some(LocalMask::LinearGradient { start, end })) = (
                        state.retouch_drag_origin,
                        state
                            .retouch_active_local
                            .and_then(|index| state.edit.local_adjustments.get_mut(index))
                            .map(|adjustment| &mut adjustment.mask),
                    ) {
                        *start = origin;
                        *end = uv;
                    }
                }
                RetouchTool::Heal | RetouchTool::Clone => {
                    if let Some(spot) = state
                        .retouch_active_spot
                        .and_then(|index| state.edit.spots.get_mut(index))
                    {
                        let add = spot.stroke.last().is_none_or(|last| {
                            let dx = last[0] - uv[0];
                            let dy = last[1] - uv[1];
                            (dx * dx + dy * dy).sqrt() > spot.radius * 0.15
                        });
                        if add {
                            spot.stroke.push(uv);
                        }
                    }
                }
                RetouchTool::None | RetouchTool::MaskAdd | RetouchTool::MaskSubtract => {}
            }
        }
    }
    if response.drag_stopped() {
        state.retouch_active_spot = None;
        state.retouch_drag_origin = None;
    } else if response.clicked()
        && matches!(state.retouch_tool, RetouchTool::Heal | RetouchTool::Clone)
    {
        if let Some(uv) = pointer_uv {
            let radius = state.retouch_radius.max(0.002);
            let correction = state.spot_ui.correction(
                uv,
                radius,
                state.retouch_feather,
                state.retouch_tool == RetouchTool::Clone,
                false,
            );
            state.edit.spots.push(correction);
            state.spot_ui.selected = Some(state.edit.spots.len() - 1);
        }
    }
}

/// How the viewport paints a 1:1 tile (and its untouched comparison twin)
/// into a screen rect showing the image region `x0..x1 × y0..y1`.
pub(super) struct TilePaint {
    hdr_blit: bool,
    /// Screen rect of the whole visible region; the comparison divider is
    /// positioned relative to it.
    draw_rect: egui::Rect,
    split: Option<f32>,
    compare_only: bool,
    compare: Option<(egui::TextureId, sp_pipeline::renderer::TileInfo)>,
    second_view: bool,
}

impl TilePaint {
    pub(super) fn uv(
        info: &sp_pipeline::renderer::TileInfo,
        region: (u32, u32, u32, u32),
    ) -> egui::Rect {
        let (x0, y0, x1, y1) = region;
        egui::Rect::from_min_max(
            egui::pos2(
                (info.margin_x + x0 - info.x) as f32 / info.tex_w as f32,
                (info.margin_y + y0 - info.y) as f32 / info.tex_h as f32,
            ),
            egui::pos2(
                (info.margin_x + x1 - info.x) as f32 / info.tex_w as f32,
                (info.margin_y + y1 - info.y) as f32 / info.tex_h as f32,
            ),
        )
    }

    /// Returns the texture UVs used for the primary tile.
    fn paint(
        &self,
        ui: &egui::Ui,
        painter: &egui::Painter,
        rect: egui::Rect,
        (tile_id, info): (egui::TextureId, sp_pipeline::renderer::TileInfo),
        region: (u32, u32, u32, u32),
    ) -> egui::Rect {
        let uv = Self::uv(&info, region);
        let divider = self
            .split
            .map(|split| egui::lerp(self.draw_rect.left()..=self.draw_rect.right(), split));
        if self.hdr_blit {
            let _ = ui;
            painter.add(eframe::egui_wgpu::Callback::new_paint_callback(
                rect,
                crate::hdr_view::HdrBlitCallback {
                    source: crate::hdr_view::HdrSource::Tile,
                    uv_min: [uv.min.x, uv.min.y],
                    uv_max: [uv.max.x, uv.max.y],
                    compare_split: divider
                        .filter(|_| self.compare.is_some())
                        .map(|x| (x - rect.left()) / rect.width().max(1.0)),
                    show_compare_only: self.compare_only,
                    second_view: self.second_view,
                    geometry: None,
                },
            ));
        } else {
            painter.image(tile_id, rect, uv, egui::Color32::WHITE);
            if let (Some(divider), Some((compare_id, compare_info)), false) =
                (divider, self.compare, self.compare_only)
            {
                if divider > rect.left() {
                    let clip = egui::Rect::from_min_max(
                        rect.min,
                        egui::pos2(divider.min(rect.right()), rect.bottom()),
                    );
                    painter
                        .with_clip_rect(clip.intersect(painter.clip_rect()))
                        .image(
                            compare_id,
                            rect,
                            Self::uv(&compare_info, region),
                            egui::Color32::WHITE,
                        );
                }
            }
        }
        uv
    }
}

// ------------------------------------------------------------------ crop

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CropHandle {
    Move,
    Corner(bool, bool),
    Left,
    Right,
    Top,
    Bottom,
}

/// Crop tool: full (uncropped) fit view with a draggable crop rectangle,
/// corner and edge handles, a thirds grid while dragging, an optional
/// locked aspect ratio and a live pixel readout. Return applies, Esc reverts,
/// X swaps the locked ratio's orientation.
pub(super) fn crop_overlay(
    ui: &mut egui::Ui,
    state: &mut AppState,
    panel_rect: egui::Rect,
    proxy_id: egui::TextureId,
    aw: u32,
    ah: u32,
) {
    if state.crop_saved.is_none() {
        state.crop_saved = Some(state.edit.geometry.crop);
    }
    let image_aspect = aw as f32 / ah.max(1) as f32;
    if !state.customize.open
        && !ui.ctx().egui_wants_keyboard_input()
        && ui.input(|i| {
            crate::app::customize::pressed(
                &state.customize.bindings,
                crate::app::customize::Action::Reject,
                i,
            )
        })
    {
        if let Some(aspect) = state.crop_aspect.filter(|a| (*a - 1.0).abs() > 1e-3) {
            let swapped = 1.0 / aspect;
            state.crop_aspect = Some(swapped);
            fit_aspect(&mut state.edit.geometry.crop, image_aspect, swapped);
        }
    }
    let painter = ui.painter_at(panel_rect);
    let bottom_room = 64.0;
    let area = egui::Rect::from_min_max(
        panel_rect.min + egui::vec2(0.0, 48.0),
        panel_rect.max - egui::vec2(0.0, bottom_room),
    );
    let fit = (area.width() / aw as f32)
        .min(area.height() / ah as f32)
        .min(1.0);
    let img_rect =
        egui::Rect::from_center_size(area.center(), egui::vec2(aw as f32 * fit, ah as f32 * fit));
    painter.image(
        proxy_id,
        img_rect,
        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
        egui::Color32::WHITE,
    );

    let to_screen = |cx: f32, cy: f32| {
        egui::pos2(
            img_rect.left() + cx * img_rect.width(),
            img_rect.top() + cy * img_rect.height(),
        )
    };
    let crop = state.edit.geometry.crop;
    let crop_rect =
        egui::Rect::from_min_max(to_screen(crop[0], crop[1]), to_screen(crop[2], crop[3]));

    // Interaction.
    let id = ui.id().with("crop-drag");
    let resp = ui.interact(img_rect.expand(12.0), id, egui::Sense::click_and_drag());
    let drag_state_id = ui.id().with("crop-handle");
    let hit = |pos: egui::Pos2| -> Option<CropHandle> {
        let corners = [
            (crop_rect.left_top(), false, false),
            (crop_rect.right_top(), true, false),
            (crop_rect.left_bottom(), false, true),
            (crop_rect.right_bottom(), true, true),
        ];
        if let Some((_, right, bottom)) = corners
            .iter()
            .filter(|(c, _, _)| c.distance(pos) < 16.0)
            .min_by(|a, b| a.0.distance(pos).total_cmp(&b.0.distance(pos)))
        {
            return Some(CropHandle::Corner(*right, *bottom));
        }
        let in_x = pos.x > crop_rect.left() && pos.x < crop_rect.right();
        let in_y = pos.y > crop_rect.top() && pos.y < crop_rect.bottom();
        if in_x && (pos.y - crop_rect.top()).abs() < 10.0 {
            return Some(CropHandle::Top);
        }
        if in_x && (pos.y - crop_rect.bottom()).abs() < 10.0 {
            return Some(CropHandle::Bottom);
        }
        if in_y && (pos.x - crop_rect.left()).abs() < 10.0 {
            return Some(CropHandle::Left);
        }
        if in_y && (pos.x - crop_rect.right()).abs() < 10.0 {
            return Some(CropHandle::Right);
        }
        crop_rect.contains(pos).then_some(CropHandle::Move)
    };
    if let Some(pos) = resp.hover_pos() {
        ui.ctx().set_cursor_icon(match hit(pos) {
            Some(CropHandle::Corner(r, b)) if r == b => egui::CursorIcon::ResizeNwSe,
            Some(CropHandle::Corner(..)) => egui::CursorIcon::ResizeNeSw,
            Some(CropHandle::Left | CropHandle::Right) => egui::CursorIcon::ResizeHorizontal,
            Some(CropHandle::Top | CropHandle::Bottom) => egui::CursorIcon::ResizeVertical,
            Some(CropHandle::Move) => egui::CursorIcon::Move,
            None => egui::CursorIcon::Default,
        });
    }
    if resp.drag_started() {
        if let Some(pos) = resp.interact_pointer_pos() {
            if let Some(handle) = hit(pos) {
                ui.ctx()
                    .data_mut(|d| d.insert_temp(drag_state_id, (handle, crop, pos)));
            }
        }
    }
    let dragging = ui
        .ctx()
        .data(|d| d.get_temp::<(CropHandle, [f32; 4], egui::Pos2)>(drag_state_id));
    if resp.dragged() {
        if let (Some((handle, start, origin)), Some(pos)) = (dragging, resp.interact_pointer_pos())
        {
            let du = (pos.x - origin.x) / img_rect.width();
            let dv = (pos.y - origin.y) / img_rect.height();
            state.edit.geometry.crop =
                drag_crop(start, handle, du, dv, state.crop_aspect, image_aspect);
        }
    }
    if resp.drag_stopped() {
        ui.ctx()
            .data_mut(|d| d.remove::<(CropHandle, [f32; 4], egui::Pos2)>(drag_state_id));
    }

    // Paint shade, frame, grid, handles and readout.
    let crop = state.edit.geometry.crop;
    let crop_rect =
        egui::Rect::from_min_max(to_screen(crop[0], crop[1]), to_screen(crop[2], crop[3]));
    let shade = egui::Color32::from_black_alpha(150);
    for r in [
        egui::Rect::from_min_max(img_rect.min, egui::pos2(img_rect.right(), crop_rect.top())),
        egui::Rect::from_min_max(
            egui::pos2(img_rect.left(), crop_rect.bottom()),
            img_rect.max,
        ),
        egui::Rect::from_min_max(
            egui::pos2(img_rect.left(), crop_rect.top()),
            egui::pos2(crop_rect.left(), crop_rect.bottom()),
        ),
        egui::Rect::from_min_max(
            egui::pos2(crop_rect.right(), crop_rect.top()),
            egui::pos2(img_rect.right(), crop_rect.bottom()),
        ),
    ] {
        if r.is_positive() {
            painter.rect_filled(r, 0.0, shade);
        }
    }
    painter.rect_stroke(
        crop_rect,
        0.0,
        egui::Stroke::new(1.0, egui::Color32::from_white_alpha(220)),
        egui::StrokeKind::Inside,
    );
    if dragging.is_some() {
        let grid = egui::Stroke::new(1.0, egui::Color32::from_white_alpha(120));
        for f in [1.0 / 3.0, 2.0 / 3.0] {
            let x = egui::lerp(crop_rect.x_range(), f);
            let y = egui::lerp(crop_rect.y_range(), f);
            painter.line_segment(
                [
                    egui::pos2(x, crop_rect.top()),
                    egui::pos2(x, crop_rect.bottom()),
                ],
                grid,
            );
            painter.line_segment(
                [
                    egui::pos2(crop_rect.left(), y),
                    egui::pos2(crop_rect.right(), y),
                ],
                grid,
            );
        }
    }
    let handle = egui::Stroke::new(3.0, egui::Color32::WHITE);
    let len = 16.0f32
        .min(crop_rect.width() / 3.0)
        .min(crop_rect.height() / 3.0);
    for (corner, sx, sy) in [
        (crop_rect.left_top(), 1.0, 1.0),
        (crop_rect.right_top(), -1.0, 1.0),
        (crop_rect.left_bottom(), 1.0, -1.0),
        (crop_rect.right_bottom(), -1.0, -1.0),
    ] {
        painter.line_segment([corner, corner + egui::vec2(sx * len, 0.0)], handle);
        painter.line_segment([corner, corner + egui::vec2(0.0, sy * len)], handle);
    }
    for (a, b) in [
        (
            crop_rect.center_top() - egui::vec2(9.0, 0.0),
            crop_rect.center_top() + egui::vec2(9.0, 0.0),
        ),
        (
            crop_rect.center_bottom() - egui::vec2(9.0, 0.0),
            crop_rect.center_bottom() + egui::vec2(9.0, 0.0),
        ),
        (
            crop_rect.left_center() - egui::vec2(0.0, 9.0),
            crop_rect.left_center() + egui::vec2(0.0, 9.0),
        ),
        (
            crop_rect.right_center() - egui::vec2(0.0, 9.0),
            crop_rect.right_center() + egui::vec2(0.0, 9.0),
        ),
    ] {
        painter.line_segment([a, b], handle);
    }
    let (w_px, h_px) = (
        ((crop[2] - crop[0]) * aw as f32).round() as u32,
        ((crop[3] - crop[1]) * ah as f32).round() as u32,
    );
    let readout = format!(
        "{w_px} × {h_px} px · {:.1} MP",
        f64::from(w_px) * f64::from(h_px) / 1.0e6
    );
    let galley = painter.layout_no_wrap(
        readout,
        egui::FontId::proportional(11.0),
        theme::TEXT_PRIMARY,
    );
    let chip = egui::Rect::from_center_size(
        egui::pos2(
            crop_rect.center().x,
            (crop_rect.bottom() + 16.0).min(panel_rect.bottom() - bottom_room + 14.0),
        ),
        galley.size() + egui::vec2(14.0, 6.0),
    );
    painter.rect_filled(chip, 9.0, theme::BG_HUD);
    painter.galley(
        chip.center() - galley.size() / 2.0,
        galley,
        theme::TEXT_PRIMARY,
    );
}

/// New crop after dragging `handle` by (du, dv) from `start`. With a locked
/// `aspect` (pixel width / height) corners keep the opposite corner fixed
/// and edges keep the rectangle centred on the other axis.
fn drag_crop(
    start: [f32; 4],
    handle: CropHandle,
    du: f32,
    dv: f32,
    aspect: Option<f32>,
    image_aspect: f32,
) -> [f32; 4] {
    const MIN: f32 = 0.05;
    let [mut x0, mut y0, mut x1, mut y1] = start;
    match (handle, aspect) {
        (CropHandle::Move, _) => {
            let (w, h) = (x1 - x0, y1 - y0);
            x0 = (x0 + du).clamp(0.0, 1.0 - w);
            y0 = (y0 + dv).clamp(0.0, 1.0 - h);
            [x0, y0, x0 + w, y0 + h]
        }
        (CropHandle::Corner(right, bottom), None) => {
            if right {
                x1 = (x1 + du).clamp(x0 + MIN, 1.0)
            } else {
                x0 = (x0 + du).clamp(0.0, x1 - MIN)
            }
            if bottom {
                y1 = (y1 + dv).clamp(y0 + MIN, 1.0)
            } else {
                y0 = (y0 + dv).clamp(0.0, y1 - MIN)
            }
            [x0, y0, x1, y1]
        }
        (CropHandle::Left, None) => [(x0 + du).clamp(0.0, x1 - MIN), y0, x1, y1],
        (CropHandle::Right, None) => [x0, y0, (x1 + du).clamp(x0 + MIN, 1.0), y1],
        (CropHandle::Top, None) => [x0, (y0 + dv).clamp(0.0, y1 - MIN), x1, y1],
        (CropHandle::Bottom, None) => [x0, y0, x1, (y1 + dv).clamp(y0 + MIN, 1.0)],
        (CropHandle::Corner(right, bottom), Some(r)) => {
            // Normalized height per normalized width at this pixel ratio.
            let k = image_aspect / r;
            let (ax, ay) = (if right { x0 } else { x1 }, if bottom { y0 } else { y1 });
            let moving = if right { x1 + du } else { x0 + du };
            let max_w = if right { 1.0 - ax } else { ax };
            let max_h = if bottom { 1.0 - ay } else { ay };
            let mut w = (moving - ax).abs().max(MIN).min(max_w);
            let mut h = w * k;
            if h > max_h {
                h = max_h;
                w = h / k;
            }
            let (nx0, nx1) = if right { (ax, ax + w) } else { (ax - w, ax) };
            let (ny0, ny1) = if bottom { (ay, ay + h) } else { (ay - h, ay) };
            [nx0, ny0, nx1, ny1]
        }
        (edge, Some(r)) => {
            let k = image_aspect / r;
            let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
            match edge {
                CropHandle::Left | CropHandle::Right => {
                    if edge == CropHandle::Left {
                        x0 = (x0 + du).clamp(0.0, x1 - MIN)
                    } else {
                        x1 = (x1 + du).clamp(x0 + MIN, 1.0)
                    }
                    let h = ((x1 - x0) * k).min(1.0);
                    let w = h / k;
                    if edge == CropHandle::Left {
                        x0 = x1 - w
                    } else {
                        x1 = x0 + w
                    }
                    y0 = (cy - h / 2.0).clamp(0.0, 1.0 - h);
                    [x0, y0, x1, y0 + h]
                }
                _ => {
                    if edge == CropHandle::Top {
                        y0 = (y0 + dv).clamp(0.0, y1 - MIN)
                    } else {
                        y1 = (y1 + dv).clamp(y0 + MIN, 1.0)
                    }
                    let w = ((y1 - y0) / k).min(1.0);
                    let h = w * k;
                    if edge == CropHandle::Top {
                        y0 = y1 - h
                    } else {
                        y1 = y0 + h
                    }
                    x0 = (cx - w / 2.0).clamp(0.0, 1.0 - w);
                    [x0, y0, x0 + w, y1]
                }
            }
        }
    }
}

/// Resize `crop` around its centre to the pixel `aspect`, as large as fits.
fn fit_aspect(crop: &mut [f32; 4], image_aspect: f32, aspect: f32) {
    let k = image_aspect / aspect;
    let (cx, cy) = ((crop[0] + crop[2]) / 2.0, (crop[1] + crop[3]) / 2.0);
    let mut w = crop[2] - crop[0];
    let mut h = w * k;
    if h > 1.0 {
        h = 1.0;
        w = h / k;
    }
    if w > 1.0 {
        w = 1.0;
        h = w * k;
    }
    let x0 = (cx - w / 2.0).clamp(0.0, 1.0 - w);
    let y0 = (cy - h / 2.0).clamp(0.0, 1.0 - h);
    *crop = [x0, y0, x0 + w, y0 + h];
}

/// Aspect-ratio chips shown under the image while cropping.
fn crop_hud(ctx: &egui::Context, state: &mut AppState, rect: egui::Rect, (aw, ah): (u32, u32)) {
    let image_aspect = aw as f32 / ah.max(1) as f32;
    egui::Area::new(egui::Id::new("crop-hud"))
        .order(egui::Order::Middle)
        .pivot(egui::Align2::CENTER_BOTTOM)
        .fixed_pos(rect.center_bottom() - egui::vec2(0.0, 12.0))
        .show(ctx, |ui| {
            hud_frame().show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 3.0;
                    let options: [(&str, Option<f32>); 6] = [
                        ("Free", None),
                        ("Original", Some(image_aspect)),
                        ("1:1", Some(1.0)),
                        ("4:5", Some(4.0 / 5.0)),
                        ("3:2", Some(1.5)),
                        ("16:9", Some(16.0 / 9.0)),
                    ];
                    for (label, aspect) in options {
                        let on = match (state.crop_aspect, aspect) {
                            (None, None) => true,
                            (Some(a), Some(b)) => {
                                (a - b).abs() < 1e-3 || (a - 1.0 / b).abs() < 1e-3
                            }
                            _ => false,
                        };
                        if hud_toggle(ui, label, on).clicked() {
                            state.crop_aspect = aspect;
                            if let Some(aspect) = aspect {
                                fit_aspect(&mut state.edit.geometry.crop, image_aspect, aspect);
                            }
                        }
                    }
                    hud_separator(ui);
                    let can_swap = state.crop_aspect.is_some_and(|a| (a - 1.0).abs() > 1e-3);
                    if ui
                        .add_enabled(
                            can_swap,
                            egui::Button::new(
                                egui::RichText::new(crate::i18n::text("Swap")).size(11.5),
                            )
                            .corner_radius(6.0),
                        )
                        .on_hover_text(crate::i18n::text("Swap portrait / landscape · X"))
                        .clicked()
                    {
                        if let Some(aspect) = state.crop_aspect {
                            state.crop_aspect = Some(1.0 / aspect);
                            fit_aspect(&mut state.edit.geometry.crop, image_aspect, 1.0 / aspect);
                        }
                    }
                    if ui
                        .add(
                            egui::Button::new(
                                egui::RichText::new(crate::i18n::text("Reset")).size(11.5),
                            )
                            .corner_radius(6.0),
                        )
                        .clicked()
                    {
                        state.edit.geometry.crop = [0.0, 0.0, 1.0, 1.0];
                        state.crop_aspect = None;
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    let mut chosen_model = None;
                    let mut open = ui.button(crate::i18n::text("AI upscale crop…")).clicked();
                    for scale in [2_u32, 4] {
                        let model = super::dialogs::default_upscale_model(scale);
                        let dimensions = super::dialogs::export_geometry_dims(state)
                            .and_then(|(w, h)| Some((w.checked_mul(scale)?, h.checked_mul(scale)?)));
                        let fits = dimensions.is_some_and(|(w, h)| u64::from(w) * u64::from(h) <= 150_000_000);
                        let label = dimensions.map_or_else(|| format!("{scale}×"), |(w, h)| format!("{scale}× · {w} × {h}"));
                        let response = ui.add_enabled(model.is_some() && fits, egui::Button::new(label));
                        let response = if !fits {
                            response.on_disabled_hover_text(crate::i18n::text("This crop exceeds the 150 MP AI limit. Crop more tightly or choose 2×."))
                        } else if model.is_none() {
                            response.on_disabled_hover_text(crate::i18n::text("Install the official SwinIR checkpoints in Tools → AI runtime."))
                        } else {
                            let (w, h) = dimensions.unwrap();
                            response.on_hover_text(format!("{} · {:.1} MP", crate::i18n::text(if scale == 2 { "Faithful 2×" } else { "Faithful 4×" }), f64::from(w) * f64::from(h) / 1e6))
                        };
                        if response.clicked() {
                            chosen_model = model;
                            open = true;
                        }
                    }
                    if open {
                        state.crop_mode = false;
                        state.crop_saved = None;
                        state.export_settings.size = sp_export::SizeMode::Original;
                        state.export_defaults_applied = true;
                        if let Some(model) = chosen_model {
                            state.export_settings.upscale_model = Some(model);
                        } else if state.export_settings.upscale_model.is_none() {
                            state.export_settings.upscale_model = super::dialogs::default_upscale_model(2);
                        }
                        state.export_dialog_open = true;
                    }
                });
            });
        });
}

// --------------------------------------------------------------- culling

/// Two-up or four-up culling grid. Pan and zoom are shared across cells
/// when locked; clicks focus a cell, a double-click opens it in Develop.
fn culling_view(ui: &mut egui::Ui, state: &mut AppState, cells: &[crate::app::CullCell]) {
    let panel = ui.available_rect_before_wrap();
    state.viewport_size = (panel.width(), panel.height());
    let Some(mut culling) = state.culling.clone() else {
        return;
    };
    let count = cells.len().max(1);
    let (cols, rows) = if count <= 2 { (count, 1) } else { (2, 2) };
    let gap = 4.0;
    let cell_w = ((panel.width() - gap * (cols as f32 - 1.0)) / cols as f32).max(1.0);
    let cell_h = ((panel.height() - gap * (rows as f32 - 1.0)) / rows as f32).max(1.0);
    let mut regions = vec![None; cells.len()];
    let mut clicked = None;
    let mut open = None;
    let mut drag = egui::Vec2::ZERO;
    let mut dragged_cell = None;
    let mut hovered = None;
    let mut focused_scale = None;
    for (index, cell) in cells.iter().enumerate() {
        let col = (index % cols) as f32;
        let row = (index / cols) as f32;
        let rect = egui::Rect::from_min_size(
            panel.min + egui::vec2(col * (cell_w + gap), row * (cell_h + gap)),
            egui::vec2(cell_w, cell_h),
        );
        let resp = ui.allocate_rect(rect, egui::Sense::click_and_drag());
        if resp.clicked() {
            clicked = Some(index);
        }
        if resp.double_clicked() {
            open = Some(index);
        }
        if resp.dragged() {
            drag += resp.drag_delta();
            dragged_cell = Some(index);
        }
        if resp.hovered() {
            hovered = Some(index);
        }
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, theme::BG_VIEWPORT);
        let focused = index == culling.focused;
        let Some((aw, ah)) = cell.active else {
            if let Some(error) = &cell.error {
                painter.text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    format!("{}\n{error}", cell.name),
                    egui::FontId::proportional(12.0),
                    theme::TEXT_SECONDARY,
                );
            } else {
                ui.put(
                    egui::Rect::from_center_size(rect.center(), egui::vec2(24.0, 24.0)),
                    egui::Spinner::new(),
                );
            }
            cell_frame(&painter, rect, focused, &cell.name, cell.annotations);
            continue;
        };
        let fit = (cell_w / aw as f32).min(cell_h / ah as f32).min(1.0);
        let scale = state.zoom.map_or(fit, |z| z.clamp(fit * 0.5, 6.0));
        if focused {
            focused_scale = Some((scale, aw, ah));
        }
        let center = if culling.locked {
            state.view_center
        } else {
            culling.centers.get(index).copied().unwrap_or((0.5, 0.5))
        };
        let w = ((cell_w / scale).round() as u32).clamp(1, aw);
        let h = ((cell_h / scale).round() as u32).clamp(1, ah);
        let cx = (center.0 * aw as f32).round() as i64;
        let cy = (center.1 * ah as f32).round() as i64;
        let x = (cx - i64::from(w) / 2).clamp(0, i64::from(aw - w)) as u32;
        let y = (cy - i64::from(h) / 2).clamp(0, i64::from(ah - h)) as u32;
        regions[index] = Some((x, y, w, h));
        let draw = egui::Rect::from_center_size(
            rect.center(),
            egui::vec2(w as f32 * scale, h as f32 * scale),
        );
        let uv = egui::Rect::from_min_max(
            egui::pos2(x as f32 / aw as f32, y as f32 / ah as f32),
            egui::pos2((x + w) as f32 / aw as f32, (y + h) as f32 / ah as f32),
        );
        let tile_covers = |info: &sp_pipeline::renderer::TileInfo| {
            info.x <= x && info.y <= y && info.x + info.w >= x + w && info.y + info.h >= y + h
        };
        if let Some((tile_id, info)) = cell.tile.filter(|(_, info)| tile_covers(info)) {
            let uv = TilePaint::uv(&info, (x, y, x + w, y + h));
            if cell.hdr_tile {
                culling_hdr_blit(
                    &painter,
                    draw,
                    uv,
                    crate::hdr_view::HdrSource::CullTile(index as u8),
                );
            } else {
                painter.image(tile_id, draw, uv, egui::Color32::WHITE);
            }
        } else if let Some((id, _)) = cell.proxy {
            if cell.hdr_proxy {
                culling_hdr_blit(
                    &painter,
                    draw,
                    uv,
                    crate::hdr_view::HdrSource::CullProxy(index as u8),
                );
            } else {
                painter.image(id, draw, uv, egui::Color32::WHITE);
            }
        } else if let Some((id, _)) = cell.preview {
            painter.image(id, draw, uv, egui::Color32::WHITE);
        }
        cell_frame(&painter, rect, focused, &cell.name, cell.annotations);
        if let Some(quality) = cell.quality {
            let similarity = cells
                .first()
                .and_then(|reference| reference.quality)
                .map_or_else(String::new, |reference| {
                    let __l10n_args = (&(quality.similarity(reference) * 100.0),);
                    crate::i18n::format(" · similar {0}%", &[format!("{:.0}", __l10n_args.0)])
                });
            painter.text(
                rect.left_top() + egui::vec2(10.0, 34.0),
                egui::Align2::LEFT_TOP,
                {
                    let __l10n_args = (&(quality.detail), &(quality.highlights_percent));
                    crate::i18n::format(
                        "Preview detail {0}{1} · whites {2}%",
                        &[
                            format!("{:.0}", __l10n_args.0),
                            similarity.to_string(),
                            format!("{:.1}", __l10n_args.1),
                        ],
                    )
                },
                egui::FontId::proportional(10.0),
                theme::TEXT_SECONDARY,
            );
            if let Some(faces) = quality.faces {
                painter.text(
                    rect.left_top() + egui::vec2(10.0, 48.0),
                    egui::Align2::LEFT_TOP,
                    {
                        let __l10n_args = (
                            &(faces.faces),
                            &(faces.eye_regions),
                            &(faces
                                .weakest_eye_detail
                                .map_or_else(|| "?".into(), |v| format!("{v:.0}"))),
                        );
                        crate::i18n::format(
                            "{0} faces · {1} eye regions · weakest detail {2}",
                            &[
                                __l10n_args.0.to_string(),
                                __l10n_args.1.to_string(),
                                __l10n_args.2.to_string(),
                            ],
                        )
                    },
                    egui::FontId::proportional(10.0),
                    theme::TEXT_SECONDARY,
                );
            }
        }
    }

    // Input: click focuses, drag pans, scroll or pinch zooms.
    if let Some(index) = clicked.or(dragged_cell) {
        culling.focused = index;
    }
    if let Some(index) = open {
        state.culling_open_requested = culling.paths.get(index).cloned();
    }
    let (zoom_delta, scroll) = ui.input(|i| (i.zoom_delta(), i.smooth_scroll_delta.y));
    let wheel = if hovered.is_some() && scroll.abs() > 0.0 {
        (1.0 + scroll * 0.002).clamp(0.5, 2.0)
    } else {
        1.0
    };
    let factor = zoom_delta * wheel;
    if let Some((scale, aw, ah)) = focused_scale {
        if (factor - 1.0).abs() > 1e-4 {
            state.zoom = Some((scale * factor).clamp(0.02, 6.0));
        }
        if drag != egui::Vec2::ZERO {
            let dx = drag.x / scale / aw as f32;
            let dy = drag.y / scale / ah as f32;
            if culling.locked {
                state.view_center.0 = (state.view_center.0 - dx).clamp(0.0, 1.0);
                state.view_center.1 = (state.view_center.1 - dy).clamp(0.0, 1.0);
            } else if let Some(center) = culling.centers.get_mut(culling.focused) {
                center.0 = (center.0 - dx).clamp(0.0, 1.0);
                center.1 = (center.1 - dy).clamp(0.0, 1.0);
            }
        }
    }
    culling.regions = regions;
    state.culling = Some(culling);
}

fn culling_hdr_blit(
    painter: &egui::Painter,
    draw: egui::Rect,
    uv: egui::Rect,
    source: crate::hdr_view::HdrSource,
) {
    painter.add(eframe::egui_wgpu::Callback::new_paint_callback(
        draw,
        crate::hdr_view::HdrBlitCallback {
            source,
            uv_min: [uv.min.x, uv.min.y],
            uv_max: [uv.max.x, uv.max.y],
            compare_split: None,
            show_compare_only: false,
            second_view: false,
            geometry: None,
        },
    ));
}

fn cell_frame(
    painter: &egui::Painter,
    rect: egui::Rect,
    focused: bool,
    name: &str,
    annotations: sp_catalog::Annotations,
) {
    let stroke = if focused {
        egui::Stroke::new(2.0, theme::ACCENT)
    } else {
        egui::Stroke::new(1.0, theme::HAIRLINE)
    };
    painter.rect_stroke(rect.shrink(1.0), 0.0, stroke, egui::StrokeKind::Inside);
    let flag = match annotations.flag {
        sp_catalog::Flag::Pick => "  · pick",
        sp_catalog::Flag::Reject => "  · reject",
        sp_catalog::Flag::None => "",
    };
    let stars = "★".repeat(usize::from(annotations.rating.min(5)));
    let label = format!("{name}{flag}  {stars}");
    let font = egui::FontId::proportional(12.0);
    let galley = painter.layout_no_wrap(label, font, theme::TEXT_PRIMARY);
    let pad = egui::vec2(8.0, 4.0);
    let box_rect =
        egui::Rect::from_min_size(rect.min + egui::vec2(8.0, 8.0), galley.size() + pad * 2.0);
    painter.rect_filled(box_rect, 4.0, egui::Color32::from_black_alpha(170));
    painter.galley(box_rect.min + pad, galley, theme::TEXT_PRIMARY);
}

/// One line naming what the open dialog accepts, derived from the decoder
/// lists so it cannot drift: RAW families first, then rendered formats.
fn supported_formats_line() -> String {
    let raster = sp_core::source::SUPPORTED_RASTER_EXTENSIONS;
    let mut parts = vec![crate::i18n::text("Camera RAW").to_owned(), "DNG".to_owned()];
    for (ext, label) in [
        ("jpg", "JPEG"),
        ("tif", "TIFF"),
        ("png", "PNG"),
        ("heic", "HEIC"),
        ("avif", "AVIF"),
        ("jxl", "JXL"),
    ] {
        if raster.contains(&ext) {
            parts.push(label.to_owned());
        }
    }
    parts.join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_closed_or_failed_photo_does_not_paint_its_resident_gpu_texture() {
        let ctx = egui::Context::default();
        let stale = egui::TextureId::User(42);
        let mut state = AppState {
            active_size: Some((1000, 800)),
            ..Default::default()
        };
        let output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(800.0, 600.0),
                )),
                ..Default::default()
            },
            |ui| {
                viewport(
                    ui,
                    &mut state,
                    ViewportTextures {
                        hdr_blit: false,
                        developed: Some((stale, egui::vec2(1000.0, 800.0))),
                        developed_tile: None,
                        before_denoise: None,
                        before_denoise_tile: None,
                        preview: None,
                        culling: &[],
                    },
                )
            },
        );
        for primitive in ctx.tessellate(output.shapes, output.pixels_per_point) {
            if let egui::epaint::Primitive::Mesh(mesh) = primitive.primitive {
                assert_ne!(mesh.texture_id, stale);
            }
        }
    }

    fn pixel_aspect(crop: [f32; 4], image_aspect: f32) -> f32 {
        (crop[2] - crop[0]) / (crop[3] - crop[1]) * image_aspect
    }

    #[test]
    fn mask_coordinates_survive_fitted_geometry_lens_changes_and_zoom() {
        let mut state = AppState {
            active_size: Some((6016, 4016)),
            ..Default::default()
        };
        state.edit.geometry = sp_core::edit::GeometryParams {
            crop: [0.12, 0.1, 0.88, 0.9],
            rotate_deg: 13.0,
            quarter_turns: 1,
            flip_vertical: true,
            perspective_horizontal: 24.0,
            scale: 120.0,
            ..Default::default()
        };
        let point = [0.37, 0.61];
        for k1 in [0.0, -0.08, 0.1] {
            state.lens_coordinate_map = sp_core::geometry::LensMap {
                distortion: [k1, 0.01, 0.0],
                enabled: true,
                crop_to_valid: true,
                ..Default::default()
            };
            for zoom in [None, Some(1.0)] {
                state.zoom = zoom;
                let coordinates = ViewportCoordinates::new(&state, [0.25, 0.2, 0.75, 0.7]);
                let screen = coordinates.display_uv(point);
                let authored = coordinates.canonical_uv(screen);
                assert!(
                    (authored[0] - point[0]).abs() < 3e-5 && (authored[1] - point[1]).abs() < 3e-5,
                    "{screen:?} {authored:?}"
                );
            }
        }
    }

    #[test]
    fn locked_corner_drag_keeps_the_ratio_and_the_opposite_corner() {
        let start = [0.1, 0.1, 0.9, 0.9];
        let out = drag_crop(
            start,
            CropHandle::Corner(true, true),
            -0.2,
            0.05,
            Some(1.5),
            1.35,
        );
        assert!((pixel_aspect(out, 1.35) - 1.5).abs() < 1e-3, "{out:?}");
        assert!((out[0] - 0.1).abs() < 1e-6 && (out[1] - 0.1).abs() < 1e-6);
    }

    #[test]
    fn locked_edge_drag_keeps_the_ratio_inside_the_frame() {
        let start = [0.2, 0.2, 0.8, 0.8];
        let out = drag_crop(start, CropHandle::Right, 0.5, 0.0, Some(1.0), 1.5);
        assert!((pixel_aspect(out, 1.5) - 1.0).abs() < 1e-3, "{out:?}");
        assert!(out.iter().all(|v| (0.0..=1.0).contains(v)));
    }

    #[test]
    fn free_move_stays_inside_the_image() {
        let out = drag_crop([0.5, 0.5, 0.9, 0.9], CropHandle::Move, 0.5, 0.5, None, 1.0);
        assert!((out[2] - 1.0).abs() < 1e-6 && (out[3] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn fitting_an_aspect_uses_the_largest_centred_rect() {
        let mut crop = [0.0, 0.0, 1.0, 1.0];
        fit_aspect(&mut crop, 1.5, 1.0);
        assert!((pixel_aspect(crop, 1.5) - 1.0).abs() < 1e-3);
        assert!((crop[3] - crop[1] - 1.0).abs() < 1e-6);
    }
}
