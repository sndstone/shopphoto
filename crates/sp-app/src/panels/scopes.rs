//! Persistent measurement annotations and working-space scopes.
use crate::theme;
use sp_core::{
    analysis::{ScopeData, ScopeMode},
    source::RasterImage,
    EditState,
};

pub(super) fn controls(
    ui: &mut egui::Ui,
    edit: &mut EditState,
    picker: &mut bool,
    enabled: &mut bool,
    frame: Option<&RasterImage>,
    pending: bool,
) {
    ui.collapsing(crate::i18n::text("Samples & scopes"), |ui| {
        ui.horizontal(|ui| {
            if ui
                .selectable_label(*picker, crate::i18n::text("Add sample point"))
                .clicked()
            {
                *picker = !*picker;
            }
            ui.checkbox(enabled, crate::i18n::text("Show scopes"));
        });
        if pending {
            ui.label(
                egui::RichText::new(crate::i18n::text("Updating measurements…"))
                    .small()
                    .color(theme::TEXT_FAINT),
            );
        }
        ui.label(
            egui::RichText::new(crate::i18n::text(
                "Area samples from the developed proxy. Drag numbered pins to move them.",
            ))
            .small()
            .color(theme::TEXT_SECONDARY),
        );
        let mut remove = None;
        for (index, point) in edit.sample_points.iter_mut().enumerate() {
            ui.push_id(index, |ui| {
                ui.horizontal(|ui| {
                    ui.label(format!("{}", index + 1));
                    ui.text_edit_singleline(&mut point.name);
                    if ui.small_button(crate::i18n::text("×")).clicked() {
                        remove = Some(index);
                    }
                });
                if let Some(sample) =
                    frame.and_then(|frame| sp_core::analysis::sample(frame, point.position))
                {
                    let rgb = sample.rgb.0;
                    let lab = sample.lab.0;
                    ui.label(
                        egui::RichText::new(format!(
                            "RGB {:.3}  {:.3}  {:.3}",
                            rgb[0], rgb[1], rgb[2]
                        ))
                        .small()
                        .monospace(),
                    );
                    ui.label(
                        egui::RichText::new(format!(
                            "Lab D65 {:.1}  {:.1}  {:.1} · {}",
                            lab[0],
                            lab[1],
                            lab[2],
                            sample
                                .ev_from_middle_gray
                                .map_or_else(|| "−∞ EV".into(), |ev| format!("{ev:+.2} EV"))
                        ))
                        .small()
                        .monospace(),
                    );
                }
            });
        }
        if let Some(index) = remove {
            edit.sample_points.remove(index);
        }
        if *enabled {
            let id = ui.id().with("scope_controls");
            let (mut mode, mut ceiling) = ui
                .data_mut(|data| data.get_temp::<(u8, f32)>(id))
                .unwrap_or((0, 0.0));
            ui.horizontal_wrapped(|ui| {
                for (value, name) in [(0, "Waveform"), (1, "RGB parade"), (2, "Lab vectorscope")] {
                    ui.selectable_value(&mut mode, value, name);
                }
            });
            if mode != 2 {
                ui.add(
                    egui::Slider::new(&mut ceiling, 0.0..=4.0)
                        .text(crate::i18n::text("Top, EV above white"))
                        .step_by(1.0),
                );
            }
            ui.data_mut(|data| data.insert_temp(id, (mode, ceiling)));
            if let Some(frame) = frame {
                let key = (
                    frame.linear_rec2020.as_ptr() as usize,
                    mode,
                    ceiling.to_bits(),
                );
                let cache_id = ui.id().with("scope_texture");
                let cached = ui.data_mut(|data| {
                    data.get_temp::<((usize, u8, u32), egui::TextureHandle)>(cache_id)
                });
                let texture = if let Some((_, texture)) = cached.filter(|(old, _)| *old == key) {
                    texture
                } else {
                    let scope = ScopeData::build(
                        frame,
                        match mode {
                            1 => ScopeMode::Parade,
                            2 => ScopeMode::Vectorscope,
                            _ => ScopeMode::Waveform,
                        },
                        ceiling,
                    );
                    let maximum = scope
                        .counts
                        .iter()
                        .flat_map(|values| values.iter())
                        .copied()
                        .max()
                        .unwrap_or(1)
                        .max(1) as f32;
                    let colors = [theme::LABEL_RED, theme::LABEL_GREEN, theme::LABEL_BLUE];
                    let pixels = scope
                        .counts
                        .iter()
                        .map(|values| {
                            let mut rgb = [0.0f32; 3];
                            for (channel, count) in values.iter().enumerate() {
                                let intensity =
                                    ((*count as f32 + 1.0).ln() / (maximum + 1.0).ln()).sqrt();
                                let color = if mode == 1 {
                                    colors[channel]
                                } else if mode == 2 {
                                    theme::ACCENT
                                } else {
                                    theme::TEXT_PRIMARY
                                };
                                for (out, value) in
                                    rgb.iter_mut().zip([color.r(), color.g(), color.b()])
                                {
                                    *out += f32::from(value) * intensity;
                                }
                            }
                            egui::Color32::from_rgb(
                                rgb[0].min(255.0) as u8,
                                rgb[1].min(255.0) as u8,
                                rgb[2].min(255.0) as u8,
                            )
                        })
                        .collect();
                    let texture = ui.ctx().load_texture(
                        "Photographic scope",
                        egui::ColorImage::new([scope.width, scope.height], pixels),
                        egui::TextureOptions::NEAREST,
                    );
                    ui.data_mut(|data| data.insert_temp(cache_id, (key, texture.clone())));
                    texture
                };
                let width = ui.available_width().min(380.0);
                let size = egui::vec2(width, if mode == 2 { width } else { width * 0.5 });
                let response = ui.add(egui::Image::new(&texture).fit_to_exact_size(size));
                let rect = response.rect;
                let painter = ui.painter();
                for fraction in [0.25, 0.5, 0.75] {
                    let y = egui::lerp(rect.top()..=rect.bottom(), fraction);
                    painter.line_segment(
                        [egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)],
                        egui::Stroke::new(0.5, theme::HAIRLINE),
                    );
                }
                if mode == 2 {
                    painter.line_segment(
                        [rect.center_top(), rect.center_bottom()],
                        egui::Stroke::new(0.5, theme::HAIRLINE),
                    );
                }
                ui.label(
                    egui::RichText::new(crate::i18n::text(if mode == 2 {
                        "Whole image · CIE Lab D65 a*/b*"
                    } else {
                        "Whole image · scene-linear Rec.2020"
                    }))
                    .small()
                    .color(theme::TEXT_SECONDARY),
                );
            }
        }
    });
}
