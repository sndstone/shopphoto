//! The develop inspector: tone, curves, colour, detail, lens, retouching
//! and metadata panels, with their shared slider and card widgets.

use super::*;
use crate::edit_groups::{self, Group};

/// Enabling HDR is an HDR-output intent. Keep the separate target control in
/// sync on that transition, while still allowing the photographer to choose
/// an SDR target explicitly after HDR has been enabled.
pub(super) fn select_hdr_target_when_enabled(edit: &mut sp_core::EditState, was_enabled: bool) {
    if !was_enabled && edit.hdr.enabled {
        edit.hdr.target = sp_core::edit::HdrTarget::Hdr;
    }
}

// ------------------------------------------------------------- slider units

/// Display unit and precision of a develop slider's number field.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub(super) enum Unit {
    /// Derived from the range: signed when it spans zero, decimals from
    /// the span.
    #[default]
    Auto,
    Plain(usize),
    Signed(usize),
    Ev,
    Kelvin,
    Percent,
    Px,
    Deg,
}

impl Unit {
    fn resolve(self, range: &std::ops::RangeInclusive<f32>) -> Unit {
        if self != Unit::Auto {
            return self;
        }
        let span = (range.end() - range.start()).abs().max(1e-6);
        let decimals = (2.0 - span.log10().floor()).clamp(0.0, 4.0) as usize;
        if *range.start() < 0.0 && *range.end() > 0.0 {
            Unit::Signed(decimals)
        } else {
            Unit::Plain(decimals)
        }
    }

    fn decimals(self) -> usize {
        match self {
            Unit::Plain(d) | Unit::Signed(d) => d,
            Unit::Ev => 2,
            Unit::Px => 1,
            Unit::Auto | Unit::Kelvin | Unit::Percent | Unit::Deg => 0,
        }
    }

    pub(super) fn format(self, value: f64) -> String {
        let signed = |v: f64, d: usize| {
            let text = format!("{:.*}", d, v.abs());
            if text.trim_start_matches(['0', '.']).is_empty() {
                format!("{:.*}", d, 0.0)
            } else if v < 0.0 {
                format!("−{text}")
            } else {
                format!("+{text}")
            }
        };
        match self {
            Unit::Auto => format!("{value:.1}"),
            Unit::Plain(d) => format!("{value:.d$}"),
            Unit::Signed(d) => signed(value, d),
            Unit::Ev => format!("{} EV", signed(value, 2)),
            Unit::Kelvin => format!("{value:.0} K"),
            Unit::Percent => format!("{value:.0}%"),
            Unit::Px => format!("{value:.1} px"),
            Unit::Deg => format!("{value:.0}°"),
        }
    }

    fn parse(text: &str) -> Option<f64> {
        let cleaned: String = text
            .replace('−', "-")
            .chars()
            .filter(|c| c.is_ascii_digit() || matches!(c, '.' | '-' | '+'))
            .collect();
        cleaned.parse().ok()
    }
}

/// How a develop slider maps and paints its value.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct SliderSpec {
    pub unit: Unit,
    /// Logarithmic position (temperature): equal steps feel equal.
    pub log: bool,
    /// Colored track (start, middle, end) instead of the accent fill.
    pub gradient: Option<[egui::Color32; 3]>,
}

impl SliderSpec {
    pub(super) fn unit(unit: Unit) -> Self {
        Self {
            unit,
            ..Self::default()
        }
    }
}

/// Maps a value to a 0..1 track position and back.
#[derive(Clone, Copy)]
struct Scale {
    lo: f32,
    hi: f32,
    log: bool,
}

impl Scale {
    fn new(range: &std::ops::RangeInclusive<f32>, log: bool) -> Self {
        let (lo, hi) = (*range.start(), *range.end());
        Self {
            lo,
            hi,
            log: log && lo > 0.0 && hi > lo,
        }
    }

    fn to_t(self, value: f32) -> f32 {
        let t = if self.log {
            (value.max(self.lo) / self.lo).ln() / (self.hi / self.lo).ln()
        } else {
            (value - self.lo) / (self.hi - self.lo).max(f32::EPSILON)
        };
        t.clamp(0.0, 1.0)
    }

    fn value_at(self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        if self.log {
            self.lo * (self.hi / self.lo).powf(t)
        } else {
            self.lo + t * (self.hi - self.lo)
        }
    }
}

/// Put a `DragValue` in a hard layout slot. `Ui::add_sized` only suggests a
/// maximum size and `DragValue` deliberately renders its text with
/// `TextWrapMode::Extend`, so values such as `-117.0` can otherwise widen the
/// entire inspector and move the viewport boundary while dragging.
#[cfg_attr(not(test), allow(dead_code))] // exercised by the layout test in panels.rs
pub(super) fn fixed_drag_value(
    ui: &mut egui::Ui,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    speed: f32,
) -> egui::Response {
    fixed_drag_value_fmt(ui, value, range, speed, Unit::Plain(1), false)
}

fn fixed_drag_value_fmt(
    ui: &mut egui::Ui,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    speed: f32,
    unit: Unit,
    accent: bool,
) -> egui::Response {
    let slot_size = egui::vec2(theme::SLIDER_VALUE_W, 18.0);
    let (slot, _) = ui.allocate_exact_size(slot_size, egui::Sense::hover());
    let mut slot_ui = ui.new_child(egui::UiBuilder::new().max_rect(slot).layout(
        egui::Layout::centered_and_justified(egui::Direction::LeftToRight),
    ));
    slot_ui.set_clip_rect(slot.intersect(ui.clip_rect()));
    slot_ui.spacing_mut().interact_size = slot_size;
    if accent {
        slot_ui.visuals_mut().override_text_color = Some(theme::ACCENT_HOVER);
    }
    // A borderless field that reads as text until hovered or edited.
    let visuals = slot_ui.visuals_mut();
    visuals.widgets.inactive.weak_bg_fill = egui::Color32::TRANSPARENT;
    visuals.widgets.inactive.bg_stroke = egui::Stroke::NONE;
    slot_ui.add(
        egui::DragValue::new(value)
            .range(range)
            .speed(speed)
            .custom_formatter(move |n, _| unit.format(n))
            .custom_parser(Unit::parse),
    )
}

/// Three-stop horizontal gradient rectangle.
fn gradient_rect(painter: &egui::Painter, rect: egui::Rect, stops: [egui::Color32; 3]) {
    let mut mesh = egui::Mesh::default();
    for (i, x) in [rect.left(), rect.center().x, rect.right()]
        .into_iter()
        .enumerate()
    {
        mesh.colored_vertex(egui::pos2(x, rect.top()), stops[i]);
        mesh.colored_vertex(egui::pos2(x, rect.bottom()), stops[i]);
    }
    for i in 0..2u32 {
        let b = i * 2;
        mesh.add_triangle(b, b + 1, b + 2);
        mesh.add_triangle(b + 1, b + 3, b + 2);
    }
    painter.add(mesh);
}

/// The slider track: rail (or gradient), fill from the neutral point, tick
/// at the default, round handle. Click or drag sets the value (⇧ drags
/// fine), arrow keys step it while focused (⇧ ×10, ⌥ ×0.1).
#[allow(clippy::too_many_arguments)]
fn slider_track(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut f32,
    range: &std::ops::RangeInclusive<f32>,
    default: f32,
    spec: SliderSpec,
    width: f32,
    unit: Unit,
) -> egui::Response {
    let scale = Scale::new(range, spec.log);
    let (rect, mut response) =
        ui.allocate_exact_size(egui::vec2(width, 20.0), egui::Sense::click_and_drag());
    let inset = 7.0;
    let x0 = rect.left() + inset;
    let w = (rect.width() - 2.0 * inset).max(1.0);
    let before = *value;
    if response.drag_started() || response.clicked() {
        if let Some(pos) = response.interact_pointer_pos() {
            *value = scale.value_at((pos.x - x0) / w);
        }
    } else if response.dragged() {
        let fine = ui.input(|i| i.modifiers.shift);
        if fine {
            *value = scale.value_at(scale.to_t(*value) + response.drag_delta().x / w * 0.1);
        } else if let Some(pos) = response.interact_pointer_pos() {
            *value = scale.value_at((pos.x - x0) / w);
        }
    }
    if response.has_focus() {
        ui.memory_mut(|m| {
            m.set_focus_lock_filter(
                response.id,
                egui::EventFilter {
                    horizontal_arrows: true,
                    vertical_arrows: true,
                    ..Default::default()
                },
            );
        });
        let (dir, mods, home, end) = ui.input(|i| {
            let mut d = 0.0f32;
            if i.key_pressed(egui::Key::ArrowRight) || i.key_pressed(egui::Key::ArrowUp) {
                d += 1.0;
            }
            if i.key_pressed(egui::Key::ArrowLeft) || i.key_pressed(egui::Key::ArrowDown) {
                d -= 1.0;
            }
            (
                d,
                i.modifiers,
                i.key_pressed(egui::Key::Home),
                i.key_pressed(egui::Key::End),
            )
        });
        if dir != 0.0 {
            let mul = if mods.shift {
                10.0
            } else if mods.alt {
                0.1
            } else {
                1.0
            };
            if scale.log {
                *value = scale.value_at(scale.to_t(*value) + dir * 0.005 * mul);
            } else {
                let step = 10f32
                    .powi(-(unit.decimals() as i32))
                    .max((scale.hi - scale.lo) / 200.0);
                *value = (*value + dir * step * mul).clamp(scale.lo, scale.hi);
            }
        }
        if home {
            *value = scale.lo;
        }
        if end {
            *value = scale.hi;
        }
    }

    let painter = ui.painter_at(rect.expand(4.0));
    let enabled = ui.is_enabled();
    let fade = |c: egui::Color32| if enabled { c } else { c.gamma_multiply(0.4) };
    let cy = rect.center().y;
    let x_of = |t: f32| x0 + t * w;
    let bipolar = scale.lo < 0.0 && scale.hi > 0.0;
    if let Some(stops) = spec.gradient {
        let track =
            egui::Rect::from_min_max(egui::pos2(x0, cy - 3.0), egui::pos2(x0 + w, cy + 3.0));
        gradient_rect(&painter, track, stops.map(fade));
    } else {
        painter.rect_filled(
            egui::Rect::from_min_max(egui::pos2(x0, cy - 2.0), egui::pos2(x0 + w, cy + 2.0)),
            2.0,
            fade(theme::SLIDER_RAIL),
        );
        let origin = if bipolar { scale.to_t(0.0) } else { 0.0 };
        let t = scale.to_t(*value);
        let (a, b) = (origin.min(t), origin.max(t));
        if b - a > 0.001 {
            painter.rect_filled(
                egui::Rect::from_min_max(
                    egui::pos2(x_of(a), cy - 2.0),
                    egui::pos2(x_of(b), cy + 2.0),
                ),
                2.0,
                fade(theme::ACCENT),
            );
        }
    }
    if bipolar || spec.gradient.is_some() {
        let tick = x_of(scale.to_t(if bipolar && spec.gradient.is_none() {
            0.0
        } else {
            default
        }));
        painter.line_segment(
            [egui::pos2(tick, cy - 6.0), egui::pos2(tick, cy + 6.0)],
            egui::Stroke::new(1.0, fade(theme::SLIDER_TICK)),
        );
    }
    let hx = x_of(scale.to_t(*value));
    let radius = if response.hovered() || response.dragged() {
        7.0
    } else {
        6.0
    };
    if response.has_focus() {
        painter.circle_stroke(
            egui::pos2(hx, cy),
            radius + 2.5,
            egui::Stroke::new(2.0, theme::ACCENT.gamma_multiply(0.8)),
        );
    }
    painter.circle_filled(
        egui::pos2(hx, cy + 1.0),
        radius + 0.5,
        egui::Color32::from_black_alpha(120),
    );
    painter.circle_filled(egui::pos2(hx, cy), radius, fade(theme::SLIDER_HANDLE));

    if *value != before {
        response.mark_changed();
    }
    let v = f64::from(*value);
    response.widget_info(|| egui::WidgetInfo::slider(enabled, v, crate::i18n::text(label)));
    response
}

// ------------------------------------------------------------ tool search

fn search_id() -> egui::Id {
    egui::Id::new("inspector-search")
}

/// Current inspector search: (lower-case query, whole card matches).
fn search_state(ui: &egui::Ui) -> Option<(String, bool)> {
    ui.data(|d| d.get_temp::<(String, bool)>(search_id()))
        .filter(|(query, _)| !query.is_empty())
}

fn set_search(ui: &egui::Ui, query: &str, card_matches: bool) {
    ui.data_mut(|d| d.insert_temp(search_id(), (query.to_lowercase(), card_matches)));
}

/// A slider row is hidden while searching unless its label (or its whole
/// card) matches.
fn label_matches(label: &str, query: &str) -> bool {
    let title_case = label.chars().next().map_or_else(String::new, |first| {
        first.to_uppercase().collect::<String>() + &label[first.len_utf8()..]
    });
    label.to_lowercase().contains(query)
        || crate::i18n::text(label).to_lowercase().contains(query)
        || crate::i18n::text(&title_case)
            .to_lowercase()
            .contains(query)
}

fn search_hides(ui: &egui::Ui, label: &str) -> bool {
    search_state(ui).is_some_and(|(q, card)| !card && !label_matches(label, &q))
}

fn search_highlights(ui: &egui::Ui, label: &str) -> bool {
    search_state(ui).is_some_and(|(q, _)| label_matches(label, &q))
}

/// Searchable control names per card, in addition to slider labels drawn
/// at run time. Used to decide which cards a search shows.
const CARD_INDEX: &[(&str, &[&str])] = &[
    (
        "History",
        &["snapshot", "version", "virtual copy", "undo", "step"],
    ),
    (
        "Basic",
        &[
            "profile",
            "look",
            "auto balance",
            "exposure only",
            "temp",
            "tint",
            "white balance",
            "exposure",
            "brightness",
            "contrast",
            "highlights",
            "shadows",
            "whites",
            "blacks",
            "recover",
            "recovery",
            "propagation",
            "look strength",
            "neutral",
        ],
    ),
    (
        "Tone Curve",
        &[
            "curve",
            "highlights",
            "lights",
            "darks",
            "shadows",
            "parametric",
            "pipette",
        ],
    ),
    (
        "Color",
        &[
            "vibrance",
            "saturation",
            "hsl",
            "mixer",
            "hue",
            "luminance",
            "grading",
            "wheel",
            "balance",
            "monochrome",
            "black & white",
            "red",
            "orange",
            "yellow",
            "green",
            "aqua",
            "blue",
            "purple",
            "magenta",
        ],
    ),
    (
        "Local & retouch",
        &[
            "brush",
            "radial",
            "gradient",
            "luminance",
            "color",
            "mask",
            "heal",
            "clone",
            "spot",
            "feather",
            "local",
        ],
    ),
    (
        "Detail",
        &[
            "demosaic",
            "ahd",
            "sharpening",
            "Restore blur",
            "Blur radius",
            "Noise guard",
            "sharpness",
            "amount",
            "radius",
            "detail",
            "masking",
            "halo",
            "noise",
            "denoise",
            "luminance",
            "chroma",
            "hot-pixel",
            "loupe",
        ],
    ),
    (
        "Effects",
        &[
            "texture",
            "clarity",
            "dehaze",
            "vignette",
            "midpoint",
            "roundness",
            "feather",
            "grain",
            "size",
            "roughness",
        ],
    ),
    (
        "Lens Corrections",
        &[
            "lens",
            "profile",
            "vignetting",
            "distortion",
            "lateral",
            "chromatic",
            "ca",
            "red ca",
            "blue ca",
            "crop to valid",
            "fringe",
            "defringe",
            "purple",
            "green",
            "hue",
            "range",
            "bloom",
            "fringe width",
            "edge protection",
        ],
    ),
    (
        "Geometry",
        &[
            "crop",
            "aspect",
            "auto level",
            "straighten",
            "rotate",
            "flip",
            "vertical",
            "horizontal",
            "perspective",
            "scale",
            "upright",
            "guides",
            "calibration",
        ],
    ),
    ("Resolution", &["resolution", "supersample", "full"]),
    (
        "HDR",
        &[
            "hdr",
            "sdr",
            "target",
            "headroom",
            "peak",
            "ref. white",
            "reference white",
            "shoulder",
        ],
    ),
    (
        "Metadata",
        &[
            "metadata", "camera", "lens", "iso", "shutter", "aperture", "focal", "captured", "gps",
            "exif",
        ],
    ),
];

fn card_matches(title: &str, query: &str) -> (bool, usize) {
    let title_match = label_matches(title, query);
    let hits = CARD_INDEX
        .iter()
        .find(|(t, _)| *t == title)
        .map_or(0, |(_, labels)| {
            labels.iter().filter(|l| label_matches(l, query)).count()
        });
    (title_match, hits)
}

// ------------------------------------------------------------ slider rows

/// Standard develop slider: label, track, number, hover reset. The label
/// scrubs the value when dragged; label or track double-click resets; the
/// number accepts typing. Returns true if the value changed this frame.
pub(super) fn param_slider(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    default: f32,
) -> bool {
    param_slider_spec(ui, label, value, range, default, SliderSpec::default())
}

pub(super) fn param_slider_spec(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    default: f32,
    spec: SliderSpec,
) -> bool {
    if search_hides(ui, label) {
        return false;
    }
    let unit = spec.unit.resolve(&range);
    let scale = Scale::new(&range, spec.log);
    let epsilon = 10f32.powi(-(unit.decimals() as i32)) * 0.5;
    let modified = (*value - default).abs() > epsilon;
    let highlight = search_highlights(ui, label);
    let before = *value;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        // Label: left-aligned, scrubbable, double-click resets.
        let (label_rect, label_resp) = ui.allocate_exact_size(
            egui::vec2(theme::SLIDER_LABEL_W, 18.0),
            egui::Sense::click_and_drag(),
        );
        let label_color = if highlight {
            theme::ACCENT_HOVER
        } else if modified || label_resp.hovered() {
            theme::TEXT_PRIMARY
        } else {
            theme::TEXT_SECONDARY
        };
        ui.painter().with_clip_rect(label_rect).text(
            label_rect.left_center(),
            egui::Align2::LEFT_CENTER,
            crate::i18n::text(label),
            egui::FontId::proportional(11.5),
            if ui.is_enabled() {
                label_color
            } else {
                label_color.gamma_multiply(0.5)
            },
        );
        if label_resp.hovered() || label_resp.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        }
        let reset_from_label = label_resp.double_clicked();
        let label_drag = label_resp.dragged().then(|| label_resp.drag_delta().x);
        label_resp.on_hover_text(crate::i18n::text(if label == "Exposure" {
            "Drag to adjust · double-click to reset · ⇧ double-click for Auto Exposure"
        } else {
            "Drag to adjust · double-click to reset"
        }));

        let spacing = ui.spacing().item_spacing.x;
        let slider_w =
            (ui.available_width() - theme::SLIDER_VALUE_W - theme::SLIDER_RESET_W - 2.0 * spacing)
                .max(60.0);
        let track = slider_track(ui, label, value, &range, default, spec, slider_w, unit);
        let track_w = (slider_w - 14.0).max(1.0);
        if let Some(dx) = label_drag {
            let fine = ui.input(|i| i.modifiers.shift);
            *value =
                scale.value_at(scale.to_t(*value) + dx / track_w * if fine { 0.1 } else { 1.0 });
        }
        if label == "Masking"
            && track.hovered()
            && ui.input(|input| input.modifiers.alt && input.pointer.primary_down())
        {
            ui.ctx().data_mut(|data| {
                data.insert_temp(egui::Id::new("sharpen-mask-preview"), true);
            });
        }
        let reset_from_track = track.double_clicked();
        track.on_hover_text(crate::i18n::text(
            "Drag to adjust · ⇧ drag for fine control · double-click to reset",
        ));

        let drag_speed = if scale.log {
            (*value * 0.004).max(f32::EPSILON)
        } else {
            ((scale.hi - scale.lo) / slider_w).max(f32::EPSILON)
        };
        fixed_drag_value_fmt(ui, value, range.clone(), drag_speed, unit, modified);

        // Reset slot: always reserved so the row never reflows.
        let (reset_rect, reset_resp) = ui.allocate_exact_size(
            egui::vec2(theme::SLIDER_RESET_W, 18.0),
            egui::Sense::click(),
        );
        let row_hovered = ui.rect_contains_pointer(ui.min_rect());
        let show_reset = modified && row_hovered && ui.is_enabled();
        let reset_clicked = show_reset && reset_resp.clicked();
        if show_reset {
            paint_reset_icon(
                ui.painter(),
                reset_rect.center(),
                if reset_resp.hovered() {
                    theme::TEXT_PRIMARY
                } else {
                    theme::TEXT_FAINT
                },
            );
            reset_resp.on_hover_text(format!(
                "{} {}",
                crate::i18n::text("Reset"),
                crate::i18n::text(label)
            ));
        }

        let reset = reset_from_label || reset_from_track || reset_clicked;
        let auto_exposure = (reset_from_label || reset_from_track)
            && label == "Exposure"
            && ui.input(|input| input.modifiers.shift);
        if auto_exposure {
            ui.ctx()
                .data_mut(|data| data.insert_temp(egui::Id::new("auto-exposure-request"), true));
        } else if reset {
            *value = default;
        }
    });
    *value = value.clamp(*range.start(), *range.end());
    *value != before
}

/// Circular "reset" arrow, drawn so it does not depend on font coverage.
pub(super) fn paint_reset_icon(painter: &egui::Painter, center: egui::Pos2, color: egui::Color32) {
    let r = 4.5;
    let points: Vec<egui::Pos2> = (0..=16)
        .map(|i| {
            let a = std::f32::consts::PI * (0.25 + 1.5 * i as f32 / 16.0);
            center + egui::vec2(a.cos(), -a.sin()) * r
        })
        .collect();
    let end = *points.last().expect("arc points");
    painter.add(egui::Shape::line(points, egui::Stroke::new(1.3, color)));
    painter.add(egui::Shape::convex_polygon(
        vec![
            end + egui::vec2(-2.5, -1.5),
            end + egui::vec2(2.2, -1.2),
            end + egui::vec2(0.0, 2.6),
        ],
        color,
        egui::Stroke::NONE,
    ));
}

/// An eye (group active) or a struck-through eye (bypassed).
fn paint_eye_icon(painter: &egui::Painter, center: egui::Pos2, open: bool, color: egui::Color32) {
    let w = 6.5;
    let top: Vec<egui::Pos2> = (0..=12)
        .map(|i| {
            let t = i as f32 / 12.0;
            egui::pos2(
                center.x - w + 2.0 * w * t,
                center.y - 3.6 * (std::f32::consts::PI * t).sin(),
            )
        })
        .collect();
    let bottom: Vec<egui::Pos2> = top
        .iter()
        .map(|p| egui::pos2(p.x, 2.0 * center.y - p.y))
        .collect();
    painter.add(egui::Shape::line(top, egui::Stroke::new(1.2, color)));
    painter.add(egui::Shape::line(bottom, egui::Stroke::new(1.2, color)));
    painter.circle_filled(center, 1.8, color);
    if !open {
        painter.line_segment(
            [
                center + egui::vec2(-5.5, 5.0),
                center + egui::vec2(5.5, -5.0),
            ],
            egui::Stroke::new(1.4, color),
        );
    }
}

// ------------------------------------------------------------------ cards

/// Inspector sections for the jump bar, in scroll order.
pub(super) const SECTIONS: [&str; 6] = ["Light", "Color", "Detail", "Optics", "Geometry", "Output"];

/// Per-frame card bookkeeping shared by every card in the inspector.
pub(super) struct Cards {
    query: String,
    bypass_mask: u16,
    solo: bool,
    resets: Vec<Group>,
    opened: Option<&'static str>,
    jump: Option<usize>,
    focus: Option<String>,
    section_tops: [Option<f32>; SECTIONS.len()],
}

/// Collapsible tool group rendered as a rounded card. The header shows how
/// many controls differ from their defaults, a hover reset for the group
/// and, where meaningful, a bypass eye. Returns whether the body was shown.
#[allow(clippy::too_many_arguments)]
pub(super) fn tool_group(
    ui: &mut egui::Ui,
    cards: &mut Cards,
    group: Option<Group>,
    title: &'static str,
    section: Option<usize>,
    open: &mut bool,
    modified: usize,
    body: impl FnOnce(&mut egui::Ui),
) {
    let query = cards.query.to_lowercase();
    let searching = !query.is_empty();
    let (title_match, hits) = card_matches(title, &query);
    if searching && !title_match && hits == 0 {
        return;
    }
    let bypassed = group.is_some_and(|g| cards.bypass_mask & g.bit() != 0);
    theme::card_frame()
        .inner_margin(egui::Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let (header, header_resp) = ui
                .allocate_exact_size(egui::vec2(ui.available_width(), 24.0), egui::Sense::click());
            if let Some(section) = section {
                if cards.section_tops[section].is_none() {
                    cards.section_tops[section] = Some(header.top());
                    if cards.jump == Some(section) {
                        *open = true;
                        header_resp.scroll_to_me(Some(egui::Align::Min));
                    }
                }
            }
            if cards.focus.as_deref() == Some(title) {
                *open = true;
                header_resp.scroll_to_me(Some(egui::Align::Min));
            }
            let hovered = header_resp.hovered();
            let painter = ui.painter();
            let shown = *open || searching;
            // Chevron.
            let c = egui::pos2(header.left() + 5.0, header.center().y);
            let tri = if shown {
                vec![
                    c + egui::vec2(-3.5, -2.0),
                    c + egui::vec2(3.5, -2.0),
                    c + egui::vec2(0.0, 2.5),
                ]
            } else {
                vec![
                    c + egui::vec2(-2.0, -3.5),
                    c + egui::vec2(2.5, 0.0),
                    c + egui::vec2(-2.0, 3.5),
                ]
            };
            painter.add(egui::Shape::convex_polygon(
                tri,
                theme::TEXT_FAINT,
                egui::Stroke::NONE,
            ));
            let title_color = if bypassed {
                theme::TEXT_FAINT.gamma_multiply(0.6)
            } else if hovered {
                theme::TEXT_PRIMARY
            } else {
                theme::TEXT_FAINT
            };
            let galley = painter.layout_no_wrap(
                crate::i18n::text(title).to_uppercase(),
                egui::FontId::proportional(10.5),
                title_color,
            );
            let text_pos = egui::pos2(
                header.left() + 16.0,
                header.center().y - galley.size().y / 2.0,
            );
            let text_w = galley.size().x;
            painter.galley(text_pos, galley, title_color);
            if bypassed {
                let y = header.center().y;
                painter.line_segment(
                    [
                        egui::pos2(text_pos.x, y),
                        egui::pos2(text_pos.x + text_w, y),
                    ],
                    egui::Stroke::new(1.0, theme::TEXT_FAINT),
                );
            }
            if modified > 0 {
                let badge_text = modified.to_string();
                let bg = painter.layout_no_wrap(
                    badge_text,
                    egui::FontId::proportional(9.5),
                    theme::ACCENT_HOVER,
                );
                let bw = (bg.size().x + 10.0).max(16.0);
                let badge = egui::Rect::from_center_size(
                    egui::pos2(text_pos.x + text_w + 8.0 + bw / 2.0, header.center().y),
                    egui::vec2(bw, 15.0),
                );
                painter.rect_filled(badge, 7.5, theme::ACCENT.gamma_multiply(0.22));
                painter.galley(badge.center() - bg.size() / 2.0, bg, theme::ACCENT_HOVER);
            }
            // Right-side controls: bypass eye, then reset on hover.
            let mut right = header.right();
            let mut control_clicked = false;
            if let Some(g) = group.filter(|g| g.can_bypass()) {
                let eye_rect = egui::Rect::from_center_size(
                    egui::pos2(right - 9.0, header.center().y),
                    egui::vec2(18.0, 18.0),
                );
                let eye = ui.interact(eye_rect, ui.id().with((title, "eye")), egui::Sense::click());
                let color = if bypassed {
                    theme::WARNING
                } else if eye.hovered() {
                    theme::TEXT_PRIMARY
                } else {
                    theme::TEXT_FAINT
                };
                if eye.hovered() || bypassed || hovered || modified > 0 {
                    paint_eye_icon(ui.painter(), eye_rect.center(), !bypassed, color);
                }
                if eye.clicked() {
                    cards.bypass_mask ^= g.bit();
                    control_clicked = true;
                }
                eye.on_hover_text(crate::i18n::text(
                    &(if bypassed {
                        crate::i18n::format(
                            "{0} is bypassed · click to switch it back on",
                            &[title.to_string()],
                        )
                    } else {
                        crate::i18n::format(
                            "Bypass {0} to compare (not recorded in history)",
                            &[title.to_string()],
                        )
                    }),
                ));
                right -= 22.0;
            }
            if let (Some(g), true) = (group, modified > 0) {
                let reset_rect = egui::Rect::from_center_size(
                    egui::pos2(right - 9.0, header.center().y),
                    egui::vec2(18.0, 18.0),
                );
                let reset = ui.interact(
                    reset_rect,
                    ui.id().with((title, "reset")),
                    egui::Sense::click(),
                );
                if hovered || reset.hovered() {
                    paint_reset_icon(
                        ui.painter(),
                        reset_rect.center(),
                        if reset.hovered() {
                            theme::TEXT_PRIMARY
                        } else {
                            theme::TEXT_FAINT
                        },
                    );
                }
                if reset.clicked() {
                    cards.resets.push(g);
                    control_clicked = true;
                }
                reset.on_hover_text(format!(
                    "{} {}",
                    crate::i18n::text("Reset"),
                    crate::i18n::text(title)
                ));
            }
            if header_resp.clicked() && !control_clicked && !searching {
                *open = !*open;
                let solo = cards.solo || ui.input(|i| i.modifiers.alt);
                if *open && solo {
                    cards.opened = Some(title);
                }
            }
            header_resp.on_hover_text(crate::i18n::text(
                "Click to expand · Option-click to open only this card",
            ));
            if shown {
                ui.add_space(2.0);
                set_search(ui, &cards.query, title_match);
                if bypassed {
                    ui.multiply_opacity(0.55);
                }
                body(ui);
                set_search(ui, &cards.query, false);
            }
        });
    ui.add_space(6.0);
}

/// Disclosure for a card's specialist controls. Open while searching.
fn advanced(
    ui: &mut egui::Ui,
    mask: &mut u16,
    bit: u16,
    count: usize,
    body: impl FnOnce(&mut egui::Ui),
) {
    let searching = search_state(ui).is_some();
    let open = searching || *mask & bit != 0;
    if !searching {
        let text = if open {
            crate::i18n::text("Hide advanced").to_owned()
        } else {
            format!(
                "{} · {count} {}",
                crate::i18n::text("Advanced"),
                crate::i18n::text("extra controls")
            )
        };
        let resp = ui.add(
            egui::Label::new(
                egui::RichText::new(crate::i18n::text(text.as_ref()))
                    .size(11.0)
                    .color(theme::ACCENT_HOVER),
            )
            .sense(egui::Sense::click()),
        );
        if resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if resp.clicked() {
            *mask ^= bit;
        }
    }
    if open {
        egui::Frame::new()
            .inner_margin(egui::Margin {
                left: 8,
                right: 0,
                top: 2,
                bottom: 2,
            })
            .stroke(egui::Stroke::NONE)
            .show(ui, |ui| {
                let rect = ui.max_rect();
                ui.painter().line_segment(
                    [
                        egui::pos2(rect.left() - 6.0, rect.top()),
                        egui::pos2(rect.left() - 6.0, rect.bottom()),
                    ],
                    egui::Stroke::new(2.0, theme::HAIRLINE),
                );
                body(ui);
            });
    }
}

const ADV_BASIC: u16 = 1;
const ADV_DETAIL: u16 = 2;
const ADV_LENS: u16 = 4;
const ADV_GEOMETRY: u16 = 8;
const ADV_HDR: u16 = 16;

fn sub_heading(ui: &mut egui::Ui, text: &str) {
    ui.label(
        egui::RichText::new(crate::i18n::text(text))
            .size(11.0)
            .color(theme::TEXT_SECONDARY),
    );
}

// ------------------------------------------------------------- retouching

#[allow(clippy::too_many_arguments)]
pub(super) fn retouch_panel(
    ui: &mut egui::Ui,
    edit: &mut sp_core::EditState,
    tool: &mut RetouchTool,
    active_local: &mut Option<usize>,
    radius: &mut f32,
    feather: &mut f32,
    color_range_picker: &mut Option<usize>,
    show_overlay: &mut bool,
    refinement: &mut crate::state::SpotUi,
) {
    use sp_core::edit::{ColorParams, LocalAdjustment, LocalMask, MaskOperation, ToneParams};

    sub_heading(ui, "Local adjustments");
    ui.checkbox(show_overlay, crate::i18n::text("Show mask coverage (M)"));
    ui.horizontal_wrapped(|ui| {
        for (label, next_tool) in [
            ("Brush", RetouchTool::Brush),
            ("Radial", RetouchTool::Radial),
            ("Gradient", RetouchTool::LinearGradient),
        ] {
            if ui.selectable_label(*tool == next_tool, label).clicked() {
                let number = edit.local_adjustments.len() + 1;
                let mask = match next_tool {
                    RetouchTool::Brush => LocalMask::Paint {
                        strokes: Vec::new(),
                        settings: sp_core::mask::BrushSettings {
                            size: radius.max(0.002),
                            feather: *feather,
                            ..Default::default()
                        },
                    },
                    RetouchTool::Radial => LocalMask::Radial {
                        center: [0.5, 0.5],
                        radius: [0.15, 0.15],
                        feather: *feather,
                    },
                    RetouchTool::LinearGradient => LocalMask::LinearGradient {
                        start: [0.25, 0.5],
                        end: [0.75, 0.5],
                    },
                    _ => unreachable!(),
                };
                edit.local_adjustments.push(LocalAdjustment {
                    name: crate::i18n::format("Local {0}", &[number.to_string()]),
                    enabled: true,
                    mask,
                    tone: ToneParams::default(),
                    color: ColorParams::default(),
                    ..sp_core::edit::LocalAdjustment::default()
                });
                *active_local = Some(edit.local_adjustments.len() - 1);
                *tool = next_tool;
            }
        }
        if ui.button(crate::i18n::text("Luminance")).clicked() {
            let number = edit.local_adjustments.len() + 1;
            edit.local_adjustments.push(LocalAdjustment {
                name: crate::i18n::format("Luminance {0}", &[number.to_string()]),
                enabled: true,
                mask: LocalMask::LuminanceRange {
                    minimum: 0.25,
                    maximum: 0.75,
                    feather: 0.1,
                },
                tone: ToneParams::default(),
                color: ColorParams::default(),
                ..sp_core::edit::LocalAdjustment::default()
            });
            *active_local = Some(edit.local_adjustments.len() - 1);
            *tool = RetouchTool::None;
        }
        if ui.button(crate::i18n::text("Color")).clicked() {
            let number = edit.local_adjustments.len() + 1;
            edit.local_adjustments.push(LocalAdjustment {
                name: crate::i18n::format("Color {0}", &[number.to_string()]),
                enabled: true,
                mask: LocalMask::ColorRange {
                    target_ab: [0.0, 0.0],
                    radius: 0.04,
                    feather: 0.04,
                },
                tone: ToneParams::default(),
                color: ColorParams::default(),
                ..sp_core::edit::LocalAdjustment::default()
            });
            let index = edit.local_adjustments.len() - 1;
            *active_local = Some(index);
            *color_range_picker = Some(index);
            *tool = RetouchTool::None;
        }
    });

    for (index, adjustment) in edit.local_adjustments.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            ui.checkbox(&mut adjustment.enabled, crate::i18n::text(""));
            let kind = match &adjustment.mask {
                LocalMask::Brush { .. } | LocalMask::Paint { .. } => "brush",
                LocalMask::Radial { .. } => "radial",
                LocalMask::LinearGradient { .. } => "gradient",
                LocalMask::LuminanceRange { .. } => "luminance",
                LocalMask::ColorRange { .. } => "color",
                LocalMask::Alpha(_) => "alpha mask",
                LocalMask::Composite { .. } => "combined",
                LocalMask::Modified { .. } => "coverage",
            };
            if ui
                .selectable_label(*active_local == Some(index), &adjustment.name)
                .clicked()
            {
                *active_local = Some(index);
            }
            ui.label(egui::RichText::new(kind).small().color(theme::TEXT_FAINT));
        });
    }
    let mut delete_local = None;
    let mut duplicate_local = None;
    let mut move_local = None;
    let mut combine_local = None;
    let names: Vec<_> = edit
        .local_adjustments
        .iter()
        .map(|a| a.name.clone())
        .collect();
    if let Some(index) = active_local.filter(|index| *index < edit.local_adjustments.len()) {
        let adjustment = &mut edit.local_adjustments[index];
        ui.text_edit_singleline(&mut adjustment.name);
        local_preset_controls(ui, adjustment);
        ui.collapsing(crate::i18n::text("Refine visible edges"),|ui|{
            param_slider(ui,"Edge radius",&mut refinement.refine_radius,1.0..=12.0,6.0);
            param_slider(ui,"Color tolerance",&mut refinement.refine_tolerance,0.005..=0.5,0.08);
            ui.label(egui::RichText::new(crate::i18n::text("Use image colors to reduce mask spill. Bakes this mask at up to 1536 pixels on its long edge; undo restores the original shape.")).small().color(theme::TEXT_FAINT));
            ui.checkbox(&mut refinement.refine_after_stroke,crate::i18n::text("Protect edges after each stroke (preview resolution)"));
            if refinement.refine_busy{ui.small(crate::i18n::text("Refining mask edges…"));}
            if ui.add_enabled(!refinement.refine_busy,egui::Button::new(crate::i18n::text("Apply guided refinement"))).clicked() {refinement.refine_request=Some(index);refinement.refine_stroke_pending=false;}
        });
        ui.horizontal(|ui| {
            if ui.small_button(crate::i18n::text("Duplicate")).clicked() {
                duplicate_local = Some(index);
            }
            if ui
                .add_enabled(
                    index > 0,
                    egui::Button::new(crate::i18n::text("Move up")).small(),
                )
                .clicked()
            {
                move_local = Some((index, index - 1));
            }
            if ui
                .add_enabled(
                    index + 1 < names.len(),
                    egui::Button::new(crate::i18n::text("Move down")).small(),
                )
                .clicked()
            {
                move_local = Some((index, index + 1));
            }
        });
        ui.checkbox(
            &mut adjustment.mask_inverted,
            crate::i18n::text("Invert coverage"),
        );
        let mut opacity = adjustment.opacity * 100.0;
        param_slider_spec(
            ui,
            "Amount",
            &mut opacity,
            0.0..=100.0,
            100.0,
            SliderSpec::unit(Unit::Percent),
        );
        adjustment.opacity = opacity / 100.0;
        ui.menu_button(crate::i18n::text("Combine with another mask…"), |ui| {
            ui.label(crate::i18n::text("Use a copy of another mask’s coverage"));
            for (other, name) in names
                .iter()
                .enumerate()
                .filter(|(other, _)| *other != index)
            {
                ui.menu_button(name, |ui| {
                    for (label, operation) in [
                        ("Add", MaskOperation::Add),
                        ("Subtract", MaskOperation::Subtract),
                        ("Intersect", MaskOperation::Intersect),
                    ] {
                        if ui.button(crate::i18n::text(label)).clicked() {
                            combine_local = Some((index, other, operation));
                            ui.close();
                        }
                    }
                });
            }
            if names.len() < 2 {
                ui.label(crate::i18n::text(
                    "Create another brush, gradient or selection first.",
                ));
            }
        });
        match &mut adjustment.mask {
            LocalMask::Paint { strokes, settings } => {
                brush_settings_editor(ui, settings);
                *radius = settings.size;
                *feather = settings.feather;
                ui.horizontal(|ui| {
                    ui.selectable_value(tool, RetouchTool::Brush, crate::i18n::text("Paint"));
                    ui.selectable_value(
                        tool,
                        RetouchTool::MaskSubtract,
                        crate::i18n::text("Erase"),
                    );
                    if ui
                        .add_enabled(
                            !strokes.is_empty(),
                            egui::Button::new(crate::i18n::text("Undo stroke")),
                        )
                        .clicked()
                    {
                        strokes.pop();
                    }
                });
            }
            mask @ (LocalMask::Composite { .. } | LocalMask::Modified { .. }) => {
                ui.push_id(("mask_components", index), |ui| {
                    composed_mask_editor(ui, mask, 0)
                });
            }
            LocalMask::Alpha(mask) => {
                ui.label({
                    let __l10n_args = (&(mask.width), &(mask.height));
                    crate::i18n::format(
                        "{0} × {1} · 16-bit alpha",
                        &[__l10n_args.0.to_string(), __l10n_args.1.to_string()],
                    )
                });
                ui.label(egui::RichText::new(&mask.source).small());
                ui.label(
                    egui::RichText::new(crate::i18n::text(
                        "M toggles mask coverage · inspect edges at 1:1",
                    ))
                    .small(),
                );

                ui.horizontal(|ui| {
                    ui.selectable_value(tool, RetouchTool::MaskAdd, crate::i18n::text("Paint +"));
                    ui.selectable_value(
                        tool,
                        RetouchTool::MaskSubtract,
                        crate::i18n::text("Paint −"),
                    );
                    ui.selectable_value(tool, RetouchTool::None, crate::i18n::text("Done"));
                });
                param_slider(ui, "Repair size", radius, 0.0001..=0.10, 0.01);
                param_slider(ui, "Repair feather", feather, 0.0..=1.0, 0.5);
                if ui
                    .button(crate::i18n::text("Undo last mask repair"))
                    .clicked()
                {
                    mask.repairs.pop();
                }
            }
            LocalMask::Brush {
                size,
                feather: mask_feather,
                ..
            } => {
                param_slider(ui, "Brush size", size, 0.002..=0.25, 0.03);
                param_slider(ui, "Feather", mask_feather, 0.0..=1.0, 0.5);
                *radius = *size;
                *feather = *mask_feather;
            }
            LocalMask::Radial {
                feather: mask_feather,
                ..
            } => {
                param_slider(ui, "Feather", mask_feather, 0.0..=1.0, 0.5);
                *feather = *mask_feather;
            }
            LocalMask::LinearGradient { .. } => {
                ui.label(
                    egui::RichText::new(crate::i18n::text("Drag its axis in the image")).small(),
                );
            }
            LocalMask::LuminanceRange {
                minimum,
                maximum,
                feather: mask_feather,
            } => {
                param_slider(ui, "Minimum", minimum, 0.0..=1.0, 0.25);
                param_slider(ui, "Maximum", maximum, 0.0..=1.0, 0.75);
                param_slider(ui, "Feather", mask_feather, 0.0..=0.5, 0.1);
                if *maximum < *minimum {
                    std::mem::swap(minimum, maximum);
                }
            }
            LocalMask::ColorRange {
                target_ab,
                radius: color_radius,
                feather: color_feather,
            } => {
                ui.label(
                    egui::RichText::new({
                        let __l10n_args = (&(target_ab[0]), &(target_ab[1]));
                        crate::i18n::format(
                            "Target Oklab · a {0} · b {1}",
                            &[
                                format!("{:+.3}", __l10n_args.0),
                                format!("{:+.3}", __l10n_args.1),
                            ],
                        )
                    })
                    .small()
                    .color(theme::TEXT_SECONDARY),
                );
                param_slider(ui, "Range", color_radius, 0.005..=0.30, 0.04);
                param_slider(ui, "Feather", color_feather, 0.0..=0.30, 0.04);
                if ui
                    .button(crate::i18n::text("Sample color in image"))
                    .clicked()
                {
                    *color_range_picker = Some(index);
                    *tool = RetouchTool::None;
                }
            }
        }
        ui.separator();
        param_slider_spec(
            ui,
            "Exposure",
            &mut adjustment.tone.exposure_ev,
            -5.0..=5.0,
            0.0,
            SliderSpec::unit(Unit::Ev),
        );
        param_slider(
            ui,
            "Brightness",
            &mut adjustment.tone.brightness,
            -100.0..=100.0,
            0.0,
        );
        param_slider(
            ui,
            "Contrast",
            &mut adjustment.tone.contrast,
            -100.0..=100.0,
            0.0,
        );
        param_slider(
            ui,
            "Highlights",
            &mut adjustment.tone.highlights,
            -100.0..=100.0,
            0.0,
        );
        param_slider(
            ui,
            "Shadows",
            &mut adjustment.tone.shadows,
            -100.0..=100.0,
            0.0,
        );
        param_slider(
            ui,
            "Saturation",
            &mut adjustment.color.saturation,
            -100.0..=100.0,
            0.0,
        );
        param_slider(
            ui,
            "Vibrance",
            &mut adjustment.color.vibrance,
            -100.0..=100.0,
            0.0,
        );
        param_slider(
            ui,
            "Whites",
            &mut adjustment.tone.whites,
            -100.0..=100.0,
            0.0,
        );
        param_slider(
            ui,
            "Blacks",
            &mut adjustment.tone.blacks,
            -100.0..=100.0,
            0.0,
        );
        ui.push_id(("local_extended", index), |ui| {
            ui.collapsing(crate::i18n::text("White balance"), |ui| {
                param_slider(
                    ui,
                    "Temperature",
                    &mut adjustment.temperature,
                    -100.0..=100.0,
                    0.0,
                );
                param_slider(ui, "Tint", &mut adjustment.tint, -100.0..=100.0, 0.0);
            });
            ui.collapsing(crate::i18n::text("Curves"), |ui| {
                let id = ui.id().with("channel");
                let mut channel = ui.data_mut(|data| data.get_temp::<u8>(id)).unwrap_or(0);
                ui.horizontal(|ui| {
                    for (value, label) in [(0, "Y"), (1, "R"), (2, "G"), (3, "B")] {
                        ui.selectable_value(&mut channel, value, crate::i18n::text(label));
                    }
                });
                ui.data_mut(|data| data.insert_temp(id, channel));
                let (curve, color) = match channel {
                    1 => (&mut adjustment.curve.red, theme::LABEL_RED),
                    2 => (&mut adjustment.curve.green, theme::LABEL_GREEN),
                    3 => (&mut adjustment.curve.blue, theme::LABEL_BLUE),
                    _ => (&mut adjustment.curve.luma, theme::TEXT_PRIMARY),
                };
                curve_editor(ui, curve, color);
                param_slider(
                    ui,
                    "Highlights",
                    &mut adjustment.curve.parametric.highlights,
                    -100.0..=100.0,
                    0.0,
                );
                param_slider(
                    ui,
                    "Lights",
                    &mut adjustment.curve.parametric.lights,
                    -100.0..=100.0,
                    0.0,
                );
                param_slider(
                    ui,
                    "Darks",
                    &mut adjustment.curve.parametric.darks,
                    -100.0..=100.0,
                    0.0,
                );
                param_slider(
                    ui,
                    "Shadows",
                    &mut adjustment.curve.parametric.shadows,
                    -100.0..=100.0,
                    0.0,
                );
            });
            ui.collapsing(crate::i18n::text("Color mixer & grading"), |ui| {
                let mut controls = sp_core::EditState {
                    color: adjustment.color,
                    ..Default::default()
                };
                let id = ui.id().with("hsl_tab");
                let mut tab = ui.data_mut(|data| data.get_temp::<u8>(id)).unwrap_or(0);
                color_mixer(ui, &mut controls, &mut tab);
                ui.data_mut(|data| data.insert_temp(id, tab));
                grading_wheels(ui, &mut controls);
                ui.checkbox(&mut controls.color.monochrome, crate::i18n::text("Black & white"));
                if controls.color.monochrome {
                    for (name, value) in BAND_NAMES.iter().zip(&mut controls.color.monochrome_mix) {
                        param_slider(ui, name, value, -100.0..=100.0, 0.0);
                    }
                }
                photographic_color_controls(ui,&mut controls.color.photographic);
                adjustment.color = controls.color;
            });
            ui.collapsing(crate::i18n::text("Corrective"),|ui| {
                param_slider(ui,"Red-eye",&mut adjustment.corrective.red_eye,0.0..=100.0,0.0);
                param_slider(ui,"Color moiré",&mut adjustment.corrective.moire,0.0..=100.0,0.0);
                param_slider(ui,"Moiré radius",&mut adjustment.corrective.moire_radius_px,1.0..=16.0,4.0);
                ui.label(egui::RichText::new(crate::i18n::text("Red-eye preserves neutral catchlights. Moiré softens false color while retaining brightness detail.")).small().color(theme::TEXT_SECONDARY));
            });
            ui.collapsing(crate::i18n::text("Texture & detail"), |ui| {
                param_slider(
                    ui,
                    "Clarity",
                    &mut adjustment.local.clarity,
                    -100.0..=100.0,
                    0.0,
                );
                param_slider(
                    ui,
                    "Texture",
                    &mut adjustment.local.texture,
                    -100.0..=100.0,
                    0.0,
                );
                param_slider(
                    ui,
                    "Dehaze",
                    &mut adjustment.local.dehaze,
                    -100.0..=100.0,
                    0.0,
                );
                param_slider(ui, "Sharpness", &mut adjustment.sharpness, 0.0..=100.0, 0.0);
                param_slider(
                    ui,
                    "Noise reduction",
                    &mut adjustment.noise_reduction,
                    0.0..=100.0,
                    0.0,
                );
            });
        });
        if ui
            .small_button(crate::i18n::text("Delete local adjustment"))
            .clicked()
        {
            delete_local = Some(index);
        }
    }
    if let Some((index, other, operation)) = combine_local {
        let left = edit.local_adjustments[index].coverage_mask();
        let right = edit.local_adjustments[other].coverage_mask();
        let mut mask = LocalMask::Composite {
            operation,
            left: Box::new(left),
            right: Box::new(right),
        };
        if mask.clamp_valid() {
            let adjustment = &mut edit.local_adjustments[index];
            adjustment.mask = mask;
            adjustment.mask_inverted = false;
            adjustment.opacity = 1.0;
            *tool = RetouchTool::None;
            *color_range_picker = None;
        }
    }
    if let Some(index) = duplicate_local {
        let mut copy = edit.local_adjustments[index].clone();
        copy.name = {
            let __l10n_args = (&(copy.name),);
            crate::i18n::format("{0} copy", std::slice::from_ref(__l10n_args.0))
        };
        edit.local_adjustments.insert(index + 1, copy);
        *active_local = Some(index + 1);
        *color_range_picker = None;
        *tool = RetouchTool::None;
    }
    if let Some((from, to)) = move_local {
        edit.local_adjustments.swap(from, to);
        *active_local = Some(to);
        *color_range_picker = None;
        *tool = RetouchTool::None;
    }
    if let Some(index) = delete_local {
        edit.local_adjustments.remove(index);
        if color_range_picker.is_some_and(|picker| picker == index) {
            *color_range_picker = None;
        } else if let Some(picker) = color_range_picker.as_mut() {
            if *picker > index {
                *picker -= 1;
            }
        }
        *active_local = None;
        *tool = RetouchTool::None;
    }
}

fn spot_corrections_panel(
    ui: &mut egui::Ui,
    edit: &mut sp_core::EditState,
    tool: &mut RetouchTool,
    radius: &mut f32,
    feather: &mut f32,
    spot_ui: &mut crate::state::SpotUi,
) {
    ui.separator();
    sub_heading(ui, &{
        let __l10n_args = (&(edit.spots.len()),);
        crate::i18n::format(
            "Heal & clone · {0} corrections",
            &[__l10n_args.0.to_string()],
        )
    });
    ui.horizontal(|ui| {
        ui.selectable_value(tool, RetouchTool::Heal, crate::i18n::text("Heal"));
        ui.selectable_value(tool, RetouchTool::Clone, crate::i18n::text("Clone"));
        if ui.button(crate::i18n::text("Done")).clicked() {
            *tool = RetouchTool::None;
        }
        if ui.button(crate::i18n::text("New correction")).clicked() {
            spot_ui.selected = None;
            if *tool == RetouchTool::None {
                *tool = RetouchTool::Heal;
            }
        }
    });
    param_slider(ui, "Spot size", radius, 0.002..=0.25, 0.03);
    param_slider(ui, "Spot feather", feather, 0.0..=1.0, 0.5);
    param_slider(ui, "Spot opacity", &mut spot_ui.opacity, 0.0..=1.0, 1.0);
    if *tool != RetouchTool::Clone {
        param_slider(
            ui,
            "Match texture",
            &mut spot_ui.heal_texture,
            0.0..=100.0,
            100.0,
        );
    }
    ui.horizontal(|ui| {
        if ui
            .selectable_label(
                spot_ui.pick_source,
                crate::i18n::text("Pick sample source…"),
            )
            .clicked()
        {
            spot_ui.pick_source = !spot_ui.pick_source;
            if *tool == RetouchTool::None {
                *tool = RetouchTool::Clone;
            }
        }
        if ui
            .small_button(crate::i18n::text("Reset sample offset"))
            .clicked()
        {
            spot_ui.source = None;
            spot_ui.offset = None;
            spot_ui.pick_source = false;
        }
    });
    ui.checkbox(&mut spot_ui.aligned, crate::i18n::text("Aligned sampling"))
        .on_hover_text(crate::i18n::text("Keep the same sample offset across new strokes. Turn off to start each stroke at the chosen source."));
    if spot_ui.selected.is_some_and(|i| i >= edit.spots.len()) {
        spot_ui.selected = None;
    }
    egui::ScrollArea::vertical()
        .id_salt("spot-corrections")
        .max_height(140.0)
        .show(ui, |ui| {
            for (index, spot) in edit.spots.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    ui.checkbox(&mut spot.enabled, crate::i18n::text(""));
                    if ui
                        .selectable_label(
                            spot_ui.selected == Some(index),
                            format!(
                                "{} {}",
                                if spot.clone { "Clone" } else { "Heal" },
                                index + 1
                            ),
                        )
                        .clicked()
                    {
                        spot_ui.selected = Some(index);
                        *tool = if spot.clone {
                            RetouchTool::Clone
                        } else {
                            RetouchTool::Heal
                        };
                    }
                });
            }
        });
    if let Some(index) = spot_ui.selected {
        let spot = &mut edit.spots[index];
        ui.push_id(index, |ui| {
            param_slider(ui, "Correction size", &mut spot.radius, 0.002..=0.25, 0.03);
            param_slider(ui, "Correction feather", &mut spot.feather, 0.0..=1.0, 0.5);
            param_slider(ui, "Correction opacity", &mut spot.opacity, 0.0..=1.0, 1.0);
            ui.checkbox(&mut spot.clone, crate::i18n::text("Clone source pixels"));
            if !spot.clone {
                param_slider(
                    ui,
                    "Match texture",
                    &mut spot.heal_texture,
                    0.0..=100.0,
                    100.0,
                );
            }
            ui.horizontal(|ui| {
                ui.label(crate::i18n::text("Source X / Y"));
                ui.add(
                    egui::DragValue::new(&mut spot.source[0])
                        .range(0.0..=1.0)
                        .speed(0.001),
                );
                ui.add(
                    egui::DragValue::new(&mut spot.source[1])
                        .range(0.0..=1.0)
                        .speed(0.001),
                );
            });
        });
        if ui.button(crate::i18n::text("Find matching source")).on_hover_text(crate::i18n::text("Suggest a source with similar surrounding texture and color. Review the source handle and move it if needed.")).clicked() {
            spot_ui.source_request=Some(index);
        }
        if ui.button(crate::i18n::text("Delete correction")).clicked() {
            edit.spots.remove(index);
            spot_ui.selected = None;
            spot_ui.drag_handle = None;
        }
    }
    dust_review_panel(ui, spot_ui);
    ui.label(
        egui::RichText::new(
            crate::i18n::text("Click or paint to correct. Option/Alt-click sets the source. Drag a source handle to resample; drag a target handle to move a correction."),
        )
        .small()
        .color(theme::TEXT_FAINT),
    );
}

fn dust_review_panel(ui: &mut egui::Ui, spot_ui: &mut crate::state::SpotUi) {
    ui.collapsing(crate::i18n::text("Find possible dust"), |ui| {
        param_slider(ui,"Sensitivity",&mut spot_ui.dust_sensitivity,0.0..=1.0,0.5);
        if ui.button(crate::i18n::text("Scan smooth areas")).clicked() { spot_ui.detect_dust=true; }
        ui.label(egui::RichText::new(crate::i18n::text("Review each proposal. Small scene details can resemble dust; tiny defects may require a manual correction at 100%.")).small().color(theme::TEXT_FAINT));
        let mut dismiss=None;
        egui::ScrollArea::vertical().id_salt("dust-review").max_height(180.0).show(ui,|ui|{
            for index in 0..spot_ui.dust_candidates.len() {ui.horizontal(|ui|{
                if ui.selectable_label(spot_ui.dust_selected==Some(index),{ let __l10n_args = (&(index+1),); crate::i18n::format("Candidate {0}", &[__l10n_args.0.to_string()]) }).clicked() {spot_ui.dust_selected=Some(index);}
                if ui.small_button(crate::i18n::text("Heal")).clicked() {spot_ui.dust_apply=Some(index);}
                if ui.small_button(crate::i18n::text("Dismiss")).clicked() {dismiss=Some(index);}
            });}
        });
        if let Some(index)=dismiss {spot_ui.dust_candidates.remove(index);spot_ui.dust_selected=None;}
        if !spot_ui.dust_candidates.is_empty() && ui.small_button(crate::i18n::text("Clear proposals")).clicked() {spot_ui.dust_candidates.clear();spot_ui.dust_selected=None;}
    });
}

fn brush_settings_editor(ui: &mut egui::Ui, settings: &mut sp_core::mask::BrushSettings) {
    param_slider(ui, "Brush size", &mut settings.size, 0.0001..=0.25, 0.03);
    param_slider(ui, "Feather", &mut settings.feather, 0.0..=1.0, 0.5);
    param_slider(ui, "Flow", &mut settings.flow, 0.01..=1.0, 1.0);
    param_slider(ui, "Spacing", &mut settings.spacing, 0.01..=1.0, 0.15);
    param_slider(ui, "Smoothing", &mut settings.smoothing, 0.0..=0.9, 0.15);
    ui.checkbox(
        &mut settings.pressure,
        crate::i18n::text("Use pressure for size"),
    )
    .on_hover_text(crate::i18n::text(
        "Uses pressure when your pen or touch device supplies it. A mouse keeps a constant size.",
    ));
}

fn photographic_color_controls(
    ui: &mut egui::Ui,
    params: &mut sp_core::correction::PhotographicColor,
) {
    ui.collapsing(crate::i18n::text("Channel mixer"), |ui| {
        for (row, name) in ["Red output", "Green output", "Blue output"]
            .into_iter()
            .enumerate()
        {
            ui.push_id(row, |ui| {
                sub_heading(ui, name);
                for (column, label) in ["Red", "Green", "Blue"].into_iter().enumerate() {
                    let mut percent = params.mixer[row][column] * 100.0;
                    param_slider(
                        ui,
                        label,
                        &mut percent,
                        -200.0..=200.0,
                        if row == column { 100.0 } else { 0.0 },
                    );
                    params.mixer[row][column] = percent * 0.01;
                }
            });
        }
        if ui
            .small_button(crate::i18n::text("Reset channel mixer"))
            .clicked()
        {
            params.mixer = Default::default();
            params.mixer[0][0] = 1.0;
            params.mixer[1][1] = 1.0;
            params.mixer[2][2] = 1.0;
        }
    });
    ui.collapsing(crate::i18n::text("Selective color"), |ui| {
        let names = [
            "Reds", "Yellows", "Greens", "Cyans", "Blues", "Magentas", "Whites", "Neutrals",
            "Blacks",
        ];
        let id = ui.id().with("selective_range");
        let mut range = ui
            .data_mut(|data| data.get_temp::<usize>(id))
            .unwrap_or(0)
            .min(8);
        egui::ComboBox::from_id_salt("selective_range")
            .selected_text(names[range])
            .show_ui(ui, |ui| {
                for (index, label) in names.iter().enumerate() {
                    ui.selectable_value(&mut range, index, crate::i18n::text(label));
                }
            });
        ui.data_mut(|data| data.insert_temp(id, range));
        ui.checkbox(
            &mut params.relative,
            crate::i18n::text("Relative adjustment"),
        );
        for (channel, label) in ["Cyan", "Magenta", "Yellow", "Black"]
            .into_iter()
            .enumerate()
        {
            param_slider(
                ui,
                label,
                &mut params.selective[range][channel],
                -100.0..=100.0,
                0.0,
            );
        }
    });
    ui.collapsing(crate::i18n::text("Skin color uniformity"),|ui| {
        param_slider(ui,"Target hue",&mut params.skin.hue_deg,0.0..=360.0,55.0);
        param_slider(ui,"Hue range",&mut params.skin.range_deg,2.0..=90.0,30.0);
        param_slider(ui,"Hue uniformity",&mut params.skin.hue_uniformity,0.0..=100.0,0.0);
        param_slider(ui,"Chroma uniformity",&mut params.skin.chroma_uniformity,0.0..=100.0,0.0);
        param_slider(ui,"Target chroma",&mut params.skin.target_chroma,0.0..=0.4,0.12);
        ui.label(egui::RichText::new(crate::i18n::text("Targets a hue range while preserving lightness and texture. Use a local mask to isolate skin from similar background colors.")).small().color(theme::TEXT_SECONDARY));
    });
}

fn local_preset_controls(ui: &mut egui::Ui, adjustment: &mut sp_core::edit::LocalAdjustment) {
    use sp_core::mask::LocalPreset;
    let status_id = ui.id().with("local_preset_status");
    let mut status = None;
    ui.horizontal_wrapped(|ui| {
        ui.menu_button(crate::i18n::text("Adjustment presets"), |ui| {
            for label in [
                "Brighten",
                "Darken",
                "Soft texture",
                "Crisp detail",
                "Dehaze",
                "Neutral",
            ] {
                if ui.button(crate::i18n::text(label)).clicked() {
                    let mut recipe = sp_core::edit::LocalAdjustment::default();
                    match label {
                        "Brighten" => recipe.tone.exposure_ev = 0.35,
                        "Darken" => recipe.tone.exposure_ev = -0.35,
                        "Soft texture" => {
                            recipe.local.texture = -35.0;
                            recipe.local.clarity = -10.0;
                        }
                        "Crisp detail" => {
                            recipe.local.texture = 20.0;
                            recipe.sharpness = 20.0;
                        }
                        "Dehaze" => recipe.local.dehaze = 25.0,
                        _ => {}
                    }
                    LocalPreset::from_adjustment(label, &recipe).apply(adjustment);
                    ui.close();
                }
            }
        });
        if ui
            .small_button(crate::i18n::text("Save preset…"))
            .on_hover_text(crate::i18n::text(
                "Save these adjustments; the current mask stays with this photo.",
            ))
            .clicked()
        {
            if let Some(path) = rfd::FileDialog::new()
                .add_filter(crate::i18n::text("Local adjustment preset"), &["splocal"])
                .set_file_name("local-adjustment.splocal")
                .save_file()
            {
                let preset = LocalPreset::from_adjustment(&adjustment.name, adjustment);
                status = Some(
                    match serde_json::to_vec_pretty(&preset)
                        .map_err(|error| error.to_string())
                        .and_then(|bytes| {
                            std::fs::write(path, bytes).map_err(|error| error.to_string())
                        }) {
                        Ok(()) => "Preset saved".to_owned(),
                        Err(error) => crate::i18n::format(
                            "Could not save preset: {0}",
                            std::slice::from_ref(&error),
                        ),
                    },
                );
            }
        }
        if ui
            .small_button(crate::i18n::text("Load preset…"))
            .on_hover_text(crate::i18n::text(
                "Apply saved adjustments to this selection.",
            ))
            .clicked()
        {
            if let Some(path) = rfd::FileDialog::new()
                .add_filter(crate::i18n::text("Local adjustment preset"), &["splocal"])
                .pick_file()
            {
                let result = (|| -> Result<LocalPreset, String> {
                    let metadata = std::fs::metadata(&path).map_err(|error| error.to_string())?;
                    if metadata.len() > 1_048_576 {
                        return Err("Preset is larger than 1 MiB".into());
                    }
                    serde_json::from_slice(&std::fs::read(path).map_err(|error| error.to_string())?)
                        .map_err(|error| error.to_string())
                })();
                status = Some(match result {
                    Ok(preset) if preset.apply(adjustment) => {
                        let __l10n_args = (&(preset.name),);
                        crate::i18n::format("Loaded {0}", std::slice::from_ref(__l10n_args.0))
                    }
                    Ok(_) => "This preset needs a newer ShopPhoto version".into(),
                    Err(error) => crate::i18n::format(
                        "Could not load preset: {0}",
                        std::slice::from_ref(&error),
                    ),
                });
            }
        }
    });
    if let Some(status) = status {
        ui.data_mut(|data| data.insert_temp(status_id, status));
    }
    if let Some(status) = ui.data_mut(|data| data.get_temp::<String>(status_id)) {
        ui.label(egui::RichText::new(crate::i18n::text(&status)).small());
    }
}

/// Inspect and refine copied operands without destructively flattening the mask.
fn composed_mask_editor(ui: &mut egui::Ui, mask: &mut sp_core::edit::LocalMask, depth: usize) {
    use sp_core::edit::{LocalMask, MaskOperation};
    if depth >= 32 {
        return;
    }
    match mask {
        LocalMask::Paint { settings, .. } => {
            brush_settings_editor(ui, settings);
        }
        LocalMask::Composite {
            operation,
            left,
            right,
        } => {
            ui.horizontal(|ui| {
                ui.selectable_value(operation, MaskOperation::Add, crate::i18n::text("Add"));
                ui.selectable_value(
                    operation,
                    MaskOperation::Subtract,
                    crate::i18n::text("Subtract"),
                );
                ui.selectable_value(
                    operation,
                    MaskOperation::Intersect,
                    crate::i18n::text("Intersect"),
                );
            });
            ui.collapsing(crate::i18n::text("Base coverage"), |ui| {
                composed_mask_editor(ui, left, depth + 1)
            });
            ui.collapsing(crate::i18n::text("Combined coverage"), |ui| {
                composed_mask_editor(ui, right, depth + 1)
            });
        }
        LocalMask::Modified {
            mask,
            inverted,
            opacity,
        } => {
            ui.checkbox(inverted, crate::i18n::text("Invert component"));
            param_slider(ui, "Component coverage", opacity, 0.0..=1.0, 1.0);
            composed_mask_editor(ui, mask, depth + 1);
        }
        LocalMask::Radial {
            center,
            radius,
            feather,
        } => {
            param_slider(ui, "Center X", &mut center[0], 0.0..=1.0, 0.5);
            param_slider(ui, "Center Y", &mut center[1], 0.0..=1.0, 0.5);
            param_slider(ui, "Width", &mut radius[0], 0.001..=1.0, 0.15);
            param_slider(ui, "Height", &mut radius[1], 0.001..=1.0, 0.15);
            param_slider(ui, "Feather", feather, 0.0..=1.0, 0.5);
        }
        LocalMask::LinearGradient { start, end } => {
            param_slider(ui, "Start X", &mut start[0], 0.0..=1.0, 0.25);
            param_slider(ui, "Start Y", &mut start[1], 0.0..=1.0, 0.5);
            param_slider(ui, "End X", &mut end[0], 0.0..=1.0, 0.75);
            param_slider(ui, "End Y", &mut end[1], 0.0..=1.0, 0.5);
        }
        LocalMask::LuminanceRange {
            minimum,
            maximum,
            feather,
        } => {
            param_slider(ui, "Minimum", minimum, 0.0..=1.0, 0.25);
            param_slider(ui, "Maximum", maximum, 0.0..=1.0, 0.75);
            if *minimum > *maximum {
                std::mem::swap(minimum, maximum);
            }
            param_slider(ui, "Feather", feather, 0.0..=0.5, 0.1);
        }
        LocalMask::ColorRange {
            target_ab,
            radius,
            feather,
        } => {
            param_slider(ui, "Color a", &mut target_ab[0], -0.5..=0.5, 0.0);
            param_slider(ui, "Color b", &mut target_ab[1], -0.5..=0.5, 0.0);
            param_slider(ui, "Range", radius, 0.005..=0.3, 0.04);
            param_slider(ui, "Feather", feather, 0.0..=0.3, 0.04);
        }
        LocalMask::Brush { size, feather, .. } => {
            param_slider(ui, "Brush size", size, 0.0001..=0.25, 0.03);
            param_slider(ui, "Feather", feather, 0.0..=1.0, 0.5);
        }
        LocalMask::Alpha(alpha) => {
            ui.label(format!(
                "{} · {} × {}",
                alpha.source, alpha.width, alpha.height
            ));
            ui.checkbox(&mut alpha.inverted, crate::i18n::text("Invert alpha"));
        }
    }
}

// -------------------------------------------------------------- inspector

/// Pinned inspector header: full-height histogram, EXIF line, tool search
/// and the section jump bar. Stays visible while the cards scroll.
fn inspector_header(ui: &mut egui::Ui, state: &mut AppState) {
    ui.add_space(theme::SPACE);
    histogram_widget(ui, state);
    exif_line(ui, state);
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        // Room for the match count only while there is a query to count.
        let reserve = if state.inspector_query.trim().is_empty() {
            0.0
        } else {
            70.0
        };
        let search = ui.add(
            egui::TextEdit::singleline(&mut state.inspector_query)
                .hint_text(crate::i18n::text("Find a tool…  ⌘F"))
                .desired_width(ui.available_width() - reserve),
        );
        if ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::F)) {
            search.request_focus();
        }
        if search.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            state.inspector_query.clear();
        }
        let query = state.inspector_query.trim().to_lowercase();
        if !query.is_empty() {
            let hits: usize = CARD_INDEX
                .iter()
                .map(|(title, _)| {
                    let (title_match, hits) = card_matches(title, &query);
                    hits + usize::from(title_match)
                })
                .sum();
            ui.label(
                egui::RichText::new({
                    let __l10n_args = (&(if hits == 1 { "" } else { "es" }),);
                    crate::i18n::format(
                        "{0} match{1}",
                        &[hits.to_string(), __l10n_args.0.to_string()],
                    )
                })
                .size(10.5)
                .color(theme::TEXT_FAINT),
            );
        }
    });
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 3.0;
        for (index, name) in SECTIONS.iter().enumerate() {
            let current =
                state.inspector_query.trim().is_empty() && state.inspector_section == index;
            let resp = ui.add(
                egui::Button::new(
                    egui::RichText::new(crate::i18n::text(name))
                        .size(11.0)
                        .color(if current {
                            theme::TEXT_PRIMARY
                        } else {
                            theme::TEXT_SECONDARY
                        }),
                )
                .fill(if current {
                    theme::ACCENT.gamma_multiply(0.2)
                } else {
                    egui::Color32::TRANSPARENT
                })
                .stroke(egui::Stroke::new(
                    1.0,
                    if current {
                        theme::ACCENT.gamma_multiply(0.7)
                    } else {
                        egui::Color32::TRANSPARENT
                    },
                ))
                .corner_radius(10.0)
                .min_size(egui::vec2(0.0, 20.0)),
            );
            if resp.clicked() {
                state.inspector_query.clear();
                state.inspector_jump = Some(index);
            }
        }
    });
    ui.add_space(4.0);
    let rect = ui.max_rect();
    ui.painter().line_segment(
        [
            egui::pos2(rect.left(), ui.cursor().top()),
            egui::pos2(rect.right(), ui.cursor().top()),
        ],
        egui::Stroke::new(1.0, theme::HAIRLINE),
    );
}

fn exif_line(ui: &mut egui::Ui, state: &AppState) {
    let Some(meta) = &state.meta else {
        return;
    };
    let mut parts = Vec::new();
    if let Some(iso) = meta.iso {
        parts.push(format!("ISO {iso}"));
    }
    if let Some(s) = meta.shutter_s.filter(|s| *s > 0.0) {
        parts.push(if s >= 1.0 {
            format!("{s:.1} s")
        } else {
            format!("1/{:.0} s", 1.0 / s)
        });
    }
    if let Some(f) = meta.aperture_f {
        parts.push(format!("f/{f:.1}"));
    }
    if let Some(mm) = meta.focal_length_mm {
        parts.push(format!("{mm:.0} mm"));
    }
    if parts.is_empty() {
        return;
    }
    ui.add_space(2.0);
    ui.label(
        egui::RichText::new(parts.join("   "))
            .size(11.0)
            .color(theme::TEXT_SECONDARY),
    );
}

fn panel_flag<'a>(open: &'a mut crate::state::PanelOpen, title: &str) -> Option<&'a mut bool> {
    Some(match title {
        "History" => &mut open.history,
        "Basic" => &mut open.basic,
        "Tone Curve" => &mut open.curve,
        "Color" => &mut open.color,
        "Local & retouch" => &mut open.local_retouch,
        "Detail" => &mut open.detail,
        "Lens Corrections" => &mut open.lens,
        "Effects" => &mut open.effects,
        "Resolution" => &mut open.resolution,
        "Geometry" => &mut open.geometry,
        "HDR" => &mut open.hdr,
        "Metadata" => &mut open.metadata,
        _ => return None,
    })
}

const TEMP_GRADIENT: [egui::Color32; 3] = [
    egui::Color32::from_rgb(0x4B, 0x7B, 0xFF),
    egui::Color32::from_rgb(0xE9, 0xE3, 0xD4),
    egui::Color32::from_rgb(0xFF, 0x8A, 0x2A),
];
const TINT_GRADIENT: [egui::Color32; 3] = [
    egui::Color32::from_rgb(0x3F, 0xBF, 0x5A),
    egui::Color32::from_rgb(0xD8, 0xD0, 0xD8),
    egui::Color32::from_rgb(0xE0, 0x5A, 0xD0),
];

pub fn right_panel(
    ui: &mut egui::Ui,
    state: &mut AppState,
    loupe: Option<(egui::TextureId, sp_pipeline::renderer::TileInfo)>,
) {
    if state.mode == Mode::Library {
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add_space(theme::SPACE);
                metadata_panel(ui, state);
            });
        return;
    }
    inspector_header(ui, state);
    // Hover previews are re-established every frame by the hovered row.
    state.history_preview = None;

    let scroll = egui::ScrollArea::vertical()
        // Reserve the scrollbar gutter permanently. This keeps every card,
        // including the noise loupe preview, at the same horizontal position
        // as sections open/close or live edits repaint their contents.
        .auto_shrink([false, false])
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible);
    let mut cards = Cards {
        query: state.inspector_query.trim().to_owned(),
        bypass_mask: state.bypass_mask,
        solo: state.solo_panels,
        resets: Vec::new(),
        opened: None,
        jump: state.inspector_jump.take(),
        focus: state.inspector_focus_card.take(),
        section_tops: [None; SECTIONS.len()],
    };
    let mut clip_top = 0.0;
    scroll.show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        clip_top = ui.clip_rect().top();
        ui.add_space(theme::SPACE * 0.75);
        inspector_cards(ui, state, loupe, &mut cards);
    });

    // Scroll spy: the last section whose first card starts above the fold.
    let mut current = 0;
    for (index, top) in cards.section_tops.iter().enumerate() {
        if top.is_some_and(|top| top <= clip_top + 40.0) {
            current = index;
        }
    }
    state.inspector_section = current;
    state.bypass_mask = cards.bypass_mask;
    if let Some(opened) = cards.opened {
        for title in [
            "History",
            "Basic",
            "Tone Curve",
            "Color",
            "Local & retouch",
            "Detail",
            "Lens Corrections",
            "Effects",
            "Resolution",
            "Geometry",
            "HDR",
            "Metadata",
        ] {
            if title != opened {
                if let Some(flag) = panel_flag(&mut state.panels, title) {
                    *flag = false;
                }
            }
        }
    }
    for group in cards.resets {
        let mut edit = state.edit.clone();
        edit_groups::reset_group(&mut edit, group);
        state.apply_edit_step_labeled(
            edit,
            Some({
                let __l10n_args = (&(group.title()),);
                crate::i18n::format("Reset {0}", &[__l10n_args.0.to_string()])
            }),
        );
    }
}

fn inspector_cards(
    ui: &mut egui::Ui,
    state: &mut AppState,
    loupe: Option<(egui::TextureId, sp_pipeline::renderer::TileInfo)>,
    cards: &mut Cards,
) {
    if state.first_edit_tip {
        egui::Frame::new()
            .fill(theme::ACCENT.gamma_multiply(0.12))
            .stroke(egui::Stroke::new(1.0, theme::ACCENT))
            .corner_radius(theme::RADIUS_CONTROL)
            .inner_margin(theme::SPACE)
            .show(ui, |ui| {
                ui.label(egui::RichText::new(crate::i18n::text("Your first edit")).strong());
                ui.label(
                    egui::RichText::new(
                        crate::i18n::text("Start with Exposure and Highlights in Basic. Drag a slider's label to scrub it, double-click to reset, and press \\ for before/after."),
                    )
                    .small()
                    .color(theme::TEXT_SECONDARY),
                );
                if ui.small_button(crate::i18n::text("Got it")).clicked() {
                    state.first_edit_tip = false;
                }
            });
        ui.add_space(theme::SPACE);
    }

    let mut open = state.panels.clone();
    history_card(ui, state, cards, &mut open.history);

    // Working copies so we can split borrows cleanly.
    let mut curve_channel = state.curve_channel;
    let mut retouch_tool = state.retouch_tool;
    let mut retouch_active_local = state.retouch_active_local;
    let mut mask_show_overlay = state.mask_show_overlay;
    let mut retouch_radius = state.retouch_radius.max(0.002);
    let mut retouch_feather = state.retouch_feather.clamp(0.0, 1.0);
    let mut spot_ui = state.spot_ui.clone();
    let mut color_range_picker = state.color_range_picker;
    let mut curve_picker = state.curve_picker;
    let mut wb_picker = state.wb_picker;
    let mut film_base_picker = state.film_base_picker;
    let mut sample_picker = state.sample_picker;
    let mut scopes_enabled = state.scopes_enabled;
    let analysis_frame = state.analysis_frame.clone();
    let analysis_dirty = state.analysis_dirty;
    let mut ai_compare_original = state.ai_compare_original;
    let mut ai_compare_split = state.ai_compare_split;
    let mut ai_compare_position = state.ai_compare_position.clamp(0.05, 0.95);
    let mut advanced_open = state.advanced_open;
    let mut hsl_tab = state.hsl_tab;
    let mut crop_aspect = state.crop_aspect;
    let metadata_path = state.open_image.clone();
    let capture_metadata = state.meta.clone();
    let counts: Vec<usize> = Group::ALL
        .iter()
        .map(|g| edit_groups::modified_count(&state.edit, *g))
        .collect();
    let count = |g: Group| counts[g as usize];
    let source_is_raw = state.source_is_raw;
    let edit = &mut state.edit;

    let profile_names = state.profile_names.clone();
    let custom_look_names = state.custom_look_names.clone();
    let lens_names = state.lens_names.clone();
    let lens_catalog_names = state.lens_catalog_names.clone();
    let matched_lens = state.matched_lens.clone();
    let recorded_lens = state.meta.as_ref().and_then(|m| m.lens_model.clone());
    let shot_aperture = state.meta.as_ref().and_then(|m| m.aperture_f);
    // Every decoded RAW is 2×2 Bayer, which the ShopPhoto model accepts.
    let camera_denoise_available = source_is_raw && state.meta.is_some();
    let shot_iso = state.meta.as_ref().and_then(|meta| meta.iso);
    let ai_denoise_busy = state.ai_denoise_busy;
    let ai_denoise_available = state.ai_denoise_available;
    let ai_denoise_status = state.ai_denoise_status.clone();
    let ai_compare_available = !ai_denoise_busy
        && state
            .ai_denoise_status
            .as_deref()
            .is_some_and(|s| s.starts_with("AI RAW denoise applied"));
    let mut import_dcp = false;
    let mut import_cube = false;
    let mut open_reference_match = false;
    let auto_busy = state.auto_in_flight;
    let auto_available = state.open_image.is_some() && !auto_busy;
    let mut auto_tone = false;
    let mut auto_exposure = false;
    let mut auto_defringe = false;
    let mut auto_ca = false;
    let mut auto_level = false;
    let mut pick_purple_fringe = false;
    let mut pick_green_fringe = false;
    let mut look_preview = None;

    // ---------------------------------------------------------- Light
    tool_group(
        ui,
        cards,
        Some(Group::Basic),
        "Basic",
        Some(0),
        &mut open.basic,
        count(Group::Basic),
        |ui| {
            if source_is_raw {
                // Camera color profile (BUILD.md §3.4 tiers).
                use sp_core::edit::ProfileChoice;
                let current_label = match &edit.profile {
                    ProfileChoice::Standard => "ShopPhoto Standard".to_owned(),
                    ProfileChoice::Named(n) => {
                        if profile_names.contains(n) {
                            n.clone()
                        } else {
                            crate::i18n::format("⚠ {0} (missing)", std::slice::from_ref(n))
                        }
                    }
                };
                ui.horizontal(|ui| {
                    ui.add_sized(
                        [theme::SLIDER_LABEL_W, 18.0],
                        egui::Label::new(
                            egui::RichText::new(crate::i18n::text("Profile"))
                                .color(theme::TEXT_SECONDARY)
                                .size(11.5),
                        ),
                    );
                    egui::ComboBox::from_id_salt("profile-combo")
                        .width(ui.available_width() - theme::SPACE)
                        .selected_text(current_label)
                        .show_ui(ui, |ui| {
                            if ui
                                .selectable_label(
                                    edit.profile == ProfileChoice::Standard,
                                    crate::i18n::text("ShopPhoto Standard"),
                                )
                                .clicked()
                            {
                                edit.profile = ProfileChoice::Standard;
                            }
                            for name in &profile_names {
                                let selected = edit.profile == ProfileChoice::Named(name.clone());
                                if ui.selectable_label(selected, name).clicked() {
                                    edit.profile = ProfileChoice::Named(name.clone());
                                }
                            }
                            ui.separator();
                            if ui.button(crate::i18n::text("Import DCP…")).clicked() {
                                import_dcp = true;
                                ui.close();
                            }
                            if ui
                                .button(crate::i18n::text("Match from reference TIFFs…"))
                                .clicked()
                            {
                                open_reference_match = true;
                                ui.close();
                            }
                        });
                });
            } else {
                ui.label(
                    egui::RichText::new(crate::i18n::text(
                        "Rendered image · embedded color / sRGB",
                    ))
                    .size(11.0)
                    .color(theme::TEXT_SECONDARY),
                );
            }
            ui.add_space(theme::SPACE * 0.5);

            ui.horizontal(|ui| {
                let auto = ui
                    .add_enabled_ui(auto_available, |ui| {
                        ui.add_sized(
                            [118.0, 28.0],
                            egui::Button::new(
                                egui::RichText::new(crate::i18n::text(if auto_busy {
                                    "Analyzing…"
                                } else {
                                    "Auto balance"
                                }))
                                .size(11.5)
                                .strong()
                                .color(theme::ON_ACCENT),
                            )
                            .fill(theme::ACCENT)
                            .corner_radius(theme::RADIUS_CONTROL),
                        )
                    })
                    .inner;
                if auto.clicked() {
                    auto_tone = true;
                }
                auto.on_hover_text(crate::i18n::text(
                    "Conservatively balances exposure, shadows and highlights from the image",
                ));
                let exposure = ui
                    .add_enabled_ui(auto_available, |ui| {
                        ui.add_sized(
                            [112.0, 28.0],
                            egui::Button::new(
                                egui::RichText::new(crate::i18n::text("Exposure only")).size(11.5),
                            )
                            .fill(theme::BG_RAISED)
                            .stroke(egui::Stroke::new(1.0, theme::HAIRLINE))
                            .corner_radius(theme::RADIUS_CONTROL),
                        )
                    })
                    .inner;
                if exposure.clicked() {
                    auto_exposure = true;
                }
                exposure.on_hover_text(crate::i18n::text(
                    "Adjust only exposure; keep every other tone control",
                ));
            });

            let look_label = edit.look.as_deref().unwrap_or("Standard").to_owned();
            ui.horizontal(|ui| {
                ui.add_sized(
                    [theme::SLIDER_LABEL_W, 18.0],
                    egui::Label::new(
                        egui::RichText::new(crate::i18n::text("Look"))
                            .color(theme::TEXT_SECONDARY)
                            .size(11.5),
                    ),
                );
                egui::ComboBox::from_id_salt("look-combo")
                    .selected_text(
                        if edit.look.as_deref().is_some_and(|name| {
                            sp_core::profile::Look::named(name).is_none()
                                && !custom_look_names.iter().any(|candidate| candidate == name)
                        }) {
                            crate::i18n::format(
                                "⚠ {0} (missing)",
                                std::slice::from_ref(&look_label),
                            )
                        } else {
                            look_label.clone()
                        },
                    )
                    .show_ui(ui, |ui| {
                        let standard =
                            ui.selectable_label(edit.look.is_none(), crate::i18n::text("Standard"));
                        if standard.hovered() {
                            look_preview = Some(None);
                        }
                        if standard.clicked() {
                            edit.look = None;
                        }
                        for name in sp_core::profile::Look::NAMES {
                            let response =
                                ui.selectable_label(edit.look.as_deref() == Some(name), name);
                            if response.hovered() {
                                look_preview = Some(Some(name.to_owned()));
                            }
                            if response.clicked() {
                                edit.look = Some(name.to_owned());
                                edit.look_strength = None;
                            }
                        }
                        if !custom_look_names.is_empty() {
                            ui.separator();
                            for name in &custom_look_names {
                                let response =
                                    ui.selectable_label(edit.look.as_deref() == Some(name), name);
                                if response.clicked() {
                                    edit.look = Some(name.clone());
                                    edit.look_strength = Some(100.0);
                                }
                            }
                        }
                        ui.separator();
                        if ui.button(crate::i18n::text("Import .cube…")).clicked() {
                            import_cube = true;
                        }
                    });
            });
            if edit
                .look
                .as_ref()
                .is_some_and(|name| custom_look_names.contains(name))
            {
                let strength = edit.look_strength.get_or_insert(100.0);
                param_slider_spec(
                    ui,
                    "Look strength",
                    strength,
                    0.0..=100.0,
                    100.0,
                    SliderSpec::unit(Unit::Percent),
                );
            }
            ui.add_space(theme::SPACE * 0.5);

            ui.collapsing(crate::i18n::text("Film negative"), |ui| {
                ui.checkbox(
                    &mut edit.negative.enabled,
                    crate::i18n::text("Convert negative to positive"),
                );
                if edit.negative.enabled {
                    if ui
                        .selectable_label(
                            film_base_picker,
                            crate::i18n::text("Sample clear film border…"),
                        )
                        .clicked()
                    {
                        film_base_picker = !film_base_picker;
                        sample_picker = false;
                        wb_picker = false;
                        retouch_tool = RetouchTool::None;
                    }
                    for (channel, name) in ["Base red", "Base green", "Base blue"]
                        .into_iter()
                        .enumerate()
                    {
                        param_slider(
                            ui,
                            name,
                            &mut edit.negative.base.0[channel],
                            0.0001..=4.0,
                            1.0,
                        );
                    }
                    param_slider(
                        ui,
                        "Density range",
                        &mut edit.negative.density_range,
                        0.1..=4.0,
                        2.0,
                    );
                    param_slider(
                        ui,
                        "Black density",
                        &mut edit.negative.black_density,
                        -1.0..=1.0,
                        0.0,
                    );
                    param_slider(ui, "Film gamma", &mut edit.negative.gamma, 0.2..=5.0, 1.0);
                }
            });
            // WB: `None` = as-shot; engaging the slider switches to explicit.
            // Temperature uses a log scale so the daylight band gets room.
            let mut temp = edit.wb.temp_kelvin.unwrap_or(5500.0);
            if param_slider_spec(
                ui,
                "Temp",
                &mut temp,
                2000.0..=50000.0,
                5500.0,
                SliderSpec {
                    unit: Unit::Kelvin,
                    log: true,
                    gradient: Some(TEMP_GRADIENT),
                },
            ) {
                edit.wb.temp_kelvin = Some(temp);
            }
            param_slider_spec(
                ui,
                "Tint",
                &mut edit.wb.tint,
                -150.0..=150.0,
                0.0,
                SliderSpec {
                    unit: Unit::Signed(0),
                    log: false,
                    gradient: Some(TINT_GRADIENT),
                },
            );
            if search_state(ui).is_none() {
                ui.horizontal(|ui| {
                let label = if wb_picker { "Cancel picker" } else { "Pick neutral…" };
                if ui
                    .add_enabled(auto_available || wb_picker, egui::Button::new(crate::i18n::text(label)).small())
                    .on_hover_text(crate::i18n::text("Click something in the photo that should be neutral grey; Temp and Tint are set to make it neutral"))
                    .clicked()
                {
                    wb_picker = !wb_picker;
                }
                // Either a way back to the camera's white balance, or a quiet
                // note that it is already in use — not both at once.
                if edit.wb.temp_kelvin.is_some() {
                    if ui
                        .add(egui::Button::new(crate::i18n::text(if source_is_raw { "Reset to as shot" } else { "Reset to original" })).small())
                        .on_hover_text(crate::i18n::text("Return to the camera's recorded white balance"))
                        .clicked()
                    {
                        edit.wb = Default::default();
                    }
                } else {
                    ui.label(egui::RichText::new(crate::i18n::text(if source_is_raw { "Using camera white balance" } else { "Using original color" })).size(10.5).color(theme::TEXT_FAINT));
                }
            });
            }
            ui.add_space(theme::SPACE * 0.5);
            param_slider_spec(
                ui,
                "Exposure",
                &mut edit.tone.exposure_ev,
                -5.0..=5.0,
                0.0,
                SliderSpec::unit(Unit::Ev),
            );
            if !auto_busy
                && ui
                    .ctx()
                    .data_mut(|data| {
                        data.remove_temp::<bool>(egui::Id::new("auto-exposure-request"))
                    })
                    .unwrap_or(false)
            {
                auto_exposure = true;
            }
            param_slider(
                ui,
                "Brightness",
                &mut edit.tone.brightness,
                -100.0..=100.0,
                0.0,
            );
            param_slider(ui, "Contrast", &mut edit.tone.contrast, -100.0..=100.0, 0.0);
            param_slider(
                ui,
                "Highlights",
                &mut edit.tone.highlights,
                -100.0..=100.0,
                0.0,
            );
            param_slider(ui, "Shadows", &mut edit.tone.shadows, -100.0..=100.0, 0.0);
            param_slider(ui, "Whites", &mut edit.tone.whites, -100.0..=100.0, 0.0);
            param_slider(ui, "Blacks", &mut edit.tone.blacks, -100.0..=100.0, 0.0);
            advanced(ui, &mut advanced_open, ADV_BASIC, 3, |ui| {
                ui.add_enabled(
                    source_is_raw,
                    egui::Checkbox::new(
                        &mut edit.highlight_recovery.enabled,
                        crate::i18n::text("Recover clipped RAW highlights"),
                    ),
                );
                ui.add_enabled_ui(source_is_raw && edit.highlight_recovery.enabled, |ui| {
                    param_slider_spec(
                        ui,
                        "Recovery",
                        &mut edit.highlight_recovery.strength,
                        0.0..=100.0,
                        100.0,
                        SliderSpec::unit(Unit::Percent),
                    );
                    let mut radius = f32::from(edit.highlight_recovery.radius);
                    if param_slider_spec(
                        ui,
                        "Propagation",
                        &mut radius,
                        1.0..=32.0,
                        2.0,
                        SliderSpec::unit(Unit::Plain(0)),
                    ) {
                        edit.highlight_recovery.radius = radius.round().clamp(1.0, 32.0) as u8;
                    }
                });
            });
        },
    );

    tool_group(
        ui,
        cards,
        Some(Group::Curve),
        "Tone Curve",
        Some(0),
        &mut open.curve,
        count(Group::Curve),
        |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(
                    &mut curve_channel,
                    CurveChannel::Luma,
                    crate::i18n::text("Y"),
                );
                ui.selectable_value(
                    &mut curve_channel,
                    CurveChannel::Red,
                    crate::i18n::text("R"),
                );
                ui.selectable_value(
                    &mut curve_channel,
                    CurveChannel::Green,
                    crate::i18n::text("G"),
                );
                ui.selectable_value(
                    &mut curve_channel,
                    CurveChannel::Blue,
                    crate::i18n::text("B"),
                );
            });
            let (curve, color) = match curve_channel {
                CurveChannel::Luma => (&mut edit.curve.luma, theme::TEXT_PRIMARY),
                CurveChannel::Red => (&mut edit.curve.red, theme::LABEL_RED),
                CurveChannel::Green => (&mut edit.curve.green, theme::LABEL_GREEN),
                CurveChannel::Blue => (&mut edit.curve.blue, theme::LABEL_BLUE),
            };
            curve_editor(ui, curve, color);
            let pipette_label = if curve_picker {
                "Cancel image pipette"
            } else {
                "Adjust from image"
            };
            if ui.button(pipette_label).clicked() {
                curve_picker = !curve_picker;
                if curve_picker {
                    color_range_picker = None;
                }
            }
            ui.add_space(theme::SPACE * 0.5);
            sub_heading(ui, "Parametric");
            param_slider(
                ui,
                "Highlights",
                &mut edit.curve.parametric.highlights,
                -100.0..=100.0,
                0.0,
            );
            param_slider(
                ui,
                "Lights",
                &mut edit.curve.parametric.lights,
                -100.0..=100.0,
                0.0,
            );
            param_slider(
                ui,
                "Darks",
                &mut edit.curve.parametric.darks,
                -100.0..=100.0,
                0.0,
            );
            param_slider(
                ui,
                "Shadows",
                &mut edit.curve.parametric.shadows,
                -100.0..=100.0,
                0.0,
            );
        },
    );

    // ---------------------------------------------------------- Color
    let mut hsl_selected = state.hsl_selected;
    tool_group(
        ui,
        cards,
        Some(Group::Color),
        "Color",
        Some(1),
        &mut open.color,
        count(Group::Color),
        |ui| {
            param_slider(
                ui,
                "Vibrance",
                &mut edit.color.vibrance,
                -100.0..=100.0,
                0.0,
            );
            param_slider(
                ui,
                "Saturation",
                &mut edit.color.saturation,
                -100.0..=100.0,
                0.0,
            );
            ui.add_space(theme::SPACE * 0.5);
            super::scopes::controls(
                ui,
                edit,
                &mut sample_picker,
                &mut scopes_enabled,
                analysis_frame.as_deref(),
                analysis_dirty,
            );
            if sample_picker {
                retouch_tool = RetouchTool::None;
                wb_picker = false;
                film_base_picker = false;
            }
            color_mixer(ui, edit, &mut hsl_tab);
            photographic_color_controls(ui, &mut edit.color.photographic);
            ui.add_space(theme::SPACE * 0.5);
            ui.separator();
            grading_wheels(ui, edit);
            param_slider(
                ui,
                "Balance",
                &mut edit.color.grading.balance,
                -100.0..=100.0,
                0.0,
            );
            ui.add_space(theme::SPACE * 0.5);
            ui.separator();
            ui.checkbox(&mut edit.color.monochrome, crate::i18n::text("Monochrome"));
            if edit.color.monochrome {
                monochrome_mixer(ui, edit, &mut hsl_selected);
            }
        },
    );

    tool_group(
        ui,
        cards,
        Some(Group::Local),
        "Local & retouch",
        Some(1),
        &mut open.local_retouch,
        count(Group::Local),
        |ui| {
            retouch_panel(
                ui,
                edit,
                &mut retouch_tool,
                &mut retouch_active_local,
                &mut retouch_radius,
                &mut retouch_feather,
                &mut color_range_picker,
                &mut mask_show_overlay,
                &mut spot_ui,
            );
            spot_corrections_panel(
                ui,
                edit,
                &mut retouch_tool,
                &mut retouch_radius,
                &mut retouch_feather,
                &mut spot_ui,
            );
        },
    );

    // ---------------------------------------------------------- Detail
    tool_group(
        ui,
        cards,
        Some(Group::Detail),
        "Detail",
        Some(2),
        &mut open.detail,
        count(Group::Detail),
        |ui| {
            ui.add_enabled(source_is_raw, egui::Checkbox::new(&mut edit.high_detail_demosaic, crate::i18n::text("High-detail demosaic (AHD)")))
            .on_hover_text(
                crate::i18n::text("Reduces zippering and color moiré in the 1:1 view and exports; the fast fit preview is unchanged"),
            );
            ui.separator();
            sub_heading(ui, "Blur restoration");
            param_slider(
                ui,
                "Restore blur",
                &mut edit.sharpen.restore_blur,
                0.0..=100.0,
                0.0,
            );
            if edit.sharpen.restore_blur > 0.0 {
                param_slider_spec(
                    ui,
                    "Blur radius",
                    &mut edit.sharpen.restore_radius_px,
                    1.0..=2.0,
                    1.0,
                    SliderSpec::unit(Unit::Px),
                );
                param_slider(
                    ui,
                    "Noise guard",
                    &mut edit.sharpen.restore_regularization,
                    0.0..=100.0,
                    50.0,
                );
                ui.small(crate::i18n::text("Small Gaussian blur only. Inspect at 1:1 and compare before/after; strong motion blur cannot be recovered."));
            }
            sub_heading(ui, "Sharpening");
            // Master control: one slider that rewrites the five manual
            // sharpening controls below with a balanced recipe.
            let mut sharpness = edit.sharpen.sharpness;
            if param_slider_spec(
                ui,
                "Sharpness",
                &mut sharpness,
                0.0..=100.0,
                0.0,
                SliderSpec::unit(Unit::Plain(0)),
            ) {
                edit.sharpen.apply_sharpness(sharpness);
            }
            let follows = edit.sharpen.follows_sharpness();
            if search_state(ui).is_none() {
                ui.label(
                    egui::RichText::new(crate::i18n::text(if follows {
                        "Auto · Sharpness drives the manual controls under Advanced."
                    } else {
                        "Manual · the controls under Advanced were tweaked by hand."
                    }))
                    .size(10.0)
                    .color(if follows {
                        theme::TEXT_FAINT
                    } else {
                        theme::ACCENT
                    }),
                );
            }
            advanced(ui, &mut advanced_open, ADV_DETAIL, 8, |ui| {
                param_slider(ui, "Amount", &mut edit.sharpen.amount, 0.0..=150.0, 0.0);
                param_slider_spec(
                    ui,
                    "Radius",
                    &mut edit.sharpen.radius_px,
                    0.5..=3.0,
                    1.0,
                    SliderSpec::unit(Unit::Px),
                );
                param_slider(ui, "Detail", &mut edit.sharpen.detail, 0.0..=100.0, 25.0);
                param_slider(
                    ui,
                    "Masking",
                    &mut edit.sharpen.threshold,
                    0.0..=100.0,
                    25.0,
                );
                param_slider(
                    ui,
                    "Halo guard",
                    &mut edit.sharpen.halo_suppression,
                    0.0..=100.0,
                    50.0,
                );
                param_slider(
                    ui,
                    "Lum. detail",
                    &mut edit.nr.luminance_detail,
                    0.0..=100.0,
                    50.0,
                );
                param_slider(
                    ui,
                    "Chr. detail",
                    &mut edit.nr.chroma_detail,
                    0.0..=100.0,
                    50.0,
                );
                ui.add_enabled(
                    source_is_raw,
                    egui::Checkbox::new(
                        &mut edit.nr.hot_pixels,
                        crate::i18n::text("Hot-pixel removal"),
                    ),
                );
            });
            ui.add_space(theme::SPACE * 0.5);
            sub_heading(ui, "Noise Reduction");
            if camera_denoise_available {
                let heading = ui.label(
                    egui::RichText::new("ShopPhoto RAW Denoise")
                        .size(11.5)
                        .strong()
                        .color(theme::TEXT_PRIMARY),
                );
                heading.on_hover_text(
                crate::i18n::text("Runs ShopPhoto's own neural model (rawdenoise-lab r4_plain) on Bayer data before demosaic. macOS uses Core ML; Windows and Linux use portable ONNX inference."),
            );
                ui.add_enabled_ui(ai_denoise_available, |ui| {
                    ui.horizontal(|ui| {
                        use sp_core::edit::CameraDenoiseMode;
                        ui.selectable_value(
                            &mut edit.nr.camera_denoise_mode,
                            CameraDenoiseMode::Off,
                            crate::i18n::text("Off"),
                        )
                        .on_hover_text(crate::i18n::text(
                            "Use the manual luminance and chroma controls",
                        ));
                        ui.selectable_value(
                            &mut edit.nr.camera_denoise_mode,
                            CameraDenoiseMode::Cleanest,
                            crate::i18n::text("Cleanest"),
                        )
                        .on_hover_text(crate::i18n::text(
                            "Use the complete model result for maximum cleanup",
                        ));
                        ui.selectable_value(
                            &mut edit.nr.camera_denoise_mode,
                            CameraDenoiseMode::PreserveDetail,
                            crate::i18n::text("Preserve detail"),
                        )
                        .on_hover_text(crate::i18n::text(
                            "Keep 20–25% original micro-contrast for a cleaner, crisp result",
                        ));
                    });
                });
                let iso =
                    shot_iso.map_or_else(|| "ISO auto".to_owned(), |iso| format!("ISO {iso}"));
                ui.label(
                    egui::RichText::new(crate::i18n::format("Neural model r4_plain · {0}", &[iso]))
                        .size(10.5)
                        .color(theme::TEXT_FAINT),
                );
                ui.add_enabled_ui(ai_compare_available, |ui| {
                let before = ui.checkbox(&mut ai_compare_original, crate::i18n::text("Show before denoise"))
                    .on_hover_text(crate::i18n::text("Switch the whole viewport between untouched and denoised sensor data; every other edit and HDR setting remains active"));
                if before.changed() && ai_compare_original {
                    ai_compare_split = false;
                }
                let split = ui.checkbox(&mut ai_compare_split, crate::i18n::text("Split before / after"))
                    .on_hover_text(crate::i18n::text("Overlay both developed versions with a draggable divider; before is on the left and after is on the right"));
                if split.changed() && ai_compare_split {
                    ai_compare_original = false;
                }
                if ai_compare_split {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(crate::i18n::text("Before")).size(10.0).color(theme::TEXT_FAINT));
                        ui.add(
                            egui::Slider::new(&mut ai_compare_position, 0.05..=0.95)
                                .show_value(false),
                        );
                        ui.label(egui::RichText::new(crate::i18n::text("After")).size(10.0).color(theme::TEXT_FAINT));
                    });
                }
            });
                ui.label(
                    egui::RichText::new(crate::i18n::text(
                        &(ai_denoise_status.unwrap_or_else(|| {
                            if ai_denoise_available {
                                "Ready · local neural model".into()
                            } else {
                                "Models unavailable · GPU fallback".into()
                            }
                        })),
                    ))
                    .size(10.5)
                    .color(if ai_denoise_busy {
                        theme::ACCENT
                    } else {
                        theme::TEXT_FAINT
                    }),
                );
                if !ai_denoise_available {
                    if state.ai_model_install_busy {
                        ui.add(
                            egui::ProgressBar::new(state.ai_model_install_progress)
                                .text(crate::i18n::text("Downloading models…")),
                        );
                    } else if ui
                        .button(crate::i18n::text("Download ShopPhoto model…"))
                        .clicked()
                    {
                        state.ai_model_install_requested = true;
                    }
                }
                ui.add_space(theme::SPACE * 0.5);
            }
            let manual_enabled = !camera_denoise_available
                || edit.nr.camera_denoise_mode == sp_core::edit::CameraDenoiseMode::Off;
            ui.add_enabled_ui(manual_enabled, |ui| {
                param_slider(ui, "Luminance", &mut edit.nr.luminance, 0.0..=100.0, 0.0);
                param_slider(ui, "Chroma", &mut edit.nr.chroma, 0.0..=100.0, 0.0);
            });
            if search_state(ui).is_none() {
                ui.add_space(theme::SPACE * 0.5);
                noise_loupe(ui, loupe);
            }
        },
    );

    tool_group(
        ui,
        cards,
        Some(Group::Effects),
        "Effects",
        Some(2),
        &mut open.effects,
        count(Group::Effects),
        |ui| {
            param_slider(ui, "Texture", &mut edit.local.texture, -100.0..=100.0, 0.0);
            param_slider(ui, "Clarity", &mut edit.local.clarity, -100.0..=100.0, 0.0);
            param_slider(ui, "Dehaze", &mut edit.local.dehaze, -100.0..=100.0, 0.0);
            ui.add_space(theme::SPACE * 0.5);
            sub_heading(ui, "Vignette");
            param_slider(ui, "Amount", &mut edit.vignette.amount, -100.0..=100.0, 0.0);
            param_slider(
                ui,
                "Midpoint",
                &mut edit.vignette.midpoint,
                0.0..=100.0,
                50.0,
            );
            param_slider(
                ui,
                "Roundness",
                &mut edit.vignette.roundness,
                0.0..=100.0,
                50.0,
            );
            param_slider(ui, "Feather", &mut edit.vignette.feather, 0.0..=100.0, 50.0);
            ui.add_space(theme::SPACE);
            sub_heading(ui, "Film Grain");
            param_slider(ui, "Amount", &mut edit.grain.amount, 0.0..=100.0, 0.0);
            param_slider(ui, "Size", &mut edit.grain.size, 0.5..=4.0, 1.0);
            param_slider(
                ui,
                "Roughness",
                &mut edit.grain.roughness,
                0.0..=100.0,
                50.0,
            );
        },
    );

    // ---------------------------------------------------------- Optics
    tool_group(
        ui,
        cards,
        Some(Group::Lens),
        "Lens Corrections",
        Some(3),
        &mut open.lens,
        count(Group::Lens),
        |ui| {
            ui.checkbox(
                &mut edit.lens.enable,
                crate::i18n::text("Enable lens corrections"),
            );
            ui.add_enabled_ui(edit.lens.enable, |ui| {
            let identified_lens = edit
                .lens
                .lens_model_override
                .clone()
                .or_else(|| recorded_lens.clone());
            let active = edit
                .lens
                .profile
                .clone()
                .filter(|name| lens_names.contains(name))
                .or_else(|| matched_lens.clone());
            ui.add_enabled_ui(source_is_raw, |ui| {
            if search_state(ui).is_none() {
                ui.horizontal(|ui| {
                    ui.add_sized([theme::SLIDER_LABEL_W, 18.0], egui::Label::new(egui::RichText::new(crate::i18n::text("Profile")).size(11.5).color(theme::TEXT_SECONDARY)));
                    egui::ComboBox::from_id_salt("lens_profile")
                        .selected_text(active.clone().unwrap_or_else(|| "None".to_owned()))
                        .show_ui(ui, |ui| {
                            let auto = matched_lens.as_deref().map_or_else(
                                || "Auto (no match)".to_owned(),
                                |name| crate::i18n::format("Auto ({0})", &[name.to_string()]),
                            );
                            if ui.selectable_label(edit.lens.profile.is_none(), auto).clicked() {
                                edit.lens.profile = None;
                            }
                            for name in &lens_names {
                                let selected = edit.lens.profile.as_ref() == Some(name);
                                if ui.selectable_label(selected, name).clicked() {
                                    edit.lens.profile = Some(name.clone());
                                }
                            }
                        });
                });
            }
            param_slider_spec(ui, "Vignetting", &mut edit.lens.vignetting, 0.0..=100.0, 100.0, SliderSpec::unit(Unit::Percent));
            // Say plainly what is and is not being corrected: a silently
            // inactive correction is worse than a visible "not calibrated".
            let status = match (&active, &identified_lens) {
                (Some(_), _) => shot_aperture.map_or_else(
                    || "Applied; no aperture in EXIF, using the widest grid".to_owned(),
                    |f| crate::i18n::format("Applied, interpolated to f/{0}", &[format!("{:.1}", f)]),
                ),
                (None, Some(lens)) => crate::i18n::format("{0} identified · no ShopPhoto shading calibration yet", std::slice::from_ref(lens)),
                (None, None) => "No lens recorded; choose an XCD lens under Advanced".to_owned(),
            };
            if search_state(ui).is_none() {
                ui.label(egui::RichText::new(crate::i18n::text(&status)).size(10.0).color(theme::TEXT_SECONDARY));
            }
            });
            if !source_is_raw { ui.label(crate::i18n::text("Sensor lens shading is available for RAW sources.")); }
            ui.separator();
            ui.horizontal(|ui| {
                ui.checkbox(&mut edit.lens.defringe, crate::i18n::text("Remove color fringes"));
                let auto = ui.add_enabled(
                    auto_available,
                    egui::Button::new(crate::i18n::text(if auto_busy { "Analyzing…" } else { "Auto" })).small(),
                );
                if auto.clicked() {
                    auto_defringe = true;
                }
                auto.on_hover_text(
                    crate::i18n::text("Analyze highlight edges for local purple and green chroma outliers; no change is made when evidence is weak"),
                );
            });
            ui.add_enabled_ui(edit.lens.defringe, |ui| {
                param_slider(ui, "Purple fringe", &mut edit.lens.defringe_purple, 0.0..=100.0, 0.0);
                param_slider(ui, "Green fringe", &mut edit.lens.defringe_green, 0.0..=100.0, 0.0);
            });
            advanced(ui, &mut advanced_open, ADV_LENS, 18, |ui| {
                ui.horizontal(|ui| {
                    ui.add_sized([theme::SLIDER_LABEL_W, 18.0], egui::Label::new(egui::RichText::new(crate::i18n::text("Lens")).size(11.5).color(theme::TEXT_SECONDARY)));
                    egui::ComboBox::from_id_salt("lens_identification")
                        .selected_text(identified_lens.clone().unwrap_or_else(|| "Unknown".to_owned()))
                        .show_ui(ui, |ui| {
                            let automatic = recorded_lens.as_deref().map_or(
                                "Auto (not recorded)".to_owned(),
                                |name| crate::i18n::format("Auto ({0})", &[name.to_string()]),
                            );
                            if ui.selectable_label(edit.lens.lens_model_override.is_none(), automatic).clicked() {
                                edit.lens.lens_model_override = None;
                            }
                            ui.separator();
                            ui.label(egui::RichText::new(crate::i18n::text("Hasselblad XCD catalog")).size(10.0).color(theme::TEXT_FAINT));
                            for name in &lens_catalog_names {
                                let selected = edit.lens.lens_model_override.as_ref() == Some(name);
                                if ui.selectable_label(selected, name).clicked() {
                                    edit.lens.lens_model_override = Some(name.clone());
                                }
                            }
                        });
                });
                sub_heading(ui, "Distortion & lateral CA");
                if ui.add_enabled(auto_available, egui::Button::new(crate::i18n::text("Auto lateral CA")).small()).clicked() {
                    auto_ca = true;
                }
                param_slider(ui, "Distortion", &mut edit.lens.distortion[0], -0.5..=0.5, 0.0);
                param_slider(ui, "Red CA", &mut edit.lens.lateral_ca[0], -0.01..=0.01, 0.0);
                param_slider(ui, "Blue CA", &mut edit.lens.lateral_ca[1], -0.01..=0.01, 0.0);
                ui.checkbox(&mut edit.lens.crop_to_valid, crate::i18n::text("Crop to valid pixels"));
                if ui
                    .add_enabled(active.is_some(), egui::Button::new(crate::i18n::text("Save geometry to .splens")).small())
                    .on_hover_text(crate::i18n::text("Store the fitted distortion and lateral-CA coefficients in the selected user lens profile"))
                    .clicked()
                {
                    state.save_lens_geometry_requested = true;
                }
                sub_heading(ui, "Fringe hues");
                ui.add_enabled_ui(edit.lens.defringe, |ui| {
                    ui.horizontal(|ui| {
                        if ui.small_button(crate::i18n::text("Pick purple…")).clicked() {
                            pick_purple_fringe = true;
                        }
                        if ui.small_button(crate::i18n::text("Pick green…")).clicked() {
                            pick_green_fringe = true;
                        }
                    });
                    param_slider_spec(ui, "Purple hue", &mut edit.lens.defringe_purple_hue, 0.0..=360.0, 300.0, SliderSpec::unit(Unit::Deg));
                    param_slider_spec(ui, "Purple range", &mut edit.lens.defringe_purple_range, 2.0..=90.0, 35.0, SliderSpec::unit(Unit::Deg));
                    param_slider_spec(ui, "Green hue", &mut edit.lens.defringe_green_hue, 0.0..=360.0, 120.0, SliderSpec::unit(Unit::Deg));
                    param_slider_spec(ui, "Green range", &mut edit.lens.defringe_green_range, 2.0..=90.0, 35.0, SliderSpec::unit(Unit::Deg));
                    ui.checkbox(&mut edit.lens.defringe_wide_bloom, crate::i18n::text("Wide / clipped highlight bloom"));
                    param_slider_spec(ui, "Fringe width", &mut edit.lens.defringe_radius_px, 1.0..=256.0, 2.0, SliderSpec::unit(Unit::Plain(0)));
                    param_slider(ui, "Edge protection", &mut edit.lens.defringe_threshold, 0.0..=100.0, 55.0);
                    if search_state(ui).is_none() {
                        ui.label(
                            egui::RichText::new(
                                crate::i18n::text("Pick directly on the colored halo. Wide mode handles eclipse-like bloom; fully clipped white detail cannot be recovered."),
                            )
                            .size(10.0)
                            .color(theme::TEXT_FAINT),
                        );
                    }
                });
            });
        });
        },
    );

    // -------------------------------------------------------- Geometry
    let active_size = state.active_size;
    let mut crop_mode = state.crop_mode;
    tool_group(
        ui,
        cards,
        Some(Group::Geometry),
        "Geometry",
        Some(4),
        &mut open.geometry,
        count(Group::Geometry),
        |ui| {
            ui.horizontal(|ui| {
                if ui
                    .selectable_label(crop_mode, crate::i18n::text("Crop"))
                    .on_hover_text(crate::i18n::text(
                        "Crop in the viewport · Return applies, Esc cancels",
                    ))
                    .clicked()
                {
                    crop_mode = !crop_mode;
                }
                if ui.small_button(crate::i18n::text("Reset crop")).clicked() {
                    edit.geometry.crop = [0.0, 0.0, 1.0, 1.0];
                    crop_aspect = None;
                }
                if ui
                    .add_enabled(
                        auto_available,
                        egui::Button::new(crate::i18n::text("Auto level")).small(),
                    )
                    .clicked()
                {
                    auto_level = true;
                }
                if let Some(image) = active_size {
                    ui.menu_button(crate::i18n::text("Aspect"), |ui| {
                        if ui.button(crate::i18n::text("Free")).clicked() {
                            crop_aspect = None;
                        }
                        let original = image.0 as f32 / image.1.max(1) as f32;
                        for (name, aspect) in [
                            ("Original", original),
                            ("1 : 1", 1.0),
                            ("4 : 5", 4.0 / 5.0),
                            ("3 : 2", 3.0 / 2.0),
                            ("16 : 9", 16.0 / 9.0),
                        ] {
                            if ui.button(name).clicked() {
                                set_crop_aspect(edit, image, aspect);
                                crop_aspect = Some(aspect);
                            }
                        }
                    });
                }
            });
            param_slider_spec(
                ui,
                "Straighten",
                &mut edit.geometry.rotate_deg,
                -45.0..=45.0,
                0.0,
                SliderSpec::unit(Unit::Signed(1)),
            );
            ui.horizontal(|ui| {
                if ui.small_button(crate::i18n::text("Rotate left")).clicked() {
                    edit.geometry.quarter_turns = (edit.geometry.quarter_turns + 3) % 4;
                }
                if ui.small_button(crate::i18n::text("Rotate right")).clicked() {
                    edit.geometry.quarter_turns = (edit.geometry.quarter_turns + 1) % 4;
                }
                ui.checkbox(
                    &mut edit.geometry.flip_horizontal,
                    crate::i18n::text("Flip H"),
                );
                ui.checkbox(
                    &mut edit.geometry.flip_vertical,
                    crate::i18n::text("Flip V"),
                );
            });
            advanced(ui, &mut advanced_open, ADV_GEOMETRY, 7, |ui| {
                param_slider(
                    ui,
                    "Vertical",
                    &mut edit.geometry.perspective_vertical,
                    -100.0..=100.0,
                    0.0,
                );
                param_slider(
                    ui,
                    "Horizontal",
                    &mut edit.geometry.perspective_horizontal,
                    -100.0..=100.0,
                    0.0,
                );
                param_slider_spec(
                    ui,
                    "Scale",
                    &mut edit.geometry.scale,
                    10.0..=400.0,
                    100.0,
                    SliderSpec::unit(Unit::Percent),
                );
                ui.add_space(theme::SPACE * 0.5);
                sub_heading(ui, "Guided upright");
                ui.horizontal_wrapped(|ui| {
                    ui.selectable_value(
                        &mut state.geometry_tool,
                        GeometryTool::UprightVertical,
                        crate::i18n::text("Vertical line"),
                    );
                    ui.selectable_value(
                        &mut state.geometry_tool,
                        GeometryTool::UprightHorizontal,
                        crate::i18n::text("Horizontal line"),
                    );
                    if ui.small_button(crate::i18n::text("Clear guides")).clicked() {
                        state.upright_guides.clear();
                        state.geometry_tool = GeometryTool::None;
                    }
                });
                if let Some(solution) = sp_core::edit::solve_guided_upright(&state.upright_guides) {
                    if ui
                        .small_button(crate::i18n::text("Apply guided upright"))
                        .clicked()
                    {
                        edit.geometry.rotate_deg = solution.rotate_deg;
                        edit.geometry.perspective_horizontal = solution.perspective_horizontal;
                        edit.geometry.perspective_vertical = solution.perspective_vertical;
                        state.geometry_tool = GeometryTool::None;
                    }
                } else {
                    ui.label(
                        egui::RichText::new(crate::i18n::text(
                            "Draw two vertical, horizontal, or one pair of each.",
                        ))
                        .size(10.0)
                        .color(theme::TEXT_FAINT),
                    );
                }
                ui.add_space(theme::SPACE * 0.5);
                sub_heading(ui, "Distortion calibration");
                ui.horizontal_wrapped(|ui| {
                    ui.selectable_value(
                        &mut state.geometry_tool,
                        GeometryTool::DistortionLine,
                        crate::i18n::text("Trace bent line"),
                    );
                    if ui.small_button(crate::i18n::text("Clear lines")).clicked() {
                        state.distortion_lines.clear();
                        state.geometry_tool = GeometryTool::None;
                    }
                });
                if state.distortion_lines.len() >= 2 {
                    if ui
                        .small_button(crate::i18n::text("Fit traced lines"))
                        .clicked()
                    {
                        if let Ok(coefficients) =
                            sp_core::lens::fit_radial_distortion(&state.distortion_lines)
                        {
                            edit.lens.enable = true;
                            edit.lens.distortion = coefficients;
                            state.geometry_tool = GeometryTool::None;
                        }
                    }
                } else {
                    ui.label(
                        egui::RichText::new(crate::i18n::text(
                            "Trace at least two grid or architectural lines with 3+ samples each.",
                        ))
                        .size(10.0)
                        .color(theme::TEXT_FAINT),
                    );
                }
            });
        },
    );

    // ---------------------------------------------------------- Output
    tool_group(
        ui,
        cards,
        Some(Group::Resolution),
        "Resolution",
        Some(5),
        &mut open.resolution,
        count(Group::Resolution),
        |ui| {
            resolution_group(ui, edit, active_size);
        },
    );

    tool_group(
        ui,
        cards,
        Some(Group::Hdr),
        "HDR",
        Some(5),
        &mut open.hdr,
        count(Group::Hdr),
        |ui| {
            let hdr_was_enabled = edit.hdr.enabled;
            ui.checkbox(
                &mut edit.hdr.enabled,
                crate::i18n::text("HDR preview & export"),
            );
            select_hdr_target_when_enabled(edit, hdr_was_enabled);
            ui.add_enabled_ui(edit.hdr.enabled, |ui| {
                ui.horizontal(|ui| {
                    ui.radio_value(
                        &mut edit.hdr.target,
                        sp_core::edit::HdrTarget::Sdr,
                        crate::i18n::text("SDR target"),
                    );
                    ui.radio_value(
                        &mut edit.hdr.target,
                        sp_core::edit::HdrTarget::Hdr,
                        crate::i18n::text("HDR target"),
                    );
                });
                param_slider_spec(
                    ui,
                    "Peak headroom",
                    &mut edit.hdr.headroom_stops,
                    0.0..=4.0,
                    2.0,
                    SliderSpec::unit(Unit::Plain(1)),
                );
                let (meter_rect, meter_response) = ui.allocate_exact_size(
                    egui::vec2(ui.available_width(), 7.0),
                    egui::Sense::hover(),
                );
                meter_response.on_hover_text({
                    let __l10n_args = (
                        &(2f32.powf(edit.hdr.headroom_stops)),
                        &(edit.hdr.headroom_stops),
                    );
                    crate::i18n::format(
                        "Selected HDR peak: {0}× reference white ({1} stops)",
                        &[
                            format!("{:.2}", __l10n_args.0),
                            format!("{:.2}", __l10n_args.1),
                        ],
                    )
                });
                ui.painter().rect_filled(meter_rect, 2.0, theme::BG_RAISED);
                let fraction = (edit.hdr.headroom_stops / 4.0).clamp(0.0, 1.0);
                if fraction > 0.0 {
                    let fill = egui::Rect::from_min_size(
                        meter_rect.min,
                        egui::vec2(meter_rect.width() * fraction, meter_rect.height()),
                    );
                    ui.painter()
                        .rect_filled(fill, 2.0, theme::ACCENT.linear_multiply(0.75));
                }
                for index in 1..4 {
                    let x = egui::lerp(meter_rect.x_range(), index as f32 / 4.0);
                    ui.painter().line_segment(
                        [
                            egui::pos2(x, meter_rect.top()),
                            egui::pos2(x, meter_rect.bottom()),
                        ],
                        egui::Stroke::new(1.0, theme::BG_PANEL),
                    );
                }
                advanced(ui, &mut advanced_open, ADV_HDR, 2, |ui| {
                    param_slider_spec(
                        ui,
                        "Ref. white",
                        &mut edit.hdr.scene_white_ev,
                        -2.0..=2.0,
                        -0.4,
                        SliderSpec::unit(Unit::Ev),
                    );
                    param_slider(
                        ui,
                        "Shoulder",
                        &mut edit.hdr.highlight_rolloff,
                        0.0..=100.0,
                        50.0,
                    );
                });
            });
            let note = if state.hdr_surface && cfg!(target_os = "linux") {
                "Experimental linear scRGB surface · verify compositor HDR"
            } else if state.hdr_surface {
                "EDR display active — highlights above UI white are real"
            } else {
                "SDR display · HDR preserved in PQ export"
            };
            ui.label(
                egui::RichText::new(crate::i18n::text(note))
                    .size(10.5)
                    .color(theme::TEXT_SECONDARY),
            );
            ui.label(
                egui::RichText::new(crate::i18n::text(
                    &(if cfg!(target_os = "linux") {
                        "Display headroom: not reported by the compositor".into()
                    } else {
                        {
                            let __l10n_args = (&(state.edr_potential.max(1.0).log2()),);
                            crate::i18n::format(
                                "Display headroom: {0} stops",
                                &[format!("{:.2}", __l10n_args.0)],
                            )
                        }
                    }),
                ))
                .size(10.5)
                .color(theme::TEXT_SECONDARY),
            );
            if edit.hdr.enabled
                && edit.hdr.target == sp_core::edit::HdrTarget::Hdr
                && state.hdr_surface
                && state.edr_headroom > 0.0
            {
                let available_stops = state.edr_headroom.max(1.0).log2();
                if edit.hdr.headroom_stops > available_stops + 0.25 {
                    ui.label(
                        egui::RichText::new({ let __l10n_args = (&(edit.hdr.headroom_stops - available_stops),); crate::i18n::format("Peak is {0} stops above current panel headroom · export remains intact", &[format!("{:.1}", __l10n_args.0)]) })
                        .size(10.5)
                        .color(theme::WARNING),
                    );
                }
            }
        },
    );

    tool_group(
        ui,
        cards,
        None,
        "Metadata",
        Some(5),
        &mut open.metadata,
        0,
        |ui| {
            capture_metadata_panel(ui, metadata_path.as_deref(), capture_metadata.as_ref());
        },
    );

    if cards.query.is_empty() {
        ui.add_space(theme::SPACE * 0.5);
        ui.horizontal(|ui| {
            let any = counts.iter().any(|n| *n > 0);
            if ui
                .add_enabled(
                    any,
                    egui::Button::new(
                        egui::RichText::new(crate::i18n::text("Reset all edits")).size(11.5),
                    )
                    .fill(egui::Color32::TRANSPARENT),
                )
                .on_hover_text(crate::i18n::text(
                    "Return every control to its default (one undo step)",
                ))
                .clicked()
            {
                state.apply_edit_step_labeled(
                    sp_core::EditState::default(),
                    Some("Reset all".into()),
                );
                state.active_preset = None;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.checkbox(&mut state.solo_panels, crate::i18n::text("Solo"))
                    .on_hover_text(crate::i18n::text(
                        "Opening one card closes the others (Option-click a header does this once)",
                    ));
            });
        });
        ui.add_space(theme::SPACE);
    }

    state.panels = open;
    state.curve_channel = curve_channel;
    state.retouch_tool = retouch_tool;
    state.retouch_active_local = retouch_active_local;
    state.mask_show_overlay = mask_show_overlay;
    state.retouch_radius = retouch_radius;
    state.retouch_feather = retouch_feather;
    state.spot_ui = spot_ui;
    if color_range_picker.is_some() {
        curve_picker = false;
    }
    state.color_range_picker = color_range_picker;
    state.curve_picker = curve_picker;
    state.wb_picker = wb_picker;
    state.film_base_picker = film_base_picker;
    state.sample_picker = sample_picker;
    state.scopes_enabled = scopes_enabled;
    state.hsl_selected = hsl_selected;
    state.hsl_tab = hsl_tab;
    state.crop_mode = crop_mode;
    state.crop_aspect = crop_aspect;
    state.advanced_open = advanced_open;
    state.ai_compare_original = ai_compare_original;
    state.ai_compare_split = ai_compare_split;
    state.ai_compare_position = ai_compare_position;
    if import_dcp {
        state.import_dcp_requested = true;
    }
    if import_cube {
        state.import_cube_requested = true;
    }
    if open_reference_match {
        state.reference_wizard_open = true;
    }
    state.auto_tone_requested |= auto_tone;
    state.auto_exposure_requested |= auto_exposure;
    state.auto_defringe_requested |= auto_defringe;
    state.auto_ca_requested |= auto_ca;
    state.auto_level_requested |= auto_level;
    if pick_purple_fringe {
        state.defringe_picker = Some(crate::state::DefringePickerTarget::Purple);
    } else if pick_green_fringe {
        state.defringe_picker = Some(crate::state::DefringePickerTarget::Green);
    }
    state.look_preview = look_preview;
}

// ---------------------------------------------------------------- history

/// Seconds since the Unix epoch, for relative history times.
fn now_s() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .min(i64::MAX as u64) as i64
}

pub(super) fn relative_time(created_s: i64, now: i64) -> String {
    let age = (now - created_s).max(0);
    match age {
        0..=44 => "just now".to_owned(),
        45..=89 => "1 min ago".to_owned(),
        90..=3_599 => {
            let __l10n_args = (&((age + 30) / 60),);
            crate::i18n::format("{0} min ago", &[__l10n_args.0.to_string()])
        }
        3_600..=86_399 => {
            let __l10n_args = (&((age + 1_800) / 3_600),);
            crate::i18n::format("{0} h ago", &[__l10n_args.0.to_string()])
        }
        _ => {
            let __l10n_args = (&((age + 43_200) / 86_400),);
            crate::i18n::format("{0} d ago", &[__l10n_args.0.to_string()])
        }
    }
}

/// UTC "YYYY-MM-DD HH:MM" for a Unix timestamp (civil-from-days).
pub(super) fn utc_timestamp(seconds: i64) -> String {
    let days = seconds.div_euclid(86_400);
    let secs = seconds.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02} UTC",
        secs / 3_600,
        (secs % 3_600) / 60
    )
}

fn history_card(ui: &mut egui::Ui, state: &mut AppState, cards: &mut Cards, open: &mut bool) {
    let steps = state.persistent_history.len();
    let mut jump = None;
    let mut restore_snapshot = None;
    let mut compare_with = None;
    let mut save_snapshot = false;
    let mut preview = None;
    let current = state.history_current();
    let cursor = state.history_cursor;
    tool_group(ui, cards, None, "History", None, open, 0, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label(
                egui::RichText::new(crate::i18n::text("Versions"))
                    .size(11.0)
                    .color(theme::TEXT_SECONDARY),
            );
            if ui
                .selectable_label(state.virtual_copy.is_none(), crate::i18n::text("Primary"))
                .clicked()
            {
                state.virtual_copy_switch_requested = Some(String::new());
            }
            for copy_id in &state.virtual_copies {
                if ui
                    .selectable_label(
                        state.virtual_copy.as_deref() == Some(copy_id),
                        state.virtual_copy_names.get(copy_id).unwrap_or(copy_id),
                    )
                    .clicked()
                {
                    state.virtual_copy_switch_requested = Some(copy_id.clone());
                }
            }
            if ui
                .small_button(crate::i18n::text("New virtual copy"))
                .clicked()
            {
                state.virtual_copy_create_requested = true;
            }
        });
        if state.virtual_copy.is_some() {
            ui.horizontal_wrapped(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut state.virtual_copy_name_draft)
                        .hint_text(crate::i18n::text("Version name"))
                        .desired_width(130.0),
                );
                if ui
                    .add_enabled(
                        !state.virtual_copy_name_draft.trim().is_empty(),
                        egui::Button::new(crate::i18n::text("Rename")).small(),
                    )
                    .clicked()
                {
                    state.virtual_copy_rename_requested = true;
                }
                if ui.small_button(crate::i18n::text("Remove…")).clicked() {
                    state.virtual_copy_trash_confirm = true;
                }
            });
            if state.virtual_copy_trash_confirm {
                ui.label(crate::i18n::text(
                    "Move this version's edits to Trash? The original photo is retained.",
                ));
                ui.horizontal(|ui| {
                    if ui
                        .button(crate::i18n::text("Move version to Trash"))
                        .clicked()
                    {
                        state.virtual_copy_trash_requested = true;
                        state.virtual_copy_trash_confirm = false;
                    }
                    if ui.button(crate::i18n::text("Cancel")).clicked() {
                        state.virtual_copy_trash_confirm = false;
                    }
                });
            }
        }
        ui.horizontal(|ui| {
            let field = ui.add(
                egui::TextEdit::singleline(&mut state.snapshot_name)
                    .hint_text(crate::i18n::text("Snapshot name"))
                    .desired_width((ui.available_width() - 118.0).max(60.0)),
            );
            let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if ui
                .add_enabled(
                    !state.snapshot_name.trim().is_empty(),
                    egui::Button::new(crate::i18n::text("Save snapshot")).small(),
                )
                .clicked()
                || (enter && !state.snapshot_name.trim().is_empty())
            {
                save_snapshot = true;
            }
        });
        if !state.snapshots.is_empty() {
            ui.horizontal_wrapped(|ui| {
                for (index, snapshot) in state.snapshots.iter().enumerate() {
                    let chip = ui.add(
                        egui::Button::new(egui::RichText::new(&snapshot.name).size(11.5))
                            .corner_radius(7.0),
                    );
                    if chip.hovered() {
                        preview = Some(snapshot.edit.clone());
                    }
                    if chip.clicked() {
                        restore_snapshot = Some(index);
                    }
                    chip.on_hover_text({
                        let __l10n_args = (&(utc_timestamp(snapshot.created_s)),);
                        crate::i18n::format(
                            "Restore snapshot · saved {0}",
                            std::slice::from_ref(__l10n_args.0),
                        )
                    })
                    .context_menu(|ui| {
                        if ui
                            .button(crate::i18n::text("Compare with current"))
                            .clicked()
                        {
                            compare_with = Some((snapshot.name.clone(), snapshot.edit.clone()));
                            ui.close();
                        }
                    });
                    let cmp = ui.add(
                        egui::Button::new(egui::RichText::new(crate::i18n::text("vs")).size(11.0))
                            .small()
                            .frame(false),
                    );
                    if cmp.clicked() {
                        compare_with = Some((snapshot.name.clone(), snapshot.edit.clone()));
                    }
                    cmp.on_hover_text({
                        let __l10n_args = (&(snapshot.name),);
                        crate::i18n::format(
                            "Split-compare “{0}” with the current edit",
                            std::slice::from_ref(__l10n_args.0),
                        )
                    });
                }
            });
        }
        ui.add_space(4.0);
        if steps == 0 {
            ui.label(
                egui::RichText::new(crate::i18n::text("Edits appear here as named steps."))
                    .size(10.5)
                    .color(theme::TEXT_FAINT),
            );
            return;
        }
        let now = now_s();
        egui::ScrollArea::vertical()
            .id_salt("history-steps")
            .max_height(220.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                for index in (0..steps).rev().take(60) {
                    let entry = &state.persistent_history[index];
                    let is_current = current == Some(index);
                    let is_redo = cursor.is_some_and(|c| index > c);
                    let (rect, resp) = ui.allocate_exact_size(
                        egui::vec2(ui.available_width(), 22.0),
                        egui::Sense::click(),
                    );
                    let painter = ui.painter();
                    if is_current {
                        painter.rect_filled(rect, 5.0, theme::ACCENT.gamma_multiply(0.16));
                    } else if resp.hovered() {
                        painter.rect_filled(rect, 5.0, theme::BG_HOVER);
                    }
                    let dot = egui::pos2(rect.left() + 9.0, rect.center().y);
                    painter.line_segment(
                        [
                            egui::pos2(dot.x, rect.top()),
                            egui::pos2(dot.x, rect.bottom()),
                        ],
                        egui::Stroke::new(1.0, theme::HAIRLINE),
                    );
                    if is_current {
                        painter.circle_filled(dot, 3.5, theme::ACCENT);
                    } else if is_redo {
                        painter.circle_stroke(dot, 3.0, egui::Stroke::new(1.0, theme::TEXT_FAINT));
                    } else {
                        painter.circle_filled(dot, 3.0, theme::TEXT_FAINT);
                    }
                    let label_color = if is_redo {
                        theme::TEXT_FAINT.gamma_multiply(0.7)
                    } else if is_current {
                        theme::TEXT_PRIMARY
                    } else {
                        theme::TEXT_SECONDARY
                    };
                    painter
                        .with_clip_rect(
                            rect.shrink2(egui::vec2(60.0, 0.0))
                                .translate(egui::vec2(-40.0, 0.0)),
                        )
                        .text(
                            egui::pos2(rect.left() + 20.0, rect.center().y),
                            egui::Align2::LEFT_CENTER,
                            &entry.label,
                            egui::FontId::proportional(12.0),
                            label_color,
                        );
                    painter.text(
                        egui::pos2(rect.right() - 6.0, rect.center().y),
                        egui::Align2::RIGHT_CENTER,
                        relative_time(entry.created_s, now),
                        egui::FontId::proportional(10.5),
                        theme::TEXT_FAINT,
                    );
                    if resp.hovered() && !is_current {
                        preview = Some(entry.edit.clone());
                    }
                    if resp.clicked() {
                        jump = Some(index);
                    }
                    let label = entry.label.clone();
                    let edit = entry.edit.clone();
                    resp.on_hover_text({
                        let __l10n_args = (&(utc_timestamp(entry.created_s)),);
                        crate::i18n::format(
                            "{0} · click to return to this step",
                            std::slice::from_ref(__l10n_args.0),
                        )
                    })
                    .context_menu(|ui| {
                        if ui
                            .button(crate::i18n::text("Compare with current"))
                            .clicked()
                        {
                            compare_with = Some((label.clone(), edit.clone()));
                            ui.close();
                        }
                    });
                }
            });
    });
    state.history_preview = preview;
    if save_snapshot {
        let name = state.snapshot_name.trim().to_owned();
        state.snapshots.retain(|snapshot| snapshot.name != name);
        state.snapshots.push(sp_catalog::NamedSnapshot {
            name,
            created_s: now_s(),
            edit: state.edit.clone(),
        });
        state.snapshot_name.clear();
        state.sidecar_dirty = true;
    }
    if let Some(index) = restore_snapshot {
        let snapshot = &state.snapshots[index];
        let (name, edit) = (snapshot.name.clone(), snapshot.edit.clone());
        state.apply_edit_step_labeled(
            edit,
            Some(crate::i18n::format(
                "Snapshot: {0}",
                std::slice::from_ref(&name),
            )),
        );
    }
    if let Some(index) = jump {
        state.jump_to_history(index);
    }
    if let Some(before) = compare_with {
        state.compare_before = Some(before);
        state.compare_mode = crate::state::CompareMode::Split;
        state.show_original = false;
    }
}

// --------------------------------------------------------------- loupe

/// 1:1 noise loupe (Phase 4): a true full-resolution crop at the view
/// center so NR is judged at pixel level regardless of viewport zoom.
pub(super) fn noise_loupe(
    ui: &mut egui::Ui,
    loupe: Option<(egui::TextureId, sp_pipeline::renderer::TileInfo)>,
) {
    ui.label(theme::section_text("Loupe · 1:1"));
    ui.add_space(2.0);
    let size = egui::vec2(ui.available_width(), 150.0);
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, theme::RADIUS_CONTROL as f32, theme::BG_VIEWPORT);
    match loupe {
        Some((id, info)) => {
            // Show the visible region 1:1-ish, centered, UV-cropping margins.
            let uv = egui::Rect::from_min_max(
                egui::pos2(
                    info.margin_x as f32 / info.tex_w as f32,
                    info.margin_y as f32 / info.tex_h as f32,
                ),
                egui::pos2(
                    (info.margin_x + info.w) as f32 / info.tex_w as f32,
                    (info.margin_y + info.h) as f32 / info.tex_h as f32,
                ),
            );
            let side = rect.height().min(rect.width());
            let img_rect = egui::Rect::from_center_size(rect.center(), egui::vec2(side, side));
            painter.image(id, img_rect, uv, egui::Color32::WHITE);
            painter.rect_stroke(
                img_rect,
                theme::RADIUS_CONTROL as f32,
                egui::Stroke::new(1.0, theme::HAIRLINE),
                egui::StrokeKind::Inside,
            );
        }
        None => {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "Loupe shows at fit zoom — viewing 1:1 already",
                egui::FontId::proportional(10.5),
                theme::TEXT_FAINT,
            );
        }
    }
}

// ------------------------------------------------------------- histogram

const HISTOGRAM_REGIONS: [&str; 5] = ["Blacks", "Shadows", "Exposure", "Highlights", "Whites"];

/// Live RGB histogram from the first three planes of the renderer's
/// R/G/B/luminance histogram. It is also a control: dragging horizontally
/// edits the tone control of the region under the pointer, which is tinted
/// and labelled while hovered. The corner triangles show and toggle
/// shadow/highlight clipping.
pub(super) fn histogram_widget(ui: &mut egui::Ui, state: &mut AppState) {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), theme::HISTOGRAM_H),
        egui::Sense::click_and_drag(),
    );
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, theme::RADIUS_PANEL as f32 - 2.0, theme::BG_VIEWPORT);
    let Some(bins) = state.histogram.clone().filter(|bins| bins.len() >= 768) else {
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            crate::i18n::text("Histogram"),
            egui::FontId::proportional(11.0),
            theme::TEXT_FAINT,
        );
        return;
    };
    // Normalize against the 99.5th-percentile bin so a spike (clipped sky)
    // doesn't flatten everything else.
    let mut sorted: Vec<u32> = bins[..768].to_vec();
    sorted.sort_unstable();
    let peak = sorted[(sorted.len() as f32 * 0.995) as usize].max(1) as f32;

    let fills = [
        egui::Color32::from_rgba_unmultiplied(0xE8, 0x5C, 0x48, 70),
        egui::Color32::from_rgba_unmultiplied(0x66, 0xD1, 0x7E, 70),
        egui::Color32::from_rgba_unmultiplied(0x5C, 0x96, 0xF0, 70),
    ];
    let lines = [
        egui::Color32::from_rgba_unmultiplied(0xF0, 0x77, 0x60, 200),
        egui::Color32::from_rgba_unmultiplied(0x84, 0xE0, 0x99, 200),
        egui::Color32::from_rgba_unmultiplied(0x7C, 0xAE, 0xFF, 200),
    ];
    let inner = rect.shrink(4.0);
    let bar_w = inner.width() / 256.0;
    for (plane, fill) in fills.iter().enumerate() {
        for i in 0..256 {
            let v = (bins[plane * 256 + i] as f32 / peak).min(1.0);
            if v <= 0.0 {
                continue;
            }
            let x = inner.left() + bar_w * i as f32;
            painter.rect_filled(
                egui::Rect::from_min_max(
                    egui::pos2(x, inner.bottom() - inner.height() * v),
                    egui::pos2(x + bar_w, inner.bottom()),
                ),
                0.0,
                *fill,
            );
        }
        let points: Vec<egui::Pos2> = (0..256)
            .map(|i| {
                let v = (bins[plane * 256 + i] as f32 / peak).min(1.0);
                egui::pos2(
                    inner.left() + bar_w * (i as f32 + 0.5),
                    inner.bottom() - inner.height() * v,
                )
            })
            .collect();
        painter.add(egui::Shape::line(
            points,
            egui::Stroke::new(1.0, lines[plane]),
        ));
    }
    let white_x = inner.left() + bar_w * 224.0;
    painter.line_segment(
        [
            egui::pos2(white_x, inner.top()),
            egui::pos2(white_x, inner.bottom()),
        ],
        egui::Stroke::new(1.0, theme::TEXT_FAINT.gamma_multiply(0.6)),
    );

    // Region hover/drag.
    let drag_id = ui.id().with("histogram-tone-drag");
    let region_at = |x: f32| (((x - rect.left()) / rect.width()).clamp(0.0, 0.999) * 5.0) as usize;
    if response.drag_started() {
        if let Some(pos) = response.interact_pointer_pos() {
            let start = [
                state.edit.tone.blacks,
                state.edit.tone.shadows,
                state.edit.tone.exposure_ev,
                state.edit.tone.highlights,
                state.edit.tone.whites,
            ];
            ui.ctx()
                .data_mut(|data| data.insert_temp(drag_id, (region_at(pos.x), start, pos.x)));
        }
    }
    let dragging = ui
        .ctx()
        .data(|d| d.get_temp::<(usize, [f32; 5], f32)>(drag_id));
    if response.dragged() {
        if let (Some((region, start, x0)), Some(pos)) = (dragging, response.interact_pointer_pos())
        {
            let normalized = (pos.x - x0) / rect.width();
            match region {
                0 => state.edit.tone.blacks = (start[0] + normalized * 200.0).clamp(-100.0, 100.0),
                1 => state.edit.tone.shadows = (start[1] + normalized * 200.0).clamp(-100.0, 100.0),
                2 => state.edit.tone.exposure_ev = (start[2] + normalized * 10.0).clamp(-5.0, 5.0),
                3 => {
                    state.edit.tone.highlights =
                        (start[3] + normalized * 200.0).clamp(-100.0, 100.0)
                }
                _ => state.edit.tone.whites = (start[4] + normalized * 200.0).clamp(-100.0, 100.0),
            }
        }
    }
    if response.drag_stopped() {
        ui.ctx()
            .data_mut(|d| d.remove::<(usize, [f32; 5], f32)>(drag_id));
    }
    let active_region = if response.dragged() {
        dragging.map(|(region, _, _)| region)
    } else {
        response.hover_pos().map(|pos| region_at(pos.x))
    };
    if let Some(region) = active_region {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        let band = egui::Rect::from_min_max(
            egui::pos2(egui::lerp(rect.x_range(), region as f32 / 5.0), rect.top()),
            egui::pos2(
                egui::lerp(rect.x_range(), (region + 1) as f32 / 5.0),
                rect.bottom(),
            ),
        );
        painter.rect_filled(band, 0.0, egui::Color32::from_white_alpha(10));
        let value = match region {
            0 => Unit::Signed(0).format(f64::from(state.edit.tone.blacks)),
            1 => Unit::Signed(0).format(f64::from(state.edit.tone.shadows)),
            2 => Unit::Ev.format(f64::from(state.edit.tone.exposure_ev)),
            3 => Unit::Signed(0).format(f64::from(state.edit.tone.highlights)),
            _ => Unit::Signed(0).format(f64::from(state.edit.tone.whites)),
        };
        let text = format!("{} {value}", HISTOGRAM_REGIONS[region]);
        let galley = painter.layout_no_wrap(
            text,
            egui::FontId::proportional(11.0),
            if response.dragged() {
                theme::ACCENT_HOVER
            } else {
                theme::TEXT_PRIMARY
            },
        );
        let chip = egui::Rect::from_center_size(
            egui::pos2(
                band.center().x.clamp(
                    rect.left() + galley.size().x / 2.0 + 8.0,
                    rect.right() - galley.size().x / 2.0 - 8.0,
                ),
                rect.top() + 14.0,
            ),
            galley.size() + egui::vec2(12.0, 4.0),
        );
        painter.rect_filled(chip, 7.0, theme::BG_HUD);
        painter.galley(
            chip.center() - galley.size() / 2.0,
            galley,
            theme::TEXT_PRIMARY,
        );
    }

    // Clipping triangles: lit when the image clips, click toggles J / O.
    let total = bins[..256]
        .iter()
        .map(|v| u64::from(*v))
        .sum::<u64>()
        .max(1) as f32;
    let threshold = total * 0.002;
    let shadows_clip = (0..3).any(|c| bins[c * 256] as f32 > threshold);
    let white_bin =
        if state.edit.hdr.enabled && state.edit.hdr.target == sp_core::edit::HdrTarget::Hdr {
            255
        } else {
            224
        };
    let highlights_clip = (0..3).any(|c| {
        bins[c * 256 + white_bin..c * 256 + 256]
            .iter()
            .map(|v| *v as f32)
            .sum::<f32>()
            > threshold
    });
    for (bit, lit, left) in [(1u32, shadows_clip, true), (2u32, highlights_clip, false)] {
        let corner = if left {
            rect.left_top() + egui::vec2(5.0, 5.0)
        } else {
            rect.right_top() + egui::vec2(-5.0, 5.0)
        };
        let size = 11.0;
        let hit = egui::Rect::from_center_size(
            corner + egui::vec2(if left { size / 2.0 } else { -size / 2.0 }, size / 2.0),
            egui::vec2(20.0, 20.0),
        );
        let resp = ui.interact(hit, ui.id().with(("clip-tri", bit)), egui::Sense::click());
        let on = state.clipping_mode & bit != 0;
        let base = if left {
            theme::CLIP_SHADOW
        } else {
            theme::CLIP_HIGHLIGHT
        };
        let color = if on || lit || resp.hovered() {
            base
        } else {
            theme::TEXT_FAINT.gamma_multiply(0.6)
        };
        let tri = if left {
            vec![
                corner,
                corner + egui::vec2(size, 0.0),
                corner + egui::vec2(0.0, size),
            ]
        } else {
            vec![
                corner,
                corner + egui::vec2(0.0, size),
                corner + egui::vec2(-size, 0.0),
            ]
        };
        painter.add(egui::Shape::convex_polygon(
            tri,
            color,
            if on {
                egui::Stroke::new(1.5, theme::TEXT_PRIMARY)
            } else {
                egui::Stroke::NONE
            },
        ));
        if resp.clicked() {
            state.clipping_mode ^= bit;
        }
        resp.on_hover_text(match (left, lit) {
            (true, true) => "Shadows are clipping · click to show them (J)",
            (true, false) => "Shadow clipping overlay (J)",
            (false, true) => "Highlights are clipping · click to show them (O)",
            (false, false) => "Highlight clipping overlay (O)",
        });
    }
}

// ------------------------------------------------------------ tone curve

/// Interactive Catmull-Rom curve editor. Drag points; click empty space to
/// add; double-click a point to remove it (endpoints are fixed in x).
pub(super) fn curve_editor(
    ui: &mut egui::Ui,
    curve: &mut sp_core::edit::ToneCurve,
    stroke_color: egui::Color32,
) {
    use sp_core::edit::CurvePoint;
    if search_state(ui).is_some() {
        return;
    }
    let size = egui::vec2(ui.available_width(), 160.0);
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, theme::RADIUS_CONTROL as f32, theme::BG_VIEWPORT);
    let inner = rect.shrink(6.0);

    let to_screen = |p: CurvePoint| {
        egui::pos2(
            inner.left() + p.x * inner.width(),
            inner.bottom() - p.y * inner.height(),
        )
    };
    let from_screen = |pos: egui::Pos2| CurvePoint {
        x: ((pos.x - inner.left()) / inner.width()).clamp(0.0, 1.0),
        y: ((inner.bottom() - pos.y) / inner.height()).clamp(0.0, 1.0),
    };

    for i in 1..4 {
        let f = i as f32 / 4.0;
        let x = inner.left() + inner.width() * f;
        let y = inner.top() + inner.height() * f;
        let st = egui::Stroke::new(1.0, theme::HAIRLINE.linear_multiply(0.5));
        painter.line_segment(
            [egui::pos2(x, inner.top()), egui::pos2(x, inner.bottom())],
            st,
        );
        painter.line_segment(
            [egui::pos2(inner.left(), y), egui::pos2(inner.right(), y)],
            st,
        );
    }

    let samples: Vec<egui::Pos2> = (0..=128)
        .map(|i| {
            let e = i as f32 / 128.0;
            to_screen(CurvePoint {
                x: e,
                y: sp_pipeline::tone::sample_curve(curve, e),
            })
        })
        .collect();
    painter.add(egui::Shape::line(
        samples,
        egui::Stroke::new(1.5, stroke_color),
    ));

    let drag_id = ui.id().with("curve-drag");
    let mut dragging: Option<usize> = ui.ctx().data_mut(|d| d.get_temp(drag_id)).unwrap_or(None);
    let hit_radius = 10.0;
    let pointer = resp.interact_pointer_pos();

    if resp.drag_started() {
        if let Some(pos) = pointer {
            dragging = curve
                .points
                .iter()
                .position(|p| to_screen(*p).distance(pos) < hit_radius);
        }
    }
    if let (Some(idx), Some(pos), true) = (dragging, pointer, resp.dragged()) {
        let mut p = from_screen(pos);
        let n = curve.points.len();
        if idx == 0 {
            p.x = 0.0;
        } else if idx == n - 1 {
            p.x = 1.0;
        } else {
            let lo = curve.points[idx - 1].x + 0.02;
            let hi = curve.points[idx + 1].x - 0.02;
            p.x = p.x.clamp(lo, hi);
        }
        curve.points[idx] = p;
    }
    if resp.drag_stopped() {
        dragging = None;
    }
    if resp.double_clicked() {
        if let Some(pos) = pointer {
            if let Some(idx) = curve
                .points
                .iter()
                .position(|p| to_screen(*p).distance(pos) < hit_radius)
            {
                let n = curve.points.len();
                if idx != 0 && idx != n - 1 {
                    curve.points.remove(idx);
                }
            }
        }
    } else if resp.clicked() {
        if let Some(pos) = pointer {
            let near_existing = curve
                .points
                .iter()
                .any(|p| to_screen(*p).distance(pos) < hit_radius);
            if !near_existing {
                let p = from_screen(pos);
                let idx = curve
                    .points
                    .iter()
                    .position(|q| q.x > p.x)
                    .unwrap_or(curve.points.len() - 1);
                curve.points.insert(idx, p);
            }
        }
    }
    ui.ctx().data_mut(|d| d.insert_temp(drag_id, dragging));

    for p in &curve.points {
        painter.circle_filled(to_screen(*p), 3.5, stroke_color);
    }

    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(crate::i18n::text(
                "click: add · drag: move · double-click: remove",
            ))
            .size(10.0)
            .color(theme::TEXT_SECONDARY),
        );
        if ui.small_button(crate::i18n::text("Reset")).clicked() {
            *curve = Default::default();
        }
    });
}

// --------------------------------------------------------------- color

const BAND_NAMES: [&str; 8] = [
    "Red", "Orange", "Yellow", "Green", "Aqua", "Blue", "Purple", "Magenta",
];
/// Representative hue (degrees) of each HSL band, for the gradient tracks.
const BAND_HUES: [f32; 8] = [0.0, 30.0, 55.0, 120.0, 180.0, 220.0, 275.0, 320.0];

fn hsv(hue_deg: f32, s: f32, v: f32) -> egui::Color32 {
    egui::Color32::from(egui::ecolor::Hsva::new(
        hue_deg.rem_euclid(360.0) / 360.0,
        s,
        v,
        1.0,
    ))
}

/// Color mixer: Hue / Saturation / Luminance tabs with all eight bands
/// visible at once, each on a track that shows what the band does.
pub(super) fn color_mixer(ui: &mut egui::Ui, edit: &mut sp_core::EditState, tab: &mut u8) {
    let searching = search_state(ui).is_some();
    ui.horizontal(|ui| {
        sub_heading(ui, "Color mixer");
        if !searching {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                for (index, name) in [(2u8, "Lum"), (1, "Sat"), (0, "Hue")] {
                    if ui
                        .selectable_label(*tab == index, egui::RichText::new(name).size(11.0))
                        .clicked()
                    {
                        *tab = index;
                    }
                }
            });
        }
    });
    for (index, name) in BAND_NAMES.iter().enumerate() {
        let h = BAND_HUES[index];
        let band = &mut edit.color.hsl[index];
        let (value, gradient) = match *tab {
            0 => (
                &mut band.hue_shift,
                [
                    hsv(h - 30.0, 0.7, 0.85),
                    hsv(h, 0.7, 0.85),
                    hsv(h + 30.0, 0.7, 0.85),
                ],
            ),
            1 => (
                &mut band.saturation,
                [hsv(h, 0.05, 0.6), hsv(h, 0.45, 0.75), hsv(h, 0.9, 0.9)],
            ),
            _ => (
                &mut band.luminance,
                [hsv(h, 0.6, 0.25), hsv(h, 0.6, 0.6), hsv(h, 0.35, 0.95)],
            ),
        };
        param_slider_spec(
            ui,
            name,
            value,
            -100.0..=100.0,
            0.0,
            SliderSpec {
                unit: Unit::Signed(0),
                log: false,
                gradient: Some(gradient),
            },
        );
    }
}

/// Three color-grading wheels. Drag the puck for hue (angle) and amount
/// (distance); ⇧ keeps the hue; double-click resets.
fn grading_wheels(ui: &mut egui::Ui, edit: &mut sp_core::EditState) {
    if search_hides(ui, "Color grading shadows midtones highlights") {
        return;
    }
    ui.horizontal(|ui| {
        sub_heading(ui, "Color grading");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                egui::RichText::new(crate::i18n::text("⇧ keeps hue"))
                    .size(10.0)
                    .color(theme::TEXT_FAINT),
            );
        });
    });
    let spacing = 8.0;
    let size = ((ui.available_width() - 2.0 * spacing) / 3.0).clamp(60.0, 104.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = spacing;
        let grading = &mut edit.color.grading;
        for (label, wheel) in [
            ("Shadows", &mut grading.shadows),
            ("Midtones", &mut grading.midtones),
            ("Highlights", &mut grading.highlights),
        ] {
            ui.vertical(|ui| {
                ui.set_width(size);
                grading_wheel(ui, label, wheel, size);
            });
        }
    });
}

fn grading_wheel(
    ui: &mut egui::Ui,
    label: &str,
    wheel: &mut sp_core::edit::ColorGradeWheel,
    size: f32,
) {
    let (rect, resp) =
        ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::click_and_drag());
    let center = rect.center();
    let radius = size / 2.0 - 2.0;
    let painter = ui.painter();
    // Fan mesh: neutral grey centre to saturated hue ring.
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(center, egui::Color32::from_gray(118));
    let segments = 60;
    for i in 0..=segments {
        let angle = i as f32 / segments as f32 * std::f32::consts::TAU;
        let hue = angle.to_degrees();
        mesh.colored_vertex(
            center + egui::vec2(angle.cos(), angle.sin()) * radius,
            hsv(hue, 0.78, 0.92),
        );
    }
    for i in 1..=segments as u32 {
        mesh.add_triangle(0, i, i + 1);
    }
    painter.add(mesh);
    painter.circle_stroke(
        center,
        radius,
        egui::Stroke::new(1.0, egui::Color32::from_black_alpha(90)),
    );
    painter.circle_filled(center, 1.5, egui::Color32::from_black_alpha(140));

    if resp.dragged() || resp.drag_started() || resp.clicked() {
        if let Some(pos) = resp.interact_pointer_pos() {
            let d = pos - center;
            let amount = (d.length() / radius * 100.0).clamp(0.0, 100.0);
            let keep_hue = ui.input(|i| i.modifiers.shift) && wheel.saturation > 0.0;
            if !keep_hue {
                wheel.hue_degrees = d.y.atan2(d.x).to_degrees().rem_euclid(360.0);
            }
            wheel.saturation = amount;
        }
    }
    if resp.double_clicked() {
        wheel.hue_degrees = 0.0;
        wheel.saturation = 0.0;
    }
    let angle = wheel.hue_degrees.to_radians();
    let puck = center + egui::vec2(angle.cos(), angle.sin()) * radius * (wheel.saturation / 100.0);
    painter.circle_filled(
        puck + egui::vec2(0.0, 1.0),
        6.5,
        egui::Color32::from_black_alpha(140),
    );
    painter.circle_filled(
        puck,
        5.0,
        if wheel.saturation > 0.0 {
            hsv(wheel.hue_degrees, 0.25 + wheel.saturation / 140.0, 0.9)
        } else {
            egui::Color32::from_gray(190)
        },
    );
    painter.circle_stroke(puck, 5.5, egui::Stroke::new(2.0, egui::Color32::WHITE));
    if resp.hovered() || resp.dragged() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
    }
    let text = if wheel.saturation > 0.05 {
        format!("{:.0}° · {:.0}", wheel.hue_degrees, wheel.saturation)
    } else {
        "neutral".to_owned()
    };
    resp.on_hover_text(crate::i18n::text(
        "Drag for hue and amount · ⇧ keeps the hue · double-click resets",
    ));
    ui.vertical_centered(|ui| {
        ui.label(
            egui::RichText::new(label)
                .size(11.0)
                .color(if wheel.saturation > 0.05 {
                    theme::TEXT_PRIMARY
                } else {
                    theme::TEXT_SECONDARY
                }),
        );
        ui.label(
            egui::RichText::new(crate::i18n::text(text.as_ref()))
                .size(10.0)
                .color(theme::TEXT_FAINT),
        );
    });
}

pub(super) fn monochrome_mixer(
    ui: &mut egui::Ui,
    edit: &mut sp_core::EditState,
    selected: &mut usize,
) {
    const NAMES: [&str; 8] = [
        "Reds", "Oranges", "Yellows", "Greens", "Aquas", "Blues", "Purples", "Magentas",
    ];
    sub_heading(ui, "Black & White Mix");
    egui::ComboBox::from_id_salt("monochrome-band")
        .selected_text(NAMES[(*selected).min(7)])
        .show_ui(ui, |ui| {
            for (index, name) in NAMES.iter().enumerate() {
                ui.selectable_value(selected, index, crate::i18n::text(name));
            }
        });
    let value = &mut edit.color.monochrome_mix[(*selected).min(7)];
    param_slider(ui, "Luminance", value, -100.0..=100.0, 0.0);
    if ui
        .small_button(crate::i18n::text("Reset B&W mix"))
        .clicked()
    {
        edit.color.monochrome_mix = [0.0; sp_core::edit::HSL_BAND_COUNT];
    }
}

/// Supersampling mode selector with computed output sizes (BUILD.md Phase 5).
/// Until an image is open, sizes are shown for the X2D 100C sensor.
pub(super) fn resolution_group(
    ui: &mut egui::Ui,
    edit: &mut sp_core::EditState,
    active_size: Option<(u32, u32)>,
) {
    let (w, h) = active_size.unwrap_or((11656, 8742));
    for (mode, label) in [
        (ResolutionMode::Full, "Full"),
        (ResolutionMode::Super2x2, "2×2 supersample"),
        (ResolutionMode::Super3x3, "3×3 supersample"),
        (ResolutionMode::Super4x4, "4×4 supersample"),
    ] {
        let f = mode.factor();
        let (mw, mh) = (w / f, h / f);
        let mp = (mw as f64 * mh as f64) / 1.0e6;
        let text = format!("{label} — {mw} × {mh} · {mp:.1} MP");
        ui.radio_value(&mut edit.resolution.mode, mode, text);
    }
    ui.label(
        egui::RichText::new(crate::i18n::text("Applies to export and 1:1 inspection"))
            .size(10.5)
            .color(theme::TEXT_SECONDARY),
    );
}

// ------------------------------------------------------------- metadata

pub(super) fn metadata_panel(ui: &mut egui::Ui, state: &mut AppState) {
    ui.label(theme::section_text("Metadata"));
    ui.add_space(theme::SPACE * 0.5);
    capture_metadata_panel(ui, state.open_image.as_deref(), state.meta.as_ref());

    ui.add_space(theme::SPACE);
    ui.separator();
    let before = state.descriptive_metadata.clone();
    ui.label(theme::section_text("Description"));
    let field_label = |ui: &mut egui::Ui, label: &str| {
        ui.label(
            egui::RichText::new(crate::i18n::text(label))
                .small()
                .color(theme::TEXT_SECONDARY),
        );
    };
    field_label(ui, "Caption");
    ui.add(
        egui::TextEdit::multiline(&mut state.descriptive_metadata.caption)
            .desired_rows(3)
            .desired_width(f32::INFINITY)
            .hint_text(crate::i18n::text("What's in this photo?")),
    );
    field_label(ui, "Keywords");
    let mut keywords = state.descriptive_metadata.keywords.join(", ");
    if ui
        .add(
            egui::TextEdit::singleline(&mut keywords)
                .desired_width(f32::INFINITY)
                .hint_text(crate::i18n::text("beach, sunset, family")),
        )
        .on_hover_text(crate::i18n::text("Comma-separated keywords"))
        .changed()
    {
        state.descriptive_metadata.keywords = keywords
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .collect();
    }
    for (label, value) in [
        ("Creator", &mut state.descriptive_metadata.creator),
        ("Copyright", &mut state.descriptive_metadata.copyright),
        ("Usage rights", &mut state.descriptive_metadata.rights),
        ("Contact email", &mut state.descriptive_metadata.contact),
    ] {
        field_label(ui, label);
        ui.add(egui::TextEdit::singleline(value).desired_width(f32::INFINITY));
    }
    const COLOR_LABELS: [(sp_catalog::ColorLabel, &str); 6] = [
        (sp_catalog::ColorLabel::None, "No label"),
        (sp_catalog::ColorLabel::Red, "Red"),
        (sp_catalog::ColorLabel::Yellow, "Yellow"),
        (sp_catalog::ColorLabel::Green, "Green"),
        (sp_catalog::ColorLabel::Blue, "Blue"),
        (sp_catalog::ColorLabel::Purple, "Purple"),
    ];
    ui.add_space(theme::SPACE * 0.5);
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(crate::i18n::text("Color label"))
                .small()
                .color(theme::TEXT_SECONDARY),
        );
        let current = state.descriptive_metadata.color_label;
        let current_name = COLOR_LABELS
            .iter()
            .find(|(label, _)| *label == current)
            .map_or("No label", |(_, name)| name);
        egui::ComboBox::from_id_salt("metadata-color-label")
            .selected_text(crate::i18n::text(current_name))
            .show_ui(ui, |ui| {
                for (label, name) in COLOR_LABELS {
                    ui.horizontal(|ui| {
                        let (dot, _) =
                            ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                        match theme::label_color(label) {
                            Some(color) => {
                                ui.painter().circle_filled(dot.center(), 4.0, color);
                            }
                            None => {
                                ui.painter().circle_stroke(
                                    dot.center(),
                                    3.5,
                                    egui::Stroke::new(1.0, theme::TEXT_FAINT),
                                );
                            }
                        }
                        ui.selectable_value(
                            &mut state.descriptive_metadata.color_label,
                            label,
                            crate::i18n::text(name),
                        );
                    });
                }
            });
        if let Some(color) = theme::label_color(current) {
            let (dot, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
            ui.painter().circle_filled(dot.center(), 4.0, color);
        }
    });
    let mut has_location = state.descriptive_metadata.gps.is_some();
    if ui
        .checkbox(&mut has_location, crate::i18n::text("Location"))
        .changed()
    {
        state.descriptive_metadata.gps = has_location.then_some([0.0, 0.0]);
    }
    if let Some([lat, lon]) = &mut state.descriptive_metadata.gps {
        ui.horizontal(|ui| {
            ui.label(crate::i18n::text("Lat"));
            ui.add(egui::DragValue::new(lat).range(-90.0..=90.0).speed(0.0001));
            ui.label(crate::i18n::text("Lon"));
            ui.add(
                egui::DragValue::new(lon)
                    .range(-180.0..=180.0)
                    .speed(0.0001),
            );
        });
        gps_map(ui, *lat, *lon);
        if ui
            .small_button(crate::i18n::text("Clear location"))
            .clicked()
        {
            state.descriptive_metadata.gps = None;
        }
    }
    if state.descriptive_metadata != before {
        state.sidecar_dirty = true;
        state.last_edit_change = Some(std::time::Instant::now());
    }

    ui.add_space(theme::SPACE);
    ui.separator();
    ui.label(theme::section_text("Interop"));
    ui.add_space(theme::SPACE * 0.5);
    let toggle = ui.checkbox(
        &mut state.write_xmp,
        crate::i18n::text("Write XMP sidecars"),
    );
    toggle.on_hover_text(
        "Mirror ratings and flags into <name>.xmp next to the RAW so Lightroom, \
         Bridge and Phocus can read them. Edits always stay in ShopPhoto's own \
         .spedit sidecar. Existing XMP ratings are read regardless of this setting.",
    );
}

/// Read-only EXIF/capture information shared by the Library inspector and the
/// Metadata card at the bottom of the Develop inspector.
fn capture_metadata_panel(
    ui: &mut egui::Ui,
    path: Option<&std::path::Path>,
    meta: Option<&sp_core::ImageMeta>,
) {
    let Some(path) = path else {
        ui.label(
            egui::RichText::new(crate::i18n::text("No image selected"))
                .italics()
                .color(theme::TEXT_SECONDARY),
        );
        return;
    };
    ui.label(path.file_name().and_then(|n| n.to_str()).unwrap_or("—"));
    let Some(meta) = meta else {
        return;
    };
    let row = |ui: &mut egui::Ui, k: &str, v: String| {
        ui.horizontal(|ui| {
            ui.add_sized(
                [72.0, 16.0],
                egui::Label::new(
                    egui::RichText::new(k)
                        .size(11.0)
                        .color(theme::TEXT_SECONDARY),
                ),
            );
            ui.label(egui::RichText::new(v).size(11.0));
        });
    };
    ui.add_space(theme::SPACE * 0.5);
    row(
        ui,
        "Camera",
        format!("{} {}", meta.camera_make, meta.camera_model),
    );
    if let Some(lens) = &meta.lens_model {
        row(ui, "Lens", lens.clone());
    }
    if let Some(iso) = meta.iso {
        row(ui, "ISO", iso.to_string());
    }
    if let Some(s) = meta.shutter_s {
        let text = if s >= 1.0 {
            format!("{s:.1}s")
        } else if s > 0.0 {
            format!("1/{:.0}s", 1.0 / s)
        } else {
            "—".into()
        };
        row(ui, "Shutter", text);
    }
    if let Some(f) = meta.aperture_f {
        row(ui, "Aperture", format!("f/{f:.1}"));
    }
    if let Some(mm) = meta.focal_length_mm {
        row(ui, "Focal", format!("{mm:.0} mm"));
    }
    if let Some(t) = &meta.captured_at {
        row(ui, "Captured", t.clone());
    }
    if let Some([lat, lon]) = meta.gps {
        row(ui, "GPS", format!("{lat:.6}, {lon:.6}"));
        gps_map(ui, lat, lon);
    }
}

/// Compact offline location map used for both embedded capture GPS and
/// user-authored coordinates. It deliberately requires no network access.
fn gps_map(ui: &mut egui::Ui, lat: f64, lon: f64) {
    let (map, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 84.0), egui::Sense::hover());
    let painter = ui.painter_at(map);
    painter.rect_filled(map, 4.0, theme::BG_VIEWPORT);
    for fraction in [0.25, 0.5, 0.75] {
        let x = egui::lerp(map.left()..=map.right(), fraction);
        let y = egui::lerp(map.top()..=map.bottom(), fraction);
        painter.line_segment(
            [egui::pos2(x, map.top()), egui::pos2(x, map.bottom())],
            egui::Stroke::new(0.5, theme::HAIRLINE),
        );
        painter.line_segment(
            [egui::pos2(map.left(), y), egui::pos2(map.right(), y)],
            egui::Stroke::new(0.5, theme::HAIRLINE),
        );
    }
    // A deliberately small, coarse world outline keeps the metadata map
    // useful offline without shipping map tiles or contacting a service.
    const LAND: &[&[(f32, f32)]] = &[
        &[
            (-168.0, 66.0),
            (-140.0, 70.0),
            (-125.0, 49.0),
            (-117.0, 32.0),
            (-97.0, 18.0),
            (-82.0, 25.0),
            (-66.0, 47.0),
            (-58.0, 55.0),
            (-80.0, 72.0),
            (-110.0, 72.0),
            (-168.0, 66.0),
        ],
        &[
            (-81.0, 12.0),
            (-70.0, -5.0),
            (-55.0, -15.0),
            (-51.0, -34.0),
            (-67.0, -55.0),
            (-76.0, -35.0),
            (-81.0, 12.0),
        ],
        &[
            (-10.0, 36.0),
            (2.0, 51.0),
            (30.0, 60.0),
            (60.0, 72.0),
            (120.0, 56.0),
            (145.0, 45.0),
            (122.0, 20.0),
            (100.0, 8.0),
            (78.0, 8.0),
            (52.0, 28.0),
            (35.0, 30.0),
            (20.0, 38.0),
            (-10.0, 36.0),
        ],
        &[
            (-17.0, 35.0),
            (10.0, 37.0),
            (34.0, 30.0),
            (50.0, 12.0),
            (40.0, -12.0),
            (18.0, -35.0),
            (2.0, -30.0),
            (-10.0, 5.0),
            (-17.0, 35.0),
        ],
        &[
            (112.0, -11.0),
            (154.0, -25.0),
            (145.0, -43.0),
            (116.0, -35.0),
            (112.0, -11.0),
        ],
    ];
    let project = |longitude: f32, latitude: f32| {
        egui::pos2(
            map.left() + ((longitude + 180.0) / 360.0) * map.width(),
            map.top() + ((90.0 - latitude) / 180.0) * map.height(),
        )
    };
    for coast in LAND {
        painter.add(egui::Shape::line(
            coast.iter().map(|&(x, y)| project(x, y)).collect(),
            egui::Stroke::new(0.8, theme::TEXT_FAINT),
        ));
    }
    let marker = egui::pos2(
        map.left() + (((lon as f32).clamp(-180.0, 180.0) + 180.0) / 360.0) * map.width(),
        map.top() + ((90.0 - (lat as f32).clamp(-90.0, 90.0)) / 180.0) * map.height(),
    );
    painter.circle_filled(marker, 4.0, theme::ACCENT);
    painter.text(
        map.left_bottom() + egui::vec2(4.0, -4.0),
        egui::Align2::LEFT_BOTTOM,
        crate::i18n::text("Offline world map"),
        egui::FontId::proportional(9.0),
        theme::TEXT_FAINT,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dutch_inspector_search_finds_translated_controls() {
        crate::i18n::set_language(crate::i18n::Language::Dutch);
        assert!(card_matches("Basic", "belichting").1 > 0);
        assert!(card_matches("Detail", "onscherpte").1 > 0);
        assert!(label_matches("Exposure", "belichting"));
        crate::i18n::set_language(crate::i18n::Language::English);
    }

    #[test]
    fn units_format_with_signs_and_suffixes() {
        assert_eq!(Unit::Ev.format(1.3), "+1.30 EV");
        assert_eq!(Unit::Ev.format(0.0), "0.00 EV");
        assert_eq!(Unit::Signed(0).format(-13.5), "−14");
        assert_eq!(Unit::Signed(0).format(0.2), "0");
        assert_eq!(Unit::Kelvin.format(5500.0), "5500 K");
        assert_eq!(Unit::parse("−14"), Some(-14.0));
        assert_eq!(Unit::parse("5 500 K"), Some(5500.0));
        assert_eq!(Unit::parse("+1.30 EV"), Some(1.3));
    }

    #[test]
    fn auto_units_follow_the_range() {
        assert_eq!(Unit::Auto.resolve(&(-100.0..=100.0)), Unit::Signed(0));
        assert_eq!(Unit::Auto.resolve(&(0.0..=100.0)), Unit::Plain(0));
        assert_eq!(Unit::Auto.resolve(&(0.5..=3.0)), Unit::Plain(2));
        assert_eq!(Unit::Auto.resolve(&(-0.01..=0.01)), Unit::Signed(4));
    }

    #[test]
    fn log_scale_gives_the_daylight_band_room() {
        let scale = Scale::new(&(2000.0..=50000.0), true);
        let daylight = scale.to_t(10_000.0) - scale.to_t(2_500.0);
        assert!(daylight > 0.4, "daylight band {daylight}");
        assert!((scale.value_at(scale.to_t(6500.0)) - 6500.0).abs() < 0.5);
        let linear = Scale::new(&(2000.0..=50000.0), false);
        assert!(linear.to_t(10_000.0) - linear.to_t(2_500.0) < 0.17);
    }

    #[test]
    fn timestamps_render_as_utc_dates() {
        assert_eq!(utc_timestamp(0), "1970-01-01 00:00 UTC");
        assert_eq!(utc_timestamp(1_790_512_345), "2026-09-27 12:32 UTC");
        assert_eq!(relative_time(100, 120), "just now");
        assert_eq!(relative_time(0, 600), "10 min ago");
    }
}
