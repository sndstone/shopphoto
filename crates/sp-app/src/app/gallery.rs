//! Private proof receipts connect returned client choices to catalog photos.
use super::*;
#[derive(Debug)]
pub(crate) struct State {
    pub open: bool,
    title: String,
    caption: bool,
    create: bool,
    import: bool,
    busy: bool,
    review: Option<(sp_export::gallery::Picks, sp_export::gallery::Receipt)>,
    apply: bool,
}
impl Default for State {
    fn default() -> Self {
        Self {
            open: false,
            title: "Photo proofs".into(),
            caption: true,
            create: false,
            import: false,
            busy: false,
            review: None,
            apply: false,
        }
    }
}
pub(super) struct Request {
    pub destination: PathBuf,
    pub title: String,
    pub caption: bool,
    pub receipts: PathBuf,
}
pub(super) enum Result {
    Create(PathBuf),
    Picks(sp_export::gallery::Picks, sp_export::gallery::Receipt),
    Cancelled,
}
fn receipts_dir() -> Option<PathBuf> {
    dirs::data_local_dir().map(|p| p.join("shopphoto/proof-receipts"))
}
impl ShopPhotoApp {
    pub(super) fn poll_gallery(&mut self, ctx: &egui::Context) {
        if let Some(result) = self.gallery_rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.gallery_rx = None;
            self.state.gallery.busy = false;
            match result {
                Ok(Result::Create(destination)) => {
                    if let Some(receipts) = receipts_dir() {
                        self.gallery_request = Some(Request {
                            destination,
                            title: self.state.gallery.title.clone(),
                            caption: self.state.gallery.caption,
                            receipts,
                        });
                        let batch = self.state.export_batch;
                        self.state.export_batch = self.state.selected_photos.len() > 1;
                        self.start_export(ctx);
                        self.state.export_batch = batch;
                        self.gallery_request = None;
                        if self.export_job.is_some() {
                            self.state.gallery.open = false;
                        }
                    } else {
                        self.state
                            .push_toast("Private gallery storage is unavailable", true);
                    }
                }
                Ok(Result::Picks(picks, receipt)) => {
                    self.state.gallery.review = Some((picks, receipt));
                    self.state.gallery.open = true;
                }
                Ok(Result::Cancelled) => {}
                Err(error) => self.state.push_toast(
                    crate::i18n::format("Proof gallery: {0}", std::slice::from_ref(&error)),
                    true,
                ),
            }
        }
        if std::mem::take(&mut self.state.gallery.apply) {
            if let Some((picks, receipt)) = self.state.gallery.review.take() {
                let mut count = 0;
                for choice in picks.choices {
                    // Unselected/unrated choices do not erase the photographer's work.
                    let path = &receipt.sources[choice.id];
                    if !self.state.catalog_photos.iter().any(|p| &p.path == path) {
                        continue;
                    }
                    if choice.picked || choice.rating > 0 {
                        self.annotate_choices(
                            path,
                            (choice.rating > 0).then_some(choice.rating),
                            choice.picked,
                        );
                        count += usize::from(choice.picked);
                    }
                }
                self.state.push_toast(
                    crate::i18n::format(
                        "Applied returned choices; {0} photos picked",
                        &[count.to_string()],
                    ),
                    false,
                );
            }
        }
        if self.state.gallery.busy {
            return;
        }
        let create = std::mem::take(&mut self.state.gallery.create);
        let import = std::mem::take(&mut self.state.gallery.import);
        if !create && !import {
            return;
        }
        let context = ctx.clone();
        let (tx, rx) = crossbeam_channel::bounded(1);
        std::thread::spawn(move || {
            let result = (|| -> std::result::Result<Result, String> {
                if create {
                    return Ok(rfd::FileDialog::new()
                        .set_title(crate::i18n::text("Choose a new proof gallery folder name"))
                        .set_file_name("Photo proofs")
                        .save_file()
                        .map_or(Result::Cancelled, Result::Create));
                }
                let Some(path) = rfd::FileDialog::new()
                    .add_filter(
                        crate::i18n::text("Returned proof picks"),
                        &["sppicks", "json"],
                    )
                    .pick_file()
                else {
                    return Ok(Result::Cancelled);
                };
                let picks = sp_export::gallery::Picks::load(&path)?;
                let receipt = sp_export::gallery::Receipt::load(
                    &receipts_dir()
                        .ok_or("Private gallery storage is unavailable")?
                        .join(picks.receipt_filename()?),
                )?;
                picks.validate(&receipt)?;
                Ok(Result::Picks(picks, receipt))
            })();
            let _ = tx.send(result);
            context.request_repaint();
        });
        self.gallery_rx = Some(rx);
        self.state.gallery.busy = true;
    }
}
pub(super) fn window(ctx: &egui::Context, state: &mut AppState) {
    if !state.gallery.open {
        return;
    }
    let mut open = true;
    egui::Window::new(crate::i18n::text("Proof gallery / slideshow")).open(&mut open).default_width(500.0).show(ctx,|ui|{
        ui.horizontal(|ui|{ui.label(crate::i18n::text("Title"));ui.text_edit_singleline(&mut state.gallery.title);});
        ui.checkbox(&mut state.gallery.caption,crate::i18n::text("Show photo captions"));
        ui.label(crate::i18n::text("Creates a portable folder of edited 2048 px JPEGs with a browser gallery, slideshow, star ratings and client picks. Camera/GPS metadata is removed. Current watermarks are included."));
        ui.label(crate::i18n::text("Share the folder yourself. Keep the private gallery receipts in ShopPhoto to import returned choices."));
        if ui.add_enabled(!state.gallery.busy&&!state.export_busy&&state.open_image.is_some(),egui::Button::new({ let __l10n_args = (&(state.selected_photos.len().max(1)),); crate::i18n::format("Create gallery from {0} photo(s)…", &[__l10n_args.0.to_string()]) })).clicked(){state.gallery.create=true;}
        if ui.add_enabled(!state.gallery.busy,egui::Button::new(crate::i18n::text("Import returned picks…"))).clicked(){state.gallery.import=true;}
        if state.gallery.busy{ui.spinner();}
        if let Some((picks,receipt))=&state.gallery.review{
            ui.separator();ui.strong({ let __l10n_args = (&(receipt.title),); crate::i18n::format("Review choices for {0}", std::slice::from_ref(__l10n_args.0)) });
            let mut missing=0;
            egui::ScrollArea::vertical().max_height(260.0).show(ui,|ui|{for choice in &picks.choices{
                if !choice.picked&&choice.rating==0{continue;}
                let source=&receipt.sources[choice.id];let present=state.catalog_photos.iter().any(|p|&p.path==source);if !present{missing+=1;}
                ui.label({ let __l10n_args = (&(source.file_name().unwrap_or_default().to_string_lossy()), &(if choice.picked{"Pick"}else{"Unselected"}), &(choice.rating), &(if present{""}else{" · not in this catalog"}),); crate::i18n::format("{0} · {1} · {2} stars{3}", &[__l10n_args.0.to_string(), __l10n_args.1.to_string(), __l10n_args.2.to_string(), __l10n_args.3.to_string()]) });
            }});
            if missing>0{ui.label(crate::i18n::format("{0} photos cannot be matched in this catalog and will be skipped.", &[missing.to_string()]));}
            ui.small(crate::i18n::text("Applies picks and nonzero ratings. Unselected/unrated choices retain existing values."));
            if ui.button(crate::i18n::text("Apply reviewed choices")).clicked(){state.gallery.apply=true;}
        }
    });
    state.gallery.open = open;
}
