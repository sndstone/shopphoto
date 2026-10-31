//! Export dialog, Reference Match wizard, overlays and modal dialogs.

use super::*;

fn export_working_dims(state: &AppState) -> Option<(u32, u32)> {
    let (width, height) = state.active_size?;
    let factor = state.edit.resolution.mode.factor().max(1);
    Some(((width / factor).max(1), (height / factor).max(1)))
}

pub(super) fn export_geometry_dims(state: &AppState) -> Option<(u32, u32)> {
    let (width, height) = export_working_dims(state)?;
    let [width, height] =
        sp_core::geometry::GeometryMap::new([width, height], state.edit.geometry).output_size;
    Some((width, height))
}

/// Known installed checkpoints and native bundles have a verified scale.
/// Arbitrary checkpoints are inspected by inference; guessing from an `x4`
/// substring would give a misleading output-size preview.
fn upscale_model_scale(path: &std::path::Path) -> Option<u32> {
    if path.is_dir() {
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(path.join("manifest.json")).ok()?).ok()?;
        return manifest["scale"]
            .as_u64()
            .and_then(|value| u32::try_from(value).ok())
            .filter(|scale| [2, 4].contains(scale));
    }
    match path.file_name()?.to_str()? {
        "SwinIR-M-x2.pth" | "001_classicalSR_DF2K_s64w8_SwinIR-M_x2.pth" => Some(2),
        "SwinIR-M-x4.pth" | "001_classicalSR_DF2K_s64w8_SwinIR-M_x4.pth" => Some(4),
        _ => None,
    }
}

pub(super) fn default_upscale_model(scale: u32) -> Option<std::path::PathBuf> {
    if let Some(native) = crate::app::native_enhance::bundle(&format!("swinir-{scale}x")) {
        return Some(native);
    }
    let root = dirs::data_local_dir()?.join("shopphoto/enhance-models");
    let native = root.join(format!("native/swinir-x{scale}"));
    if native.join("manifest.json").is_file() {
        return Some(native);
    }
    let checkpoint = root.join(format!("SwinIR-M-x{scale}.pth"));
    checkpoint.is_file().then_some(checkpoint)
}

fn export_enhanced_dims(state: &AppState) -> Option<(u32, u32)> {
    let (width, height) = export_geometry_dims(state)?;
    let scale = state
        .export_settings
        .upscale_model
        .as_deref()
        .map_or(Some(1), upscale_model_scale)?;
    Some((width.checked_mul(scale)?, height.checked_mul(scale)?))
}

fn megapixels((width, height): (u32, u32)) -> f64 {
    f64::from(width) * f64::from(height) / 1.0e6
}

fn compact_dimensions(dimensions: (u32, u32)) -> String {
    format!(
        "{} × {} · {:.1} MP",
        dimensions.0,
        dimensions.1,
        megapixels(dimensions)
    )
}

pub(super) fn recommended_export_size(dimensions: (u32, u32)) -> sp_export::SizeMode {
    // High-resolution files default to half of their actual width and height
    // (100 MP becomes about 25 MP). Normal 24 MP files remain full size.
    // Percent is deliberately relative: it follows every file's decoded
    // dimensions instead of freezing a megapixel value from an earlier file.
    if megapixels(dimensions) > 35.0 {
        sp_export::SizeMode::Percent(50.0)
    } else {
        sp_export::SizeMode::Original
    }
}

fn targets_hdr(state: &AppState) -> bool {
    state.edit.hdr.enabled && state.edit.hdr.target == sp_core::edit::HdrTarget::Hdr
}

fn export_section_heading(ui: &mut egui::Ui, title: &str) {
    ui.add_space(theme::SPACE * 0.5);
    ui.label(
        egui::RichText::new(crate::i18n::text(title))
            .size(10.5)
            .strong()
            .color(theme::TEXT_FAINT),
    );
}

/// Phase-5 export dialog. Native folder picking and GPU work are handled by
/// the app logic; the modal only edits serializable settings.
pub fn export_dialog(ctx: &egui::Context, state: &mut AppState) {
    if !state.export_dialog_open {
        return;
    }
    if !state.export_defaults_applied {
        if let Some(dimensions) = export_geometry_dims(state) {
            state.export_settings.size = recommended_export_size(dimensions);
        }
        state.export_defaults_applied = true;
    }
    if targets_hdr(state) && !state.export_hdr_defaults_applied {
        // Ultra HDR remains a normal JPEG in non-HDR software, making it the
        // least surprising default. Advanced users can still choose PQ AVIF,
        // 16-bit PNG or JPEG XL after this one-time adjustment.
        state.export_settings.format = sp_export::ExportFormat::UltraHdr { quality: 90 };
        state.export_settings.color_space = sp_export::ExportColorSpace::Srgb;
        state.export_preset_name.clear();
        state.export_hdr_defaults_applied = true;
    } else if !targets_hdr(state) {
        state.export_hdr_defaults_applied = false;
    }
    let height = (ctx.content_rect().height() - 80.0).clamp(360.0, 760.0);
    let id = egui::Id::new("export-dialog");
    egui::Modal::new(id).area(egui::Modal::default_area(id).default_size(egui::vec2(560.0,height))).show(ctx, |ui| {
      ui.set_width(540.0);
      egui::ScrollArea::vertical().max_height(height-90.0).auto_shrink([false,false]).show(ui, |ui| {
        use sp_export::{ExportColorSpace, ExportFormat, OutputSharpening, SizeMode, TiffDepth};
        ui.set_width(540.0);
        ui.heading(crate::i18n::text("Export photo"));
        let source_name = state
            .open_image
            .as_ref()
            .and_then(|path| path.file_name())
            .map_or_else(|| "Current photo".into(), |name| name.to_string_lossy());
        let working = export_working_dims(state)
            .map(compact_dimensions)
            .unwrap_or_else(|| "Resolution unavailable".into());
        egui::Frame::new()
            .fill(theme::BG_RAISED)
            .corner_radius(theme::RADIUS_CONTROL)
            .inner_margin(egui::Margin::symmetric(10, 8))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(source_name).strong());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            egui::RichText::new(working)
                                .small()
                                .color(theme::TEXT_SECONDARY),
                        );
                    });
                });
            });
        ui.add_space(theme::SPACE);

        ui.horizontal(|ui| {
            ui.label(crate::i18n::text("Preset"));
            egui::ComboBox::from_id_salt("export-preset")
                .width(230.0)
                .selected_text(if state.export_preset_name.is_empty() {
                    "Custom"
                } else {
                    &state.export_preset_name
                })
                .show_ui(ui, |ui| {
                    for preset in &state.export_presets {
                        if ui.button(&preset.name).clicked() {
                            let destination = state.export_settings.destination.clone();
                            state.export_preset_name.clone_from(&preset.name);
                            state.export_settings.clone_from(&preset.settings);
                            // Built-in presets intentionally have no folder;
                            // keep Downloads (or the user's last override).
                            if state.export_settings.destination.as_os_str().is_empty() {
                                state.export_settings.destination = destination;
                            }
                        }
                    }
                });
            ui.menu_button(crate::i18n::text("Save as…"), |ui| {
                ui.set_min_width(220.0);
                ui.label(crate::i18n::text("Preset name"));
                ui.text_edit_singleline(&mut state.export_preset_name);
                if ui
                    .add_enabled(
                        !state.export_preset_name.trim().is_empty(),
                        egui::Button::new(crate::i18n::text("Save preset")),
                    )
                    .clicked()
                {
                    state.export_preset_save_requested = true;
                    ui.close();
                }
            });
        });

        ui.separator();
        export_section_heading(ui, "FILE FORMAT");
        if targets_hdr(state) {
            let preserves_hdr = matches!(
                state.export_settings.format,
                ExportFormat::UltraHdr { .. }
                    | ExportFormat::Avif { .. }
                    | ExportFormat::Png { sixteen_bit: true }
                    | ExportFormat::JpegXl { .. }
            );
            ui.label(
                egui::RichText::new(crate::i18n::text(if preserves_hdr {
                    "HDR target detected · This format preserves the HDR rendition."
                } else {
                    "This format exports SDR. Choose Ultra HDR, AVIF, 16-bit PNG or JPEG XL for HDR."
                }))
                .small()
                .color(if preserves_hdr {
                    theme::ACCENT
                } else {
                    theme::WARNING
                }),
            );
        }

        let format_name = match state.export_settings.format {
            ExportFormat::Jpeg { .. } => "JPEG",
            ExportFormat::Tiff { .. } => "TIFF",
            ExportFormat::Png { .. } => "PNG",
            ExportFormat::Avif { .. } => "AVIF",
            ExportFormat::UltraHdr { .. } => "Ultra HDR JPEG",
            ExportFormat::JpegXl { .. } => "JPEG XL",
            ExportFormat::WebP { .. } => "WebP",
        };
        ui.horizontal(|ui| {
            ui.label(crate::i18n::text("Format"));
            egui::ComboBox::from_id_salt("export-format")
                .selected_text(format_name)
                .show_ui(ui, |ui| {
                    if ui.selectable_label(format_name == "JPEG", crate::i18n::text("JPEG")).clicked() {
                        state.export_settings.format = ExportFormat::Jpeg {
                            quality: 90,
                            chroma_444: false,
                        };
                    }
                    if ui.selectable_label(format_name == "TIFF", crate::i18n::text("TIFF")).clicked() {
                        state.export_settings.format = ExportFormat::Tiff {
                            bit_depth: TiffDepth::Sixteen,
                            compressed: true,
                        };
                    }
                    if ui.selectable_label(format_name == "PNG", crate::i18n::text("PNG")).clicked() {
                        state.export_settings.format = ExportFormat::Png { sixteen_bit: true };
                    }
                    if ui.selectable_label(format_name == "AVIF", crate::i18n::text("AVIF")).clicked() {
                        state.export_settings.format = ExportFormat::Avif { quality: 80 };
                    }
                    if ui
                        .selectable_label(format_name == "Ultra HDR JPEG", crate::i18n::text("Ultra HDR JPEG"))
                        .on_hover_text(
                            "A normal .jpg that also carries a gain map: HDR-aware viewers \
                             (Android, iOS 18+, Chrome, Safari) show the extra highlights, \
                             everything else shows the SDR image. Renders twice, so it takes \
                             about twice as long.",
                        )
                        .clicked()
                    {
                        state.export_settings.format = ExportFormat::UltraHdr { quality: 90 };
                    }
                    if ui
                        .selectable_label(format_name == "JPEG XL", crate::i18n::text("JPEG XL"))
                        .clicked()
                    {
                        state.export_settings.format = ExportFormat::JpegXl {
                            quality: 90,
                            lossless: false,
                        };
                    }
                    if ui
                        .selectable_label(format_name == "WebP", crate::i18n::text("WebP"))
                        .on_hover_text(crate::i18n::text("Lossless WebP with the ICC profile and EXIF embedded."))
                        .clicked()
                    {
                        state.export_settings.format = ExportFormat::WebP { lossless: true };
                    }
                });
        });
        match &mut state.export_settings.format {
            ExportFormat::Jpeg {
                quality,
                chroma_444,
            } => {
                ui.add(egui::Slider::new(quality, 1..=100).text(crate::i18n::text("Quality")));
                ui.checkbox(chroma_444, crate::i18n::text("4:4:4 chroma (best color detail)"));
            }
            ExportFormat::Avif { quality } | ExportFormat::UltraHdr { quality } => {
                ui.add(egui::Slider::new(quality, 1..=100).text(crate::i18n::text("Quality")));
            }
            ExportFormat::Tiff {
                bit_depth,
                compressed,
            } => {
                ui.horizontal(|ui| {
                    ui.label(crate::i18n::text("Depth"));
                    ui.radio_value(bit_depth, TiffDepth::Eight, crate::i18n::text("8-bit"));
                    ui.radio_value(bit_depth, TiffDepth::Sixteen, crate::i18n::text("16-bit"));
                    ui.checkbox(compressed, crate::i18n::text("ZIP"));
                });
            }
            ExportFormat::Png { sixteen_bit } => {
                ui.checkbox(sixteen_bit, crate::i18n::text("16-bit"));
            }
            ExportFormat::WebP { lossless } => {
                ui.checkbox(lossless, crate::i18n::text("Lossless WebP"));
                if !*lossless { ui.add(egui::Slider::new(&mut state.export_settings.webp_quality,1..=100).text(crate::i18n::text("Quality"))); }
                ui.label(
                    egui::RichText::new(crate::i18n::text("8-bit RGB · ICC, EXIF and XMP embedded"))
                        .size(10.0)
                        .color(theme::TEXT_FAINT),
                );
            }
            ExportFormat::JpegXl { quality, lossless } => {
                ui.checkbox(lossless, crate::i18n::text("Mathematically lossless"));
                ui.add_enabled_ui(!*lossless, |ui| {
                    ui.add(egui::Slider::new(quality, 1..=100).text(crate::i18n::text("Quality")));
                });
                ui.label(
                    egui::RichText::new(crate::i18n::text("16-bit RGB · ICC for SDR · Rec.2100 PQ for HDR"))
                        .size(10.0)
                        .color(theme::TEXT_FAINT),
                );
            }
        }
        ui.horizontal(|ui| {
            ui.label(crate::i18n::text("Color"));
            ui.radio_value(
                &mut state.export_settings.color_space,
                ExportColorSpace::Srgb,
                crate::i18n::text("sRGB"),
            );
            ui.radio_value(
                &mut state.export_settings.color_space,
                ExportColorSpace::DisplayP3,
                crate::i18n::text("Display P3"),
            );
            ui.radio_value(
                &mut state.export_settings.color_space,
                ExportColorSpace::Rec2020,
                crate::i18n::text("Rec.2020"),
            );
            ui.radio_value(
                &mut state.export_settings.color_space,
                ExportColorSpace::AdobeRgb,
                crate::i18n::text("Adobe RGB (1998)"),
            );
        });

        ui.separator();
        export_section_heading(ui, "OUTPUT SIZE");
        ui.collapsing(crate::i18n::text("Output ICC profile and file-size target"), |ui| {
            use sp_export::{OutputProfile, OutputIntent};
            ui.horizontal_wrapped(|ui| {
                if ui.selectable_label(state.export_settings.output_profile.is_none(),crate::i18n::text("Standard RGB profile")).clicked(){state.export_settings.output_profile=None;}
                if ui.selectable_label(state.export_settings.output_profile==Some(OutputProfile::ProPhotoRgb),crate::i18n::text("ProPhoto RGB")).clicked(){state.export_settings.output_profile=Some(OutputProfile::ProPhotoRgb);}
                if ui.button(crate::i18n::text("Custom RGB ICC…")).clicked(){state.export_profile_requested=true;}
            });
            if let Some(profile)=&state.export_settings.output_profile{
                ui.label(match profile{OutputProfile::ProPhotoRgb=>"ProPhoto RGB (D50, gamma 1.8)".into(),OutputProfile::Custom(p)=>p.display().to_string()});
                egui::ComboBox::from_id_salt("output-intent").selected_text(format!("{:?}",state.export_settings.output_intent)).show_ui(ui,|ui|{
                    for (value,label) in [(OutputIntent::Relative,"Relative colorimetric"),(OutputIntent::Perceptual,"Perceptual"),(OutputIntent::Saturation,"Saturation"),(OutputIntent::Absolute,"Absolute colorimetric")]{ui.selectable_value(&mut state.export_settings.output_intent,value,crate::i18n::text(label));}
                });
                ui.checkbox(&mut state.export_settings.output_black_point_compensation,crate::i18n::text("Black-point compensation"));
                ui.label(crate::i18n::text("SDR output only. Use 16-bit TIFF, PNG or JPEG XL for wide-gamut delivery."));
            }
            let supported=matches!(state.export_settings.format,ExportFormat::Jpeg{..}|ExportFormat::Avif{..}|ExportFormat::WebP{lossless:false}|ExportFormat::JpegXl{lossless:false,..});
            if !supported{state.export_settings.file_size_limit=None;}
            let mut enabled=state.export_settings.file_size_limit.is_some();
            if ui.add_enabled(supported,egui::Checkbox::new(&mut enabled,crate::i18n::text("Limit file size"))).changed(){state.export_settings.file_size_limit=enabled.then_some(sp_export::FileSizeLimit{max_kib:2048,minimum_quality:50});}
            if let Some(limit)=&mut state.export_settings.file_size_limit{
                ui.horizontal(|ui|{ui.add(egui::DragValue::new(&mut limit.max_kib).range(1..=1_000_000).suffix(" KiB"));ui.add(egui::Slider::new(&mut limit.minimum_quality,1..=100).text(crate::i18n::text("Minimum quality")));});
                ui.label(crate::i18n::text("Keeps dimensions; reports an error if the quality floor cannot meet the target."));
            }
        });
        ui.horizontal_wrapped(|ui| {
            ui.label(crate::i18n::text("AI upscale"));
            if ui.selectable_label(state.export_settings.upscale_model.is_none(), crate::i18n::text("Off")).clicked() {
                state.export_settings.upscale_model = None;
            }
            for (scale, label) in [(2, "Faithful 2×"), (4, "Faithful 4×")] {
                let model = default_upscale_model(scale);
                let selected = model.as_ref().is_some_and(|model| state.export_settings.upscale_model.as_ref() == Some(model));
                if ui.add_enabled(model.is_some(), egui::Button::new(crate::i18n::text(label)).selected(selected))
                    .on_disabled_hover_text(crate::i18n::text("Install the official SwinIR checkpoints in Tools → AI runtime."))
                    .clicked() {
                    state.export_settings.upscale_model = model;
                    state.export_settings.size = SizeMode::Original;
                    state.export_preset_name.clear();
                }
            }
            if ui.button(crate::i18n::text("Custom model…")).clicked() { state.upscale_model_pick_requested = true; }
        });
        if let Some(model) = &state.export_settings.upscale_model {
            ui.label(egui::RichText::new(model.file_name().unwrap_or_default().to_string_lossy()).small());
            ui.label(egui::RichText::new(crate::i18n::text("AI processes the full-resolution crop, then applies the final output size and sharpening. Maximum AI intermediate is 150 MP.")).small());
            if let Some(dimensions) = export_enhanced_dims(state) {
                ui.label(format!("{}: {}", crate::i18n::text("AI resolution"), compact_dimensions(dimensions)));
                if megapixels(dimensions) > 150.0 {
                    ui.colored_label(egui::Color32::LIGHT_RED, crate::i18n::text("This crop exceeds the 150 MP AI limit. Crop more tightly or choose 2×."));
                }
            } else {
                ui.label(crate::i18n::text("Custom model scale is checked during export; final dimensions are not yet known."));
            }
        }

        let size_label = match state.export_settings.size {
            SizeMode::Original if state.export_settings.upscale_model.is_some() => "Full AI resolution",
            SizeMode::Original => "Original working size",
            SizeMode::LongEdgePx(_) => "Long edge",
            SizeMode::ShortEdgePx(_) => "Short edge",
            SizeMode::Percent(_) => "Percentage",
            SizeMode::Exact { .. } => "Exact pixels",
            SizeMode::Megapixels(_) => "Megapixels",
        };
        if let Some(base_dimensions) = export_enhanced_dims(state) {
            let choices = [
                ("Full", SizeMode::Original),
                ("½ resolution", SizeMode::Percent(50.0)),
                ("¼ resolution", SizeMode::Percent(25.0)),
                ("4K", SizeMode::LongEdgePx(3_840)),
                ("1080p", SizeMode::LongEdgePx(1_920)),
            ];
            ui.horizontal_wrapped(|ui| {
                for (label, mode) in choices {
                    let output = mode.output_dims(base_dimensions.0, base_dimensions.1);
                    let selected = state.export_settings.size == mode;
                    let button = egui::Button::new(format!("{label}\n{} × {}", output.0, output.1))
                        .min_size(egui::vec2(98.0, 44.0))
                        .selected(selected);
                    if ui.add(button).clicked() {
                        state.export_settings.size = mode;
                        state.export_preset_name.clear();
                    }
                }
            });
        }
        ui.add_space(theme::SPACE * 0.5);
        ui.horizontal(|ui| {
            ui.label(crate::i18n::text("Resize"));
            egui::ComboBox::from_id_salt("export-size")
                .width(210.0)
                .selected_text(size_label)
                .show_ui(ui, |ui| {
                    if ui.button(crate::i18n::text(if state.export_settings.upscale_model.is_some() { "Full AI resolution" } else { "Original working size" })).clicked() {
                        state.export_settings.size = SizeMode::Original;
                    }
                    if ui.button(crate::i18n::text("Long edge")).clicked() {
                        let edge = export_enhanced_dims(state)
                            .map_or(3_840, |(width, height)| width.max(height).min(6_000));
                        state.export_settings.size = SizeMode::LongEdgePx(edge);
                    }
                    if ui.button(crate::i18n::text("Short edge")).clicked() {
                        let edge = export_enhanced_dims(state)
                            .map_or(2_160, |(width, height)| width.min(height).min(4_000));
                        state.export_settings.size = SizeMode::ShortEdgePx(edge);
                    }
                    if ui.button(crate::i18n::text("Percentage")).clicked() {
                        state.export_settings.size = SizeMode::Percent(50.0);
                    }
                    if ui.button(crate::i18n::text("Exact pixels")).clicked() {
                        let (width, height) = export_enhanced_dims(state).unwrap_or((3_000, 2_000));
                        state.export_settings.size = SizeMode::Exact { width, height };
                    }
                    if ui.button(crate::i18n::text("Megapixels")).clicked() {
                        let target = export_enhanced_dims(state)
                            .map_or(12.0, |dimensions| megapixels(dimensions).min(24.0) as f32);
                        state.export_settings.size = SizeMode::Megapixels(target);
                    }
                });
        });
        match &mut state.export_settings.size {
            SizeMode::Original => {
                ui.label(
                    egui::RichText::new(crate::i18n::text(if state.export_settings.upscale_model.is_some() { "Keeps the full AI resolution of the current crop." } else { "Uses the current crop at its full working resolution." }))
                        .small()
                        .color(theme::TEXT_SECONDARY),
                );
            }
            SizeMode::LongEdgePx(value) => {
                ui.horizontal(|ui| {
                    ui.label(crate::i18n::text("Maximum long edge"));
                    ui.add(egui::DragValue::new(value).range(1..=100_000).suffix(" px"));
                });
            }
            SizeMode::ShortEdgePx(value) => {
                ui.horizontal(|ui| {
                    ui.label(crate::i18n::text("Minimum short edge"));
                    ui.add(egui::DragValue::new(value).range(1..=100_000).suffix(" px"));
                });
            }
            SizeMode::Percent(value) => {
                ui.horizontal(|ui| {
                    ui.label(crate::i18n::text("Scale"));
                    ui.add(
                        egui::DragValue::new(value)
                            .range(1.0..=100.0)
                            .speed(1.0)
                            .suffix("%"),
                    );
                });
            }
            SizeMode::Exact { width, height } => {
                ui.horizontal(|ui| {
                    ui.label(crate::i18n::text("Fit inside"));
                    ui.add(egui::DragValue::new(width).range(1..=100_000));
                    ui.label(crate::i18n::text("×"));
                    ui.add(egui::DragValue::new(height).range(1..=100_000));
                    ui.label(crate::i18n::text("px"));
                });
            }
            SizeMode::Megapixels(value) => {
                ui.horizontal(|ui| {
                    ui.label(crate::i18n::text("Target size"));
                    ui.add(egui::DragValue::new(value).range(0.1..=200.0).suffix(" MP"));
                });
            }
        }
        if let Some(base_dimensions) = export_enhanced_dims(state) {
            let output = state
                .export_settings
                .size
                .output_dims(base_dimensions.0, base_dimensions.1);
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(crate::i18n::text("Final output"))
                        .small()
                        .color(theme::TEXT_SECONDARY),
                );
                ui.label(
                    egui::RichText::new(compact_dimensions(output))
                        .strong()
                        .color(theme::ACCENT),
                );
                if output == base_dimensions && state.export_settings.upscale_model.is_none() {
                    ui.label(
                        egui::RichText::new(crate::i18n::text("No resizing"))
                            .small()
                            .color(theme::TEXT_FAINT),
                    );
                }
            });
        }
        ui.separator();
        export_section_heading(ui, "FINISHING");
        ui.horizontal(|ui| {
            ui.label(crate::i18n::text("Output sharpening"));
            egui::ComboBox::from_id_salt("export-sharpening")
                .selected_text(match state.export_settings.output_sharpening {
                    OutputSharpening::None => "None",
                    OutputSharpening::Low => "Low",
                    OutputSharpening::Standard => "Standard",
                    OutputSharpening::High => "High",
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut state.export_settings.output_sharpening,
                        OutputSharpening::None,
                        crate::i18n::text("None"),
                    );
                    ui.selectable_value(
                        &mut state.export_settings.output_sharpening,
                        OutputSharpening::Low,
                        crate::i18n::text("Low"),
                    );
                    ui.selectable_value(
                        &mut state.export_settings.output_sharpening,
                        OutputSharpening::Standard,
                        crate::i18n::text("Standard"),
                    );
                    ui.selectable_value(
                        &mut state.export_settings.output_sharpening,
                        OutputSharpening::High,
                        crate::i18n::text("High"),
                    );
                });
        });
        ui.horizontal(|ui| {
            let mut enabled = state.export_settings.border.is_some();
            if ui.checkbox(&mut enabled, crate::i18n::text("Border")).changed() {
                state.export_settings.border = enabled.then_some(sp_export::Border {
                    width_percent: 2.0,
                    rgb: [255, 255, 255],
                });
            }
            if let Some(border) = &mut state.export_settings.border {
                ui.add(
                    egui::DragValue::new(&mut border.width_percent)
                        .range(0.0..=25.0)
                        .suffix("%"),
                );
                let mut color =
                    egui::Color32::from_rgb(border.rgb[0], border.rgb[1], border.rgb[2]);
                if ui.color_edit_button_srgba(&mut color).changed() {
                    border.rgb = [color.r(), color.g(), color.b()];
                }
            }
        });
        ui.horizontal(|ui| {
            let mut enabled = state.export_settings.watermark.is_some();
            if ui.checkbox(&mut enabled, crate::i18n::text("PNG watermark")).changed() {
                if enabled {
                    state.export_watermark_requested = true;
                } else {
                    state.export_settings.watermark = None;
                }
            }
            if let Some(watermark) = &state.export_settings.watermark {
                ui.label(
                    egui::RichText::new(
                        watermark
                            .png
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy(),
                    )
                    .small()
                    .color(theme::TEXT_SECONDARY),
                );
                if ui.button(crate::i18n::text("Change…")).clicked() {
                    state.export_watermark_requested = true;
                }
            }
        });
        if let Some(watermark) = &mut state.export_settings.watermark {
            use sp_export::WatermarkPosition;
            ui.horizontal(|ui| {
                ui.label(crate::i18n::text("Watermark position"));
                egui::ComboBox::from_id_salt("watermark-position")
                    .selected_text(match watermark.position {
                        WatermarkPosition::TopLeft => "Top left",
                        WatermarkPosition::TopRight => "Top right",
                        WatermarkPosition::BottomLeft => "Bottom left",
                        WatermarkPosition::BottomRight => "Bottom right",
                        WatermarkPosition::Center => "Center",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut watermark.position,
                            WatermarkPosition::TopLeft,
                            crate::i18n::text("Top left"),
                        );
                        ui.selectable_value(
                            &mut watermark.position,
                            WatermarkPosition::TopRight,
                            crate::i18n::text("Top right"),
                        );
                        ui.selectable_value(
                            &mut watermark.position,
                            WatermarkPosition::BottomLeft,
                            crate::i18n::text("Bottom left"),
                        );
                        ui.selectable_value(
                            &mut watermark.position,
                            WatermarkPosition::BottomRight,
                            crate::i18n::text("Bottom right"),
                        );
                        ui.selectable_value(
                            &mut watermark.position,
                            WatermarkPosition::Center,
                            crate::i18n::text("Center"),
                        );
                    });
            });
            ui.horizontal(|ui| {
                ui.add(egui::Slider::new(&mut watermark.opacity, 0.0..=1.0).text(crate::i18n::text("Opacity")));
                ui.add(
                    egui::DragValue::new(&mut watermark.scale_percent)
                        .range(1.0..=100.0)
                        .suffix("%"),
                );
            });
        }
        ui.horizontal(|ui| {
            ui.label(crate::i18n::text("Name"));
            ui.text_edit_singleline(&mut state.export_settings.filename_pattern);
        });
        ui.horizontal(|ui| {
            ui.label(crate::i18n::text("Folder"));
            let folder = if state.export_settings.destination.as_os_str().is_empty() {
                "Same as original".to_owned()
            } else {
                state.export_settings.destination.display().to_string()
            };
            ui.label(
                egui::RichText::new(folder)
                    .small()
                    .color(theme::TEXT_SECONDARY),
            );
            if ui.button(crate::i18n::text("Choose…")).clicked() {
                state.export_destination_requested = true;
            }
        });
        ui.checkbox(
            &mut state.export_settings.copy_metadata,
            crate::i18n::text("Copy capture metadata"),
        );
        ui.checkbox(
            &mut state.export_settings.copy_descriptive_metadata,
            crate::i18n::text("Include caption, keywords and rights"),
        );
        ui.add_enabled_ui(state.export_settings.copy_metadata, |ui| {
            ui.checkbox(&mut state.export_settings.strip_gps, crate::i18n::text("Strip GPS"));
        });
        if matches!(state.export_settings.format, ExportFormat::Avif { .. })
            && state.edit.resolution.mode == sp_core::edit::ResolutionMode::Full
        {
            ui.colored_label(theme::WARNING, crate::i18n::text("Full-resolution AVIF encoding can be slow."));
        }
        let selected = state.selected_photos.len();
        if selected > 1 {
            ui.checkbox(
                &mut state.export_batch,
                crate::i18n::format("Export all {0} selected photos", &[selected.to_string()]),
            );
        } else {
            state.export_batch = false;
        }

        ui.collapsing(crate::i18n::text("Text watermark"), |ui| {
            let mut enabled=state.export_settings.text_watermark.is_some();
            if ui.checkbox(&mut enabled,crate::i18n::text("Add attribution text")).changed(){state.export_settings.text_watermark=enabled.then(sp_export::TextWatermark::default);}
            if let Some(mark)=&mut state.export_settings.text_watermark{
                ui.text_edit_singleline(&mut mark.text);
                ui.add(egui::Slider::new(&mut mark.size_percent,0.5..=25.0).text(crate::i18n::text("Size %")));
                ui.add(egui::Slider::new(&mut mark.opacity,0.0..=1.0).text(crate::i18n::text("Opacity")));
                ui.color_edit_button_srgb(&mut mark.rgb);
                egui::ComboBox::from_id_salt("text-watermark-position").selected_text(format!("{:?}",mark.position)).show_ui(ui,|ui|{
                    use sp_export::WatermarkPosition as P;
                    for (value,label) in [(P::TopLeft,"Top left"),(P::TopRight,"Top right"),(P::BottomLeft,"Bottom left"),(P::BottomRight,"Bottom right"),(P::Center,"Center")]{ui.selectable_value(&mut mark.position,value,label);}
                });
            }
        });
        ui.collapsing(crate::i18n::text("Multiple recipes and queue"), |ui| {
            ui.label(crate::i18n::text("Select saved recipes, or leave them clear to use the settings above."));
            use crate::state::VersionExportMode as Versions;
            egui::ComboBox::from_id_salt("enqueue-versions").selected_text(match state.export_version_mode {Versions::Current=>"Current edit (primary for other photos)",Versions::Primary=>"Primary edits only",Versions::All=>"All primary and named versions",Versions::Selected=>"Choose individual versions"}).show_ui(ui,|ui|{
                for (mode,label) in [(Versions::Current,"Current edit (primary for other photos)"),(Versions::Primary,"Primary edits only"),(Versions::All,"All primary and named versions"),(Versions::Selected,"Choose individual versions")] {ui.selectable_value(&mut state.export_version_mode,mode,label);}
            });
            if state.export_version_mode==Versions::Selected {
                let mut sources:Vec<_>=if state.export_batch {state.selected_photos.iter().cloned().collect()}else{state.open_image.iter().cloned().collect()};sources.sort();
                egui::ScrollArea::vertical().id_salt("delivery-version-choices").max_height(180.0).show(ui,|ui|{
                    for source in sources {
                        ui.label(source.file_name().unwrap_or_default().to_string_lossy());
                        let mut versions=vec![(String::new(),"Primary".to_owned())];
                        if state.open_image.as_ref()==Some(&source) {versions.extend(state.virtual_copies.iter().map(|id|(id.clone(),state.virtual_copy_names.get(id).cloned().unwrap_or_else(||id.clone()))));}
                        else if let Some(photo)=state.catalog_photos.iter().find(|photo|photo.path==source){versions.extend(photo.virtual_copies.iter().map(|version|(version.id.clone(),version.name.clone())));}
                        for (id,name) in versions {
                            let key=(source.clone(),id);let mut selected=state.export_version_choices.contains(&key);
                            if ui.checkbox(&mut selected,name).changed(){if selected{state.export_version_choices.insert(key);}else{state.export_version_choices.remove(&key);}}
                        }
                    }
                });
            }
            for preset in &state.export_presets {
                let mut selected=state.export_recipe_names.contains(&preset.name);
                if ui.checkbox(&mut selected,&preset.name).changed(){if selected{state.export_recipe_names.insert(preset.name.clone());}else{state.export_recipe_names.remove(&preset.name);}}
            }
            ui.horizontal(|ui|{
                if ui.add_enabled(!state.export_enqueue_busy,egui::Button::new(crate::i18n::text("Add to queue"))).clicked(){state.export_enqueue_requested=true;}
                if ui.button(crate::i18n::text("Show queue")).clicked(){state.export_queue_open=true;}
            });
        });
      });
        ui.separator();
        if state.export_busy {
            ui.add(egui::ProgressBar::new(state.export_progress).show_percentage());
        }
        ui.add_space(theme::SPACE);
        ui.horizontal(|ui| {
            if state.export_busy {
                if ui.button(crate::i18n::text("Cancel")).clicked() {
                    state.export_cancel_requested = true;
                }
            } else {
                if ui.button(crate::i18n::text("Close")).clicked() {
                    state.export_dialog_open = false;
                }
                if ui
                    .add(egui::Button::new(crate::i18n::text("Export")).fill(theme::ACCENT))
                    .clicked()
                {
                    state.export_start_requested = true;
                }
            }
        });
    });
}

/// Phase 3B calibration wizard. Native pickers and fitting are launched by
/// `ShopPhotoApp::logic`; this function only paints and mutates plain state.
pub fn reference_match_wizard(ctx: &egui::Context, state: &mut AppState) {
    if !state.reference_wizard_open {
        return;
    }
    let response = egui::Modal::new(egui::Id::new("reference-match-wizard")).show(ctx, |ui| {
        ui.set_width(560.0);
        ui.heading(crate::i18n::text("Create Reference Match profile"));
        ui.label(
            egui::RichText::new(
                crate::i18n::text("Calibrate ShopPhoto against 3–10 of your own neutral 16-bit TIFF renders (for example, exports from Phocus)."),
            )
            .color(theme::TEXT_SECONDARY),
        );
        ui.add_space(theme::SPACE);

        egui::Frame::new()
            .fill(theme::BG_RAISED)
            .stroke(egui::Stroke::new(1.0, theme::HAIRLINE))
            .corner_radius(theme::RADIUS_PANEL)
            .inner_margin(theme::SPACE)
            .show(ui, |ui| {
                ui.label(
                    egui::RichText::new(crate::i18n::text("REFERENCE TIFF CHECKLIST"))
                        .size(11.0)
                        .strong(),
                );
                for (setting, value) in [
                    ("File", "16-bit TIFF · full size · no output sharpening"),
                    ("Adjustments", "Exposure 0 · contrast 0 · curves linear"),
                    ("White balance", "As Shot · tint 0"),
                    ("Detail", "Sharpening 0 · noise reduction 0"),
                    (
                        "Crop / lens",
                        "Uncropped · geometry and lens corrections off",
                    ),
                    ("Color", "Embed the working ICC profile"),
                    ("Naming", "TIFF stem must match its RAW stem"),
                ] {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(crate::i18n::text("•")).color(theme::ACCENT));
                        ui.add_sized([92.0, 18.0], egui::Label::new(setting));
                        ui.label(egui::RichText::new(value).color(theme::TEXT_SECONDARY));
                    });
                }
            });
        ui.add_space(theme::SPACE);

        ui.horizontal(|ui| {
            if ui
                .add_enabled(!state.reference_fit_busy, egui::Button::new(crate::i18n::text("Choose RAWs…")))
                .clicked()
            {
                state.reference_pick_raws_requested = true;
            }
            let text = if state.reference_raws.is_empty() {
                "No RAWs selected".to_owned()
            } else {
                { let __l10n_args = (&(state.reference_raws.len()),); crate::i18n::format("{0} RAWs selected", &[__l10n_args.0.to_string()]) }
            };
            ui.label(egui::RichText::new(text).color(theme::TEXT_SECONDARY));
        });
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    !state.reference_fit_busy,
                    egui::Button::new(crate::i18n::text("Choose TIFF folder…")),
                )
                .clicked()
            {
                state.reference_pick_tiff_requested = true;
            }
            let text = state
                .reference_tiff_dir
                .as_ref()
                .and_then(|p| p.file_name())
                .map_or_else(|| "No folder selected".into(), |p| p.to_string_lossy());
            ui.label(egui::RichText::new(text).color(theme::TEXT_SECONDARY));
        });

        if state.reference_fit_busy {
            ui.add_space(theme::SPACE);
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(crate::i18n::text("Decoding, sampling, and fitting the 33³ profile…"));
            });
        }
        if let Some(report) = &state.reference_report {
            ui.add_space(theme::SPACE);
            let passed = report.mean_delta_e_00 < 2.0;
            egui::Frame::new()
                .fill(theme::BG_RAISED)
                .corner_radius(theme::RADIUS_CONTROL)
                .inner_margin(theme::SPACE)
                .show(ui, |ui| {
                    ui.label(
                        egui::RichText::new(crate::i18n::text(if passed {
                            "Calibration passed"
                        } else {
                            "Calibration saved with a quality warning"
                        }))
                        .strong()
                        .color(if passed {
                            theme::ACCENT
                        } else {
                            theme::WARNING
                        }),
                    );
                    ui.label({ let __l10n_args = (&(report.mean_delta_e_00), &(report.p95_delta_e_00), &(report.samples), &(report.paired_images),); crate::i18n::format("Mean ΔE₀₀ {0}  ·  p95 {1}  ·  {2} holdout samples  ·  {3} paired images", &[format!("{:.2}", __l10n_args.0), format!("{:.2}", __l10n_args.1), __l10n_args.2.to_string(), __l10n_args.3.to_string()]) });
                    ui.label({ let __l10n_args = (&(report.profile_name),); crate::i18n::format("Saved and selected: {0}", std::slice::from_ref(__l10n_args.0)) });
                    if !report.all_embedded_icc {
                        ui.label(
                            egui::RichText::new(
                                crate::i18n::text("One or more TIFFs had no embedded ICC; sRGB was assumed."),
                            )
                            .color(theme::WARNING),
                        );
                    }
                });
        }

        ui.add_space(theme::SPACE);
        ui.horizontal(|ui| {
            let ready = (3..=10).contains(&state.reference_raws.len())
                && state.reference_tiff_dir.is_some()
                && !state.reference_fit_busy;
            if ui
                .add_enabled(ready, egui::Button::new(crate::i18n::text("Fit profile")))
                .clicked()
            {
                state.reference_report = None;
                state.reference_fit_requested = true;
            }
            if ui
                .add_enabled(!state.reference_fit_busy, egui::Button::new(crate::i18n::text("Close")))
                .clicked()
            {
                state.reference_wizard_open = false;
            }
        });
    });
    if response.backdrop_response.clicked() && !state.reference_fit_busy {
        state.reference_wizard_open = false;
    }
}

/// Toasts, bottom-right above the filmstrip. Non-error toasts count down
/// (paused while the pointer is over the stack); errors stay until
/// dismissed and can be copied; some offer an Undo.
pub fn toasts_overlay(ctx: &egui::Context, state: &mut AppState) {
    state.toasts_paused = false;
    if state.toasts.is_empty() {
        return;
    }
    let hidden = state
        .toasts
        .len()
        .saturating_sub(crate::state::TOAST_VISIBLE);
    let mut dismiss = Vec::new();
    let mut action = None;
    let area = egui::Area::new(egui::Id::new("toasts"))
        .order(egui::Order::Foreground)
        .anchor(
            egui::Align2::RIGHT_BOTTOM,
            egui::vec2(
                -theme::SPACE * 2.0,
                -(theme::FILMSTRIP_H + theme::SPACE * 2.0),
            ),
        )
        .show(ctx, |ui| {
            // Left-aligned column: a right-aligned parent would flip every
            // row inside the toasts to right-to-left.
            ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                if hidden > 0 {
                    egui::Frame::new()
                        .fill(theme::BG_RAISED)
                        .stroke(egui::Stroke::new(1.0, theme::HAIRLINE))
                        .corner_radius(10.0)
                        .inner_margin(egui::Margin::symmetric(9, 2))
                        .show(ui, |ui| {
                            ui.label(
                                egui::RichText::new(crate::i18n::format(
                                    "+{0} more",
                                    &[hidden.to_string()],
                                ))
                                .size(11.0)
                                .color(theme::TEXT_SECONDARY),
                            );
                        });
                    ui.add_space(4.0);
                }
                for toast in state.toasts.iter().skip(hidden) {
                    let accent = if toast.is_error {
                        theme::ERROR
                    } else {
                        theme::SUCCESS
                    };
                    let frame = egui::Frame::new()
                        .fill(theme::BG_RAISED)
                        .stroke(egui::Stroke::new(
                            1.0,
                            if toast.is_error {
                                theme::ERROR.gamma_multiply(0.8)
                            } else {
                                theme::HAIRLINE
                            },
                        ))
                        .corner_radius(theme::RADIUS_PANEL)
                        .inner_margin(egui::Margin {
                            left: 10,
                            right: 6,
                            top: 8,
                            bottom: 9,
                        })
                        .shadow(egui::Shadow {
                            offset: [0, 6],
                            blur: 18,
                            spread: 0,
                            color: egui::Color32::from_black_alpha(110),
                        })
                        .show(ui, |ui| {
                            ui.set_max_width(360.0);
                            ui.horizontal(|ui| {
                                let (icon, _) = ui.allocate_exact_size(
                                    egui::vec2(18.0, 18.0),
                                    egui::Sense::hover(),
                                );
                                let c = icon.center();
                                ui.painter().circle_filled(
                                    c,
                                    8.5,
                                    accent.gamma_multiply(if toast.is_error { 1.0 } else { 0.25 }),
                                );
                                if toast.is_error {
                                    ui.painter().text(
                                        c,
                                        egui::Align2::CENTER_CENTER,
                                        "!",
                                        egui::FontId::proportional(12.0),
                                        theme::ON_ACCENT,
                                    );
                                } else {
                                    let stroke = egui::Stroke::new(1.6, accent);
                                    ui.painter().line_segment(
                                        [c + egui::vec2(-3.5, 0.0), c + egui::vec2(-1.0, 3.0)],
                                        stroke,
                                    );
                                    ui.painter().line_segment(
                                        [c + egui::vec2(-1.0, 3.0), c + egui::vec2(4.0, -3.0)],
                                        stroke,
                                    );
                                }
                                ui.vertical(|ui| {
                                    ui.set_max_width(270.0);
                                    ui.add(
                                        egui::Label::new(
                                            egui::RichText::new(crate::i18n::text(&toast.message))
                                                .size(12.0)
                                                .color(if toast.is_error {
                                                    egui::Color32::from_rgb(0xFF, 0xC2, 0xB8)
                                                } else {
                                                    theme::TEXT_PRIMARY
                                                }),
                                        )
                                        .wrap(),
                                    );
                                    if toast.is_error
                                        && ui
                                            .small_button(crate::i18n::text("Copy details"))
                                            .clicked()
                                    {
                                        ui.ctx().copy_text(toast.message.clone());
                                    }
                                });
                                if let Some(toast_action) = toast.action {
                                    if ui
                                        .add(
                                            egui::Button::new(
                                                egui::RichText::new(toast_action.label())
                                                    .strong()
                                                    .color(theme::ACCENT_HOVER),
                                            )
                                            .small(),
                                        )
                                        .clicked()
                                    {
                                        action = Some(toast_action);
                                        dismiss.push(toast.id);
                                    }
                                }
                                if ui
                                    .add(
                                        egui::Button::new(
                                            egui::RichText::new(crate::i18n::text("×"))
                                                .size(14.0)
                                                .color(theme::TEXT_FAINT),
                                        )
                                        .frame(false),
                                    )
                                    .on_hover_text(crate::i18n::text("Dismiss"))
                                    .clicked()
                                {
                                    dismiss.push(toast.id);
                                }
                            });
                        });
                    if !toast.is_error {
                        // Countdown bar along the bottom edge.
                        let rect = frame.response.rect;
                        let fraction =
                            (toast.remaining / crate::state::TOAST_TTL_SECS).clamp(0.0, 1.0);
                        let bar = egui::Rect::from_min_max(
                            egui::pos2(rect.left() + 6.0, rect.bottom() - 3.0),
                            egui::pos2(
                                rect.left() + 6.0 + (rect.width() - 12.0) * fraction,
                                rect.bottom() - 1.5,
                            ),
                        );
                        ui.painter().rect_filled(
                            bar,
                            1.0,
                            if state.toasts_paused {
                                theme::TEXT_FAINT
                            } else {
                                theme::ACCENT
                            },
                        );
                    }
                    ui.add_space(theme::SPACE * 0.5);
                }
            });
        });
    state.toasts_paused = area.response.contains_pointer();
    for id in dismiss {
        state.dismiss_toast(id);
    }
    if action.is_some() {
        state.toast_action_requested = action;
    }
}

/// Key names as the shortcut table writes them, for highlighting the row
/// of a key pressed while the sheet is open.
fn pressed_key_name(ctx: &egui::Context) -> Option<String> {
    ctx.input(|i| {
        i.events.iter().find_map(|event| match event {
            egui::Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } => {
                let name = match key {
                    egui::Key::ArrowLeft => "←".to_owned(),
                    egui::Key::ArrowRight => "→".to_owned(),
                    egui::Key::ArrowUp => "↑".to_owned(),
                    egui::Key::ArrowDown => "↓".to_owned(),
                    egui::Key::Enter => "Return".to_owned(),
                    egui::Key::Escape => return None,
                    egui::Key::Backslash => "\\".to_owned(),
                    egui::Key::Questionmark => "?".to_owned(),
                    other => other.name().to_owned(),
                };
                let mut prefix = String::new();
                if modifiers.shift && !matches!(key, egui::Key::Questionmark) {
                    prefix.push('⇧');
                }
                if modifiers.command {
                    prefix.push('⌘');
                }
                Some(format!("{prefix}{name}"))
            }
            _ => None,
        })
    })
}

fn key_matches(keys: &str, pressed: &str) -> bool {
    if keys.split_whitespace().any(|k| k == pressed) || keys == pressed {
        return true;
    }
    // Ranges such as "0–5".
    if let (Some((lo, hi)), Ok(n)) = (keys.split_once('–'), pressed.parse::<u8>()) {
        if let (Ok(lo), Ok(hi)) = (lo.parse::<u8>(), hi.parse::<u8>()) {
            return (lo..=hi).contains(&n);
        }
    }
    false
}

fn key_cap(ui: &mut egui::Ui, text: &str) {
    egui::Frame::new()
        .fill(theme::BG_RAISED)
        .stroke(egui::Stroke::new(1.0, theme::HAIRLINE))
        .corner_radius(4.0)
        .inner_margin(egui::Margin::symmetric(6, 1))
        .show(ui, |ui| {
            ui.label(
                egui::RichText::new(text)
                    .monospace()
                    .size(11.0)
                    .color(theme::TEXT_PRIMARY),
            );
        });
}

pub fn shortcuts_dialog(ctx: &egui::Context, state: &mut AppState) {
    if !state.shortcuts_open {
        return;
    }
    let search_id = egui::Id::new("shortcut-search");
    let highlight_id = egui::Id::new("shortcut-highlight");
    let mut query = ctx
        .data(|d| d.get_temp::<String>(search_id))
        .unwrap_or_default();
    let typing = ctx.memory(|m| {
        m.focused()
            .is_some_and(|id| id == egui::Id::new("shortcut-search-field"))
    });
    if !typing {
        if let Some(key) = pressed_key_name(ctx) {
            ctx.data_mut(|d| d.insert_temp(highlight_id, key));
        }
    }
    let highlight = ctx.data(|d| d.get_temp::<String>(highlight_id));
    let response = egui::Modal::new(egui::Id::new("shortcut-cheat-sheet")).show(ctx, |ui| {
        ui.set_width(600.0);
        ui.horizontal(|ui| {
            ui.heading(crate::i18n::text("Keyboard shortcuts"));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(crate::i18n::text("Close")).clicked() {
                    state.shortcuts_open = false;
                }
                ui.label(
                    egui::RichText::new(crate::i18n::text("press a key to find it"))
                        .size(11.0)
                        .color(theme::TEXT_FAINT),
                );
            });
        });
        ui.add(
            egui::TextEdit::singleline(&mut query)
                .id(egui::Id::new("shortcut-search-field"))
                .hint_text(crate::i18n::text("Search actions or keys…"))
                .desired_width(f32::INFINITY),
        );
        ui.add_space(4.0);
        let q = query.trim().to_lowercase();
        egui::ScrollArea::vertical()
            .max_height(440.0)
            .show(ui, |ui| {
                let mut last_group = "";
                let mut any = false;
                for (group, keys, action) in crate::app::SHORTCUTS {
                    let keys =
                        crate::app::customize::label_for(&state.customize.bindings, action, keys);
                    let text_match = q.is_empty()
                        || action.to_lowercase().contains(&q)
                        || keys.to_lowercase().contains(&q)
                        || group.to_lowercase().contains(&q);
                    if !text_match {
                        continue;
                    }
                    any = true;
                    if *group != last_group {
                        last_group = group;
                        ui.add_space(6.0);
                        ui.label(theme::section_text(group));
                    }
                    let hit = highlight
                        .as_deref()
                        .is_some_and(|pressed| key_matches(&keys, pressed));
                    let row = egui::Frame::new()
                        .fill(if hit {
                            theme::ACCENT.gamma_multiply(0.25)
                        } else {
                            egui::Color32::TRANSPARENT
                        })
                        .corner_radius(5.0)
                        .inner_margin(egui::Margin::symmetric(6, 2))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new(*action).size(12.5));
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        key_cap(ui, &keys);
                                    },
                                );
                            });
                        });
                    if hit {
                        row.response.scroll_to_me(None);
                    }
                }
                if !any {
                    ui.label(
                        egui::RichText::new(crate::i18n::text(
                            "No shortcut matches. Try “clip”, “label” or “undo”.",
                        ))
                        .color(theme::TEXT_FAINT),
                    );
                }
            });
    });
    ctx.data_mut(|d| d.insert_temp(search_id, query));
    if response.should_close() {
        state.shortcuts_open = false;
    }
    if !state.shortcuts_open {
        ctx.data_mut(|d| {
            d.remove::<String>(search_id);
            d.remove::<String>(highlight_id);
        });
    }
}

pub fn debug_overlay(ctx: &egui::Context, state: &AppState) {
    if !state.debug_overlay {
        return;
    }
    egui::Area::new(egui::Id::new("performance-overlay"))
        .anchor(
            egui::Align2::LEFT_TOP,
            egui::vec2(12.0, theme::TOP_BAR_H + 12.0),
        )
        .interactable(false)
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(egui::Color32::from_black_alpha(210))
                .stroke(egui::Stroke::new(1.0, theme::HAIRLINE))
                .corner_radius(theme::RADIUS_CONTROL)
                .inner_margin(theme::SPACE)
                .show(ui, |ui| {
                    ui.monospace(format!("frame {:>6.2} ms", state.frame_ms));
                    ui.monospace(format!("textures {:>5.0} MB", state.gpu_texture_mb));
                    ui.monospace(format!("renders {:>8}", state.render_count));
                    ui.monospace(format!("photos  {:>8}", state.catalog_photos.len()));
                    ui.monospace(format!("undo    {:>8}", state.undo_stack.len()));
                    if let Some(stall) = &state.last_stall {
                        ui.monospace(format!("stall   {stall}"));
                    }
                });
        });
}

pub fn welcome_dialog(ctx: &egui::Context, state: &mut AppState) {
    if !state.welcome_open && state.update_available.is_none() {
        return;
    }
    egui::Modal::new(egui::Id::new("welcome-dialog")).show(ctx, |ui| {
        ui.set_width(440.0);
        if let Some((version, url, notes)) = &state.update_available {
            ui.heading(crate::i18n::format(
                "ShopPhoto {0} is available",
                std::slice::from_ref(version),
            ));
            ui.label(notes);
            ui.hyperlink_to(crate::i18n::text("Open download page"), url);
            if ui.button(crate::i18n::text("Later")).clicked() {
                state.update_available = None;
            }
            return;
        }
        ui.heading(crate::i18n::text("Welcome to ShopPhoto"));
        ui.label(
            egui::RichText::new(crate::i18n::text(
                "A non-destructive photo editor for camera RAWs and rendered photographs.",
            ))
            .color(theme::TEXT_SECONDARY),
        );
        ui.add_space(theme::SPACE);
        for (title, detail) in [
            ("1  Add a folder", "Build a fast, sidecar-backed library."),
            (
                "2  Open a photo",
                "Develop camera RAWs, JPEG, PNG, TIFF, WebP and JPEG XL photographs.",
            ),
            ("3  Export", "Choose resolution, color space, SDR or HDR."),
        ] {
            ui.label(egui::RichText::new(crate::i18n::text(title)).strong());
            ui.label(egui::RichText::new(crate::i18n::text(detail)).color(theme::TEXT_SECONDARY));
        }
        ui.add_space(theme::SPACE);
        ui.horizontal(|ui| {
            if ui.button(crate::i18n::text("Add photo folder…")).clicked() {
                state.library_add_folder_requested = true;
                state.mode = Mode::Library;
                state.welcome_open = false;
                state.first_edit_tip = true;
            }
            if ui
                .button(crate::i18n::text("Start with a single photo"))
                .clicked()
            {
                state.open_dialog_requested = true;
                state.welcome_open = false;
                state.first_edit_tip = true;
            }
        });
        if !state.welcome_open {
            if let Some(path) = dirs::config_dir().map(|dir| dir.join("shopphoto")) {
                let _ = std::fs::create_dir_all(&path);
                let _ = std::fs::write(path.join("first-run-complete"), b"1");
            }
        }
    });
}

#[cfg(test)]
mod upscale_size_tests {
    use super::*;

    #[test]
    fn unknown_custom_checkpoint_does_not_promise_an_output_size() {
        assert_eq!(
            upscale_model_scale(std::path::Path::new("custom-x4-model.pth")),
            None
        );
        assert_eq!(
            upscale_model_scale(std::path::Path::new("SwinIR-M-x2.pth")),
            Some(2)
        );
        assert_eq!(
            upscale_model_scale(std::path::Path::new("SwinIR-M-x4.pth")),
            Some(4)
        );
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("manifest.json"), r#"{"scale":4}"#).unwrap();
        assert_eq!(upscale_model_scale(directory.path()), Some(4));
        std::fs::write(directory.path().join("manifest.json"), r#"{"scale":8}"#).unwrap();
        assert_eq!(upscale_model_scale(directory.path()), None);
    }

    #[test]
    fn final_size_uses_cropped_native_scale_then_requested_dimensions() {
        let mut state = AppState {
            active_size: Some((1000, 800)),
            ..Default::default()
        };
        state.edit.geometry.crop = [0.2, 0.25, 0.8, 0.75];
        state.edit.geometry.quarter_turns = 1;
        state.export_settings.upscale_model = Some("SwinIR-M-x4.pth".into());
        assert_eq!(export_geometry_dims(&state), Some((400, 600)));
        assert_eq!(export_enhanced_dims(&state), Some((1600, 2400)));
        state.export_settings.size = sp_export::SizeMode::LongEdgePx(1800);
        let (width, height) = export_enhanced_dims(&state).unwrap();
        assert_eq!(
            state.export_settings.size.output_dims(width, height),
            (1200, 1800)
        );
        // Working-resolution reduction occurs before crop and SR.
        state.edit.resolution.mode = sp_core::edit::ResolutionMode::Super2x2;
        assert_eq!(export_enhanced_dims(&state), Some((800, 1200)));
    }
}
