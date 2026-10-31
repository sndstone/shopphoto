//! Physical photographic page layouts, rendered through the normal edit pipeline.
use super::*;
pub(super) struct PrintRequest {
    pub layout: sp_export::print::PrintLayout,
    pub destination: PathBuf,
}
impl ShopPhotoApp {
    pub(super) fn poll_printer(&mut self, ctx: &egui::Context) {
        if let Some(picked) = self.print_picker.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.print_picker = None;
            if let Some(destination) = picked {
                if self.state.export_busy
                    || self.state.enhancement_busy
                    || self.state.ai_denoise_busy
                {
                    self.state
                        .push_toast("Wait for the current image task before printing", false);
                } else {
                    self.print_request = Some(PrintRequest {
                        layout: self.state.print_layout.clone(),
                        destination,
                    });
                    self.print_open_pending = self.state.print_open_viewer;
                    let batch = self.state.export_batch;
                    self.state.export_batch = self.state.selected_photos.len() > 1;
                    self.start_export(ctx);
                    self.state.export_batch = batch;
                    self.print_request = None;
                    if self.export_job.is_some() {
                        self.state.print_dialog_open = false;
                    } else {
                        self.print_open_pending = false;
                    }
                }
            }
        }
        if std::mem::take(&mut self.state.print_choose_requested) && self.print_picker.is_none() {
            if let Err(error) = self.state.print_layout.validate() {
                self.state.push_toast(error, true);
                return;
            }
            let (tx, rx) = crossbeam_channel::bounded(1);
            let context = ctx.clone();
            std::thread::spawn(move || {
                let _ = tx.send(
                    rfd::FileDialog::new()
                        .set_title(crate::i18n::text("Save photographic print PDF"))
                        .set_file_name("photos.pdf")
                        .add_filter(crate::i18n::text("PDF document"), &["pdf"])
                        .save_file(),
                );
                context.request_repaint();
            });
            self.print_picker = Some(rx);
        }
    }
}
pub(super) fn print_window(ctx: &egui::Context, state: &mut AppState) {
    if !state.print_dialog_open {
        return;
    }
    let mut open = true;
    egui::Window::new(crate::i18n::text("Print / contact sheet")).open(&mut open).default_width(460.0).show(ctx,|ui|{
        let layout=&mut state.print_layout;
        ui.horizontal(|ui|{
            for (label,w,h) in [("A4",210.0,297.0),("A3",297.0,420.0),("Letter",215.9,279.4),("10 × 15 cm",100.0,150.0)]{if ui.button(label).clicked(){layout.width_mm=w;layout.height_mm=h;}}
            if ui.button(crate::i18n::text("Rotate paper")).clicked(){std::mem::swap(&mut layout.width_mm,&mut layout.height_mm);}
        });
        ui.horizontal(|ui|{ui.label(crate::i18n::text("Paper"));ui.add(egui::DragValue::new(&mut layout.width_mm).range(25.0..=2000.0).suffix(" mm wide"));ui.add(egui::DragValue::new(&mut layout.height_mm).range(25.0..=2000.0).suffix(" mm high"));});
        ui.horizontal(|ui|{ui.add(egui::DragValue::new(&mut layout.margin_mm).range(0.0..=200.0).suffix(" mm margin"));ui.add(egui::DragValue::new(&mut layout.gap_mm).range(0.0..=50.0).suffix(" mm gap"));});
        ui.horizontal(|ui|{ui.add(egui::DragValue::new(&mut layout.columns).range(1..=10).suffix(" columns"));ui.add(egui::DragValue::new(&mut layout.rows).range(1..=10).suffix(" rows"));});
        ui.horizontal(|ui|{ui.add(egui::DragValue::new(&mut layout.ppi).range(72..=1200).suffix(" pixels/inch"));ui.checkbox(&mut layout.fill,crate::i18n::text("Fill and crop cells"));});
        ui.checkbox(&mut layout.captions,crate::i18n::text("Print filenames below photos"));
        let(w,h)=layout.cell_mm();ui.label({ let __l10n_args = (&(state.selected_photos.len().max(1)),); crate::i18n::format("Photo area: {0} × {1} mm. {2} photo(s).", &[format!("{:.1}", w), format!("{:.1}", h), __l10n_args.0.to_string()]) });
        ui.label(crate::i18n::text("Creates a color-tagged PDF with the current edits. Print it at actual size in your PDF viewer."));
        ui.checkbox(&mut state.print_open_viewer,crate::i18n::text("Open PDF in system viewer for printing"));
        let valid=layout.validate();if let Err(error)=&valid{ui.colored_label(theme::ERROR,error);}
        if ui.add_enabled(valid.is_ok()&&!state.export_busy&&state.open_image.is_some(),egui::Button::new(crate::i18n::text("Create print PDF…"))).clicked(){state.print_choose_requested=true;}
    });
    state.print_dialog_open = open;
}
pub(super) fn open_pdf(path: &std::path::Path) {
    #[cfg(target_os = "macos")]
    let result = std::process::Command::new("open").arg(path).spawn();
    #[cfg(target_os = "windows")]
    let result = std::process::Command::new("explorer").arg(path).spawn();
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let result = std::process::Command::new("xdg-open").arg(path).spawn();
    if let Err(error) = result {
        tracing::warn!(%error,"Could not open PDF viewer");
    }
}
