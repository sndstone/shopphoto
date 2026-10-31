//! Selective, portable develop + delivery recipes.
use super::*;
use sp_export::workflow::PhotoWorkflow;

#[derive(Debug)]
pub(crate) struct WorkflowUi {
    pub open: bool,
    name: String,
    groups: sp_core::preset::EditGroups,
    include_develop: bool,
    save: bool,
    load: bool,
    busy: bool,
}
impl Default for WorkflowUi {
    fn default() -> Self {
        Self {
            open: false,
            name: "Photo workflow".into(),
            groups: Default::default(),
            include_develop: true,
            save: false,
            load: false,
            busy: false,
        }
    }
}
pub(super) enum WorkflowResult {
    Saved,
    Loaded(Box<PhotoWorkflow>),
    Cancelled,
}
impl ShopPhotoApp {
    pub(super) fn poll_workflow(&mut self, ctx: &egui::Context) {
        if let Some(result) = self.workflow_rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.workflow_rx = None;
            self.state.workflow.busy = false;
            match result {
                Ok(WorkflowResult::Saved) => self.state.push_toast("Photo workflow saved", false),
                Ok(WorkflowResult::Loaded(workflow)) => {
                    if self.state.open_image.is_some() {
                        let mut edit = self.state.edit.clone();
                        match workflow.apply(&mut edit) {
                            Ok(()) => {
                                self.state.apply_edit_step_labeled(
                                    edit,
                                    Some({
                                        let __l10n_args = (&(workflow.name),);
                                        crate::i18n::format(
                                            "Workflow: {0}",
                                            std::slice::from_ref(__l10n_args.0),
                                        )
                                    }),
                                );
                                self.state.export_settings = workflow.export;
                                self.state.workflow.name = workflow.name;
                                self.state.push_toast(
                                    "Workflow applied; delivery settings are ready in Export",
                                    false,
                                );
                            }
                            Err(error) => self.state.push_toast(error, true),
                        }
                    } else {
                        self.state
                            .push_toast("Open a photo before applying a workflow", true);
                    }
                }
                Ok(WorkflowResult::Cancelled) => {}
                Err(error) => self.state.push_toast(
                    crate::i18n::format("Workflow: {0}", std::slice::from_ref(&error)),
                    true,
                ),
            }
        }
        if self.state.workflow.busy {
            return;
        }
        let save = std::mem::take(&mut self.state.workflow.save);
        let load = std::mem::take(&mut self.state.workflow.load);
        if !save && !load {
            return;
        }
        let workflow = PhotoWorkflow {
            version: 1,
            name: self.state.workflow.name.trim().to_owned(),
            develop: self.state.workflow.include_develop.then(|| {
                sp_core::preset::UserPreset::from_edit(
                    self.state.workflow.name.clone(),
                    self.state.workflow.groups,
                    &self.state.edit,
                )
            }),
            export: self.state.export_settings.clone(),
        };
        let context = ctx.clone();
        let (tx, rx) = crossbeam_channel::bounded(1);
        std::thread::spawn(move || {
            let result = (|| -> Result<WorkflowResult, String> {
                let picker = rfd::FileDialog::new()
                    .add_filter(crate::i18n::text("Photo workflow"), &["spworkflow"]);
                if save {
                    let Some(path) = picker.set_file_name("photo.spworkflow").save_file() else {
                        return Ok(WorkflowResult::Cancelled);
                    };
                    workflow.save(&path)?;
                    Ok(WorkflowResult::Saved)
                } else {
                    let Some(path) = picker.pick_file() else {
                        return Ok(WorkflowResult::Cancelled);
                    };
                    PhotoWorkflow::load(&path).map(|w| WorkflowResult::Loaded(Box::new(w)))
                }
            })();
            let _ = tx.send(result);
            context.request_repaint();
        });
        self.workflow_rx = Some(rx);
        self.state.workflow.busy = true;
    }
}
pub(super) fn window(ctx: &egui::Context, state: &mut AppState) {
    if !state.workflow.open {
        return;
    }
    let mut open = true;
    egui::Window::new(crate::i18n::text("Photo workflows")).open(&mut open).resizable(false).show(ctx,|ui|{
        ui.label(crate::i18n::text("Save selected adjustments together with the current export settings."));
        ui.horizontal(|ui|{ui.label(crate::i18n::text("Name"));ui.text_edit_singleline(&mut state.workflow.name);});
        ui.checkbox(&mut state.workflow.include_develop,crate::i18n::text("Include develop adjustments"));
        ui.add_enabled_ui(state.workflow.include_develop,|ui|{
            let g=&mut state.workflow.groups;
            egui::Grid::new("workflow-groups").num_columns(2).show(ui,|ui|{
                ui.checkbox(&mut g.tone,crate::i18n::text("Tone and curves"));ui.checkbox(&mut g.color,crate::i18n::text("Color"));ui.end_row();
                ui.checkbox(&mut g.detail,crate::i18n::text("Detail and denoise"));ui.checkbox(&mut g.effects,crate::i18n::text("Effects"));ui.end_row();
                ui.checkbox(&mut g.lens,crate::i18n::text("Lens correction"));ui.checkbox(&mut g.white_balance,crate::i18n::text("White balance / negative"));ui.end_row();
                ui.checkbox(&mut g.profile,crate::i18n::text("Camera profile / look"));ui.checkbox(&mut g.geometry,crate::i18n::text("Crop and geometry"));ui.end_row();
                ui.checkbox(&mut g.spots,crate::i18n::text("Local masks and retouch"));ui.checkbox(&mut g.output,crate::i18n::text("Resolution and HDR"));ui.end_row();
            });
        });
        ui.label(crate::i18n::text("Photo ratings, captions and copyright stay with each photo. Applying a workflow creates one develop undo step."));
        ui.add_enabled_ui(!state.workflow.busy&&state.open_image.is_some(),|ui|{ui.horizontal(|ui|{
            if ui.add_enabled(!state.workflow.name.trim().is_empty(),egui::Button::new(crate::i18n::text("Save workflow…"))).clicked(){state.workflow.save=true;}
            if ui.button(crate::i18n::text("Load and apply…")).clicked(){state.workflow.load=true;}
        });});
        if state.workflow.busy{ui.spinner();}
        ui.separator();ui.label(crate::i18n::text("Automation"));
        ui.code("shopphoto render input.jpg --output result.jpg --workflow photo.spworkflow");
        ui.small(crate::i18n::text("Uses saved photo edits and reports JSON results. Run shopphoto --help for all options."));
    });
    state.workflow.open = open;
}
