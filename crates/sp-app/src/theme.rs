//! Central theme: the ONLY place UI colors, spacing, and fonts are defined
//! (BUILD.md §5.1). No per-widget ad-hoc colors anywhere else in `sp-app`.
//!
//! The look: deep neutral surfaces with a subtle cool tint, one restrained
//! blue accent, generous rounding, and card-based panel groups — modern
//! photo-tool styling (Lightroom/Capture One class), not stock egui.

use egui::{Color32, CornerRadius, Shadow, Stroke, Visuals};

// ---- Design tokens ----
/// Window backdrop (deepest surface).
#[allow(dead_code)] // reserved token for additional floating windows/dialogs
pub const BG_WINDOW: Color32 = Color32::from_rgb(0x14, 0x15, 0x17);
/// Side/top panel fill.
pub const BG_PANEL: Color32 = Color32::from_rgb(0x1B, 0x1D, 0x20);
/// Card fill inside panels (tool groups, histogram).
pub const BG_CARD: Color32 = Color32::from_rgb(0x22, 0x25, 0x29);
/// Raised interactive fills (buttons, inputs).
pub const BG_RAISED: Color32 = Color32::from_rgb(0x2A, 0x2E, 0x33);
/// Hover fill.
pub const BG_HOVER: Color32 = Color32::from_rgb(0x32, 0x37, 0x3C);
pub const HAIRLINE: Color32 = Color32::from_rgb(0x30, 0x34, 0x39);
pub const TEXT_PRIMARY: Color32 = Color32::from_rgb(0xEC, 0xED, 0xEE);
pub const TEXT_SECONDARY: Color32 = Color32::from_rgb(0x9C, 0xA3, 0xAB);
/// Muted label text (section headers).
pub const TEXT_FAINT: Color32 = Color32::from_rgb(0x8A, 0x91, 0x9B);
pub const ACCENT: Color32 = Color32::from_rgb(0x53, 0x94, 0xFF);
/// Accent text on dark fills (changed slider values, links).
pub const ACCENT_HOVER: Color32 = Color32::from_rgb(0x74, 0xA9, 0xFF);
/// Text drawn on top of an accent-filled control.
pub const ON_ACCENT: Color32 = Color32::from_rgb(0x0C, 0x12, 0x1E);
pub const WARNING: Color32 = Color32::from_rgb(0xE8, 0xA8, 0x4C);
pub const ERROR: Color32 = Color32::from_rgb(0xE8, 0x5C, 0x48);
/// Pure neutral viewport backdrop so it never biases color judgement.
pub const BG_VIEWPORT: Color32 = Color32::from_rgb(0x10, 0x11, 0x12);
/// Floating viewport toolbars and banners (translucent over the photo).
pub const BG_HUD: Color32 = Color32::from_rgba_premultiplied(0x12, 0x13, 0x15, 0xD8);
/// Slider rail and the neutral-point tick.
pub const SLIDER_RAIL: Color32 = Color32::from_rgb(0x33, 0x38, 0x3E);
pub const SLIDER_TICK: Color32 = Color32::from_rgb(0x5A, 0x61, 0x6A);
pub const SLIDER_HANDLE: Color32 = Color32::from_rgb(0xF3, 0xF4, 0xF5);
/// Positive confirmation (pick flag, finished job, applied state).
pub const SUCCESS: Color32 = Color32::from_rgb(0x5F, 0xCF, 0x86);
/// Star ratings.
pub const STAR: Color32 = Color32::from_rgb(0xF0, 0xC2, 0x4A);
/// Clipping indicators, matching the viewport clipping overlays.
pub const CLIP_SHADOW: Color32 = Color32::from_rgb(0x5B, 0x8C, 0xFF);
pub const CLIP_HIGHLIGHT: Color32 = Color32::from_rgb(0xFF, 0x5A, 0x4A);
/// Color labels (Red, Yellow, Green, Blue, Purple).
pub const LABEL_RED: Color32 = Color32::from_rgb(0xE5, 0x53, 0x4B);
pub const LABEL_YELLOW: Color32 = Color32::from_rgb(0xE3, 0xB3, 0x41);
pub const LABEL_GREEN: Color32 = Color32::from_rgb(0x57, 0xAB, 0x5A);
pub const LABEL_BLUE: Color32 = Color32::from_rgb(0x53, 0x9B, 0xF5);
pub const LABEL_PURPLE: Color32 = Color32::from_rgb(0xB0, 0x83, 0xF0);

/// Swatch color of a catalog color label (`None` for no label).
#[must_use]
pub fn label_color(label: sp_catalog::ColorLabel) -> Option<Color32> {
    match label {
        sp_catalog::ColorLabel::None => None,
        sp_catalog::ColorLabel::Red => Some(LABEL_RED),
        sp_catalog::ColorLabel::Yellow => Some(LABEL_YELLOW),
        sp_catalog::ColorLabel::Green => Some(LABEL_GREEN),
        sp_catalog::ColorLabel::Blue => Some(LABEL_BLUE),
        sp_catalog::ColorLabel::Purple => Some(LABEL_PURPLE),
    }
}

/// Base spacing unit; all margins/paddings are multiples of this.
pub const SPACE: f32 = 8.0;
pub const RADIUS_PANEL: u8 = 8;
pub const RADIUS_CONTROL: u8 = 5;

/// Fixed chrome dimensions (BUILD.md §5.2).
pub const TOP_BAR_H: f32 = 48.0;
pub const LEFT_RAIL_W: f32 = 236.0;
pub const RIGHT_PANEL_W: f32 = 356.0;
pub const FILMSTRIP_H: f32 = 100.0;

/// Fixed width of develop-slider labels; sliders flex to the remainder.
pub const SLIDER_LABEL_W: f32 = 76.0;
/// Fixed width of develop-slider numeric fields. Wide enough for
/// `50000 K` or `−5.00 EV` without allowing the formatted value to reflow
/// its parent row.
pub const SLIDER_VALUE_W: f32 = 64.0;
/// Reserved slot for a slider's hover reset button.
pub const SLIDER_RESET_W: f32 = 14.0;
/// Height of the develop histogram. Pinned above the inspector scroll.
pub const HISTOGRAM_H: f32 = 108.0;

/// Apply the ShopPhoto dark theme to the egui context. Called once at
/// startup; idempotent.
pub fn apply(ctx: &egui::Context) {
    // ShopPhoto is dark-only in v1 (BUILD.md §5.1). Without this, egui
    // follows the OS theme and renders the default LIGHT style on
    // light-mode systems, ignoring every token below.
    ctx.set_theme(egui::ThemePreference::Dark);
    let mut visuals = Visuals::dark();

    visuals.panel_fill = BG_PANEL;
    visuals.window_fill = BG_CARD;
    visuals.extreme_bg_color = BG_VIEWPORT;
    visuals.faint_bg_color = BG_CARD;
    visuals.code_bg_color = BG_CARD;

    visuals.override_text_color = Some(TEXT_PRIMARY);
    visuals.hyperlink_color = ACCENT;
    visuals.selection.bg_fill = ACCENT.linear_multiply(0.35);
    visuals.selection.stroke = Stroke::new(1.0, ACCENT);

    // Sliders: accent trail up to the handle, clean round handle.
    visuals.slider_trailing_fill = true;

    visuals.widgets.noninteractive.bg_fill = BG_PANEL;
    visuals.widgets.noninteractive.weak_bg_fill = BG_PANEL;
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, HAIRLINE);
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT_SECONDARY);

    visuals.widgets.inactive.bg_fill = BG_RAISED;
    visuals.widgets.inactive.weak_bg_fill = BG_RAISED;
    // A visible boundary remains when keyboard focus moves onto an otherwise
    // idle control; active/open focus receives the stronger accent stroke.
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, HAIRLINE);
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, TEXT_PRIMARY);

    visuals.widgets.hovered.bg_fill = BG_HOVER;
    visuals.widgets.hovered.weak_bg_fill = BG_HOVER;
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, Color32::from_rgb(0x41, 0x47, 0x4F));
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.2, TEXT_PRIMARY);
    visuals.widgets.hovered.expansion = 1.0;

    visuals.widgets.active.bg_fill = ACCENT.linear_multiply(0.55);
    visuals.widgets.active.weak_bg_fill = ACCENT.linear_multiply(0.55);
    visuals.widgets.active.bg_stroke = Stroke::new(1.0, ACCENT);
    visuals.widgets.active.fg_stroke = Stroke::new(1.2, TEXT_PRIMARY);

    visuals.widgets.open.bg_fill = BG_RAISED;
    visuals.widgets.open.weak_bg_fill = BG_RAISED;
    visuals.widgets.open.bg_stroke = Stroke::new(1.0, HAIRLINE);

    for w in [
        &mut visuals.widgets.noninteractive,
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
        &mut visuals.widgets.open,
    ] {
        w.corner_radius = CornerRadius::same(RADIUS_CONTROL);
    }
    visuals.window_corner_radius = CornerRadius::same(RADIUS_PANEL);
    visuals.menu_corner_radius = CornerRadius::same(RADIUS_CONTROL + 2);
    visuals.popup_shadow = Shadow {
        offset: [0, 6],
        blur: 24,
        spread: 0,
        color: Color32::from_black_alpha(120),
    };
    visuals.window_shadow = visuals.popup_shadow;

    ctx.set_visuals(visuals);

    ctx.all_styles_mut(|style| {
        style.spacing.item_spacing = egui::vec2(SPACE, 7.0);
        style.spacing.window_margin = egui::Margin::same(10);
        style.spacing.button_padding = egui::vec2(11.0, 6.0);
        style.spacing.interact_size.y = 28.0;
        style.spacing.combo_width = 120.0;
        style.spacing.scroll = egui::style::ScrollStyle::thin();
    });
}

/// Card frame for tool groups and panel widgets.
pub fn card_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(BG_CARD)
        .stroke(Stroke::new(1.0, HAIRLINE))
        .corner_radius(RADIUS_PANEL as f32 - 2.0)
        .inner_margin(egui::Margin::symmetric(12, 9))
}

/// Small uppercase section label.
pub fn section_text(text: &str) -> egui::RichText {
    egui::RichText::new(text.to_uppercase())
        .size(10.5)
        .color(TEXT_FAINT)
        .strong()
}

/// Selection overlay tint. Deliberately not the blue accent, which vanishes
/// over skies and water.
pub const MASK_OVERLAY: Color32 = Color32::from_rgb(0xFF, 0x3D, 0x6E);

/// Matching colour for an AI element candidate in the rail and the viewport.
pub fn candidate_color(index: usize) -> Color32 {
    const PALETTE: [Color32; 6] = [
        MASK_OVERLAY,
        LABEL_YELLOW,
        Color32::from_rgb(0x4C, 0xC9, 0xC9),
        LABEL_GREEN,
        LABEL_PURPLE,
        ACCENT,
    ];
    PALETTE[index % PALETTE.len()]
}

/// Warning triangle painted as a shape: egui's default fonts have no ⚠.
pub fn paint_warning_icon(painter: &egui::Painter, center: egui::Pos2, size: f32) {
    let top = center - egui::vec2(0.0, size * 0.55);
    let left = center + egui::vec2(-size * 0.62, size * 0.45);
    let right = center + egui::vec2(size * 0.62, size * 0.45);
    painter.add(egui::Shape::convex_polygon(
        vec![top, right, left],
        WARNING,
        egui::Stroke::NONE,
    ));
    painter.line_segment(
        [
            center - egui::vec2(0.0, size * 0.2),
            center + egui::vec2(0.0, size * 0.12),
        ],
        egui::Stroke::new((size * 0.12).max(1.0), BG_PANEL),
    );
    painter.circle_filled(
        center + egui::vec2(0.0, size * 0.28),
        (size * 0.07).max(0.8),
        BG_PANEL,
    );
}

/// Small folder outline for sidebar rows.
pub fn paint_folder_icon(painter: &egui::Painter, center: egui::Pos2, color: Color32) {
    let body = egui::Rect::from_center_size(center + egui::vec2(0.0, 1.0), egui::vec2(12.0, 8.5));
    let tab = egui::Rect::from_min_size(body.min - egui::vec2(0.0, 2.0), egui::vec2(5.0, 2.5));
    let stroke = egui::Stroke::new(1.1, color);
    painter.rect_stroke(tab, 1.0, stroke, egui::StrokeKind::Middle);
    painter.rect_stroke(body, 1.5, stroke, egui::StrokeKind::Middle);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn relative_luminance(color: Color32) -> f32 {
        let channel = |value: u8| {
            let encoded = f32::from(value) / 255.0;
            if encoded <= 0.04045 {
                encoded / 12.92
            } else {
                ((encoded + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(color.r()) + 0.7152 * channel(color.g()) + 0.0722 * channel(color.b())
    }

    fn contrast(a: Color32, b: Color32) -> f32 {
        let (lighter, darker) = {
            let a = relative_luminance(a);
            let b = relative_luminance(b);
            if a >= b {
                (a, b)
            } else {
                (b, a)
            }
        };
        (lighter + 0.05) / (darker + 0.05)
    }

    #[test]
    fn editor_text_tokens_keep_normal_text_contrast() {
        for (name, foreground, background) in [
            ("primary on panel", TEXT_PRIMARY, BG_PANEL),
            ("secondary on card", TEXT_SECONDARY, BG_CARD),
            ("faint on card", TEXT_FAINT, BG_CARD),
            ("accent button", ON_ACCENT, ACCENT),
        ] {
            let ratio = contrast(foreground, background);
            assert!(ratio >= 4.5, "{name} contrast is only {ratio:.2}:1");
        }
    }
}
