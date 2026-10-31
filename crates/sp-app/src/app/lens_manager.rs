//! Local optical calibration management; file dialogs and decoding run off UI.
use super::*;

#[derive(Debug, Default)]
pub(crate) struct State {
    pub open: bool,
    name: String,
    selected: usize,
    action: Option<Action>,
    job: Option<Receiver<Result<Option<String>, String>>>,
    status: String,
}
#[derive(Debug)]
enum Action {
    Import,
    Export(String),
    Disable(String),
    Calibrate(String),
}

fn available_path(directory: &std::path::Path, name: &str) -> PathBuf {
    let name: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .take(100)
        .collect();
    let name = if name.is_empty() { "lens" } else { &name };
    for index in 1u64.. {
        let candidate = directory.join(format!("{name}-{index}.splens"));
        if !candidate.exists() && !candidate.with_extension("splensdata").exists() {
            return candidate;
        }
    }
    unreachable!()
}

/// Publish a validated pair only to fresh names; refuse every existing file.
fn publish(
    directory: &std::path::Path,
    name: &str,
    profile: &sp_core::lens::LensProfile,
) -> Result<PathBuf, String> {
    std::fs::create_dir_all(directory).map_err(|e| e.to_string())?;
    let stage = tempfile::tempdir_in(directory).map_err(|e| e.to_string())?;
    let destination = available_path(directory, name);
    let staged = stage
        .path()
        .join(destination.file_name().unwrap_or_default());
    sp_raw::lens_store::save_lens(&staged, profile).map_err(|e| e.to_string())?;
    sp_raw::lens_store::load_lens(&staged).map_err(|e| e.to_string())?;
    sp_catalog::workflow::copy_verified(
        &staged.with_extension("splensdata"),
        &destination.with_extension("splensdata"),
    )
    .map_err(|e| e.to_string())?;
    if let Err(error) = sp_catalog::workflow::copy_verified(&staged, &destination) {
        let _ = std::fs::remove_file(destination.with_extension("splensdata"));
        return Err(error.to_string());
    }
    Ok(destination)
}

fn find_profile(
    directory: &std::path::Path,
    name: &str,
) -> Result<(PathBuf, sp_core::lens::LensProfile), String> {
    for entry in std::fs::read_dir(directory).map_err(|e| e.to_string())? {
        let path = entry.map_err(|e| e.to_string())?.path();
        if path.extension().is_some_and(|ext| ext == "splens") {
            if let Ok(profile) = sp_raw::lens_store::load_lens(&path) {
                if profile.name == name {
                    return Ok((path, profile));
                }
            }
        }
    }
    Err("The selected lens profile is no longer installed".into())
}

fn run(action: Action, directory: PathBuf) -> Result<Option<String>, String> {
    match action {
        Action::Import => {
            let Some(path) = rfd::FileDialog::new()
                .add_filter(crate::i18n::text("ShopPhoto lens profile"), &["splens"])
                .pick_file()
            else {
                return Ok(None);
            };
            let mut profile = sp_raw::lens_store::load_lens(&path).map_err(|e| e.to_string())?;
            let (installed, _) = sp_raw::lens_store::load_lens_dir(&directory);
            let base = profile.name.clone();
            let mut index = 2;
            while installed.iter().any(|p| p.name == profile.name) {
                profile.name = format!("{base} ({index})");
                index += 1;
            }
            let path = publish(&directory, &profile.key, &profile)?;
            Ok(Some({
                let __l10n_args = (&(path.display()),);
                crate::i18n::format("Imported lens profile: {0}", &[__l10n_args.0.to_string()])
            }))
        }
        Action::Export(name) => {
            let (_, profile) = find_profile(&directory, &name)?;
            let Some(folder) = rfd::FileDialog::new()
                .set_title(crate::i18n::text("Export lens profile and grid to folder"))
                .pick_folder()
            else {
                return Ok(None);
            };
            let path = publish(&folder, &profile.key, &profile)?;
            Ok(Some({
                let __l10n_args = (&(path.display()),);
                crate::i18n::format(
                    "Exported lens profile and grid: {0}",
                    &[__l10n_args.0.to_string()],
                )
            }))
        }
        Action::Disable(name) => {
            let (path, profile) = find_profile(&directory, &name)?;
            let disabled = directory.join("disabled");
            // Retain a complete, verified pair that can be imported again.
            let saved = publish(&disabled, &profile.key, &profile)?;
            std::fs::remove_file(&path).map_err(|e| e.to_string())?;
            let _ = std::fs::remove_file(path.with_extension("splensdata"));
            Ok(Some({
                let __l10n_args = (&(saved.display()),);
                crate::i18n::format(
                    "Disabled profile; recover it by importing {0}",
                    &[__l10n_args.0.to_string()],
                )
            }))
        }
        Action::Calibrate(name) => {
            if sp_raw::lens_store::load_lens_dir(&directory)
                .0
                .iter()
                .any(|p| p.name == name.trim())
            {
                return Err("A profile with this name already exists; choose another name".into());
            }
            let Some(paths) = rfd::FileDialog::new()
                .set_title(crate::i18n::text(
                    "Select evenly illuminated RAW flat fields",
                ))
                .add_filter(
                    crate::i18n::text("Camera RAW"),
                    sp_core::raw::SUPPORTED_RAW_EXTENSIONS,
                )
                .pick_files()
            else {
                return Ok(None);
            };
            let profile = sp_raw::lens_calibration::calibrate(&paths, &name)?;
            let path = publish(&directory, &profile.key, &profile)?;
            Ok(Some({
                let __l10n_args = (&(profile.apertures.len()), &(path.display()));
                crate::i18n::format(
                    "Calibrated {0} aperture grids: {1}",
                    &[__l10n_args.0.to_string(), __l10n_args.1.to_string()],
                )
            }))
        }
    }
}

impl ShopPhotoApp {
    pub(super) fn lens_manager_upkeep(&mut self, ctx: &egui::Context) {
        if let Some(result) = self
            .state
            .lens_manager
            .job
            .as_ref()
            .and_then(|rx| rx.try_recv().ok())
        {
            self.state.lens_manager.job = None;
            match result {
                Ok(Some(message)) => {
                    self.lenses = load_lens_profiles();
                    self.refresh_profile_names();
                    self.state.lens_manager.status = message;
                    if let Some(gpu) = &mut self.gpu {
                        gpu.last_lens = None;
                    }
                }
                Ok(None) => {}
                Err(error) => self.state.lens_manager.status = error,
            }
        }
        if self.state.lens_manager.job.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
            return;
        }
        let Some(action) = self.state.lens_manager.action.take() else {
            return;
        };
        let Some(directory) = lenses_dir() else {
            self.state.lens_manager.status = "The lens-profile directory is unavailable".into();
            return;
        };
        let (tx, rx) = crossbeam_channel::bounded(1);
        let context = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(run(action, directory));
            context.request_repaint();
        });
        self.state.lens_manager.job = Some(rx);
    }
}

pub(super) fn window(
    ctx: &egui::Context,
    state: &mut State,
    profiles: &[sp_core::lens::LensProfile],
) {
    if !state.open {
        return;
    }
    let mut open = true;
    egui::Window::new(crate::i18n::text("Lens profiles and flat-field calibration")).open(&mut open).default_width(580.0).show(ctx,|ui|{
        ui.label(crate::i18n::text("Profiles contain your measured optical shading and saved distortion geometry."));
        ui.add_enabled_ui(state.job.is_none(),|ui|{
            if ui.button(crate::i18n::text("Import lens profile…")).clicked(){state.action=Some(Action::Import);}
            egui::ScrollArea::vertical().max_height(180.0).show(ui,|ui|{
                for (index,profile) in profiles.iter().enumerate(){if ui.selectable_label(index==state.selected,{ let __l10n_args = (&(profile.name), &(profile.apertures.len()), &(profile.grid_w), &(profile.grid_h),); crate::i18n::format("{0} · {1} aperture grids · {2} × {3}", &[__l10n_args.0.to_string(), __l10n_args.1.to_string(), __l10n_args.2.to_string(), __l10n_args.3.to_string()]) }).clicked(){state.selected=index;}}
            });
            if let Some(profile)=profiles.get(state.selected){ui.horizontal(|ui|{if ui.button(crate::i18n::text("Export profile and grid…")).clicked(){state.action=Some(Action::Export(profile.name.clone()));}
        if ui.button(crate::i18n::text("Disable profile")).on_hover_text(crate::i18n::text("Keep a recoverable copy in the disabled profile folder.")).clicked(){state.action=Some(Action::Disable(profile.name.clone()));}});}
            ui.separator();ui.heading(crate::i18n::text("Calibrate lens shading"));
            ui.label(crate::i18n::text("Photograph a uniformly illuminated diffuser or flat target, defocused and without clipped highlights. Keep the same camera, lens and focal length; vary the aperture. Uneven lighting is recorded as correction, so inspect the result on another flat field."));
            ui.horizontal(|ui|{ui.label(crate::i18n::text("Profile name"));ui.text_edit_singleline(&mut state.name);});
            if ui.add_enabled(!state.name.trim().is_empty(),egui::Button::new(crate::i18n::text("Choose RAW flat fields and calibrate…"))).clicked(){state.action=Some(Action::Calibrate(state.name.clone()));}
            ui.small(crate::i18n::text("Geometry: use the Lens panel to mark straight lines, fit distortion, then save geometry to a selected profile."));
        });
        if state.job.is_some(){ui.spinner();ui.label(crate::i18n::text("Working…"));}
        if !state.status.is_empty(){ui.label(crate::i18n::text(&state.status));}
    });
    state.open = open;
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn publish_never_replaces_a_pair() {
        let dir = tempfile::tempdir().unwrap();
        let profile = sp_core::lens::LensProfile::identity(
            "Test",
            "test",
            sp_core::lens::GridGeometry::for_sensor(128, 128, 32.0),
        );
        let first = publish(dir.path(), "../test", &profile).unwrap();
        let second = publish(dir.path(), "../test", &profile).unwrap();
        assert_ne!(first, second);
        assert_eq!(sp_raw::lens_store::load_lens(&first).unwrap(), profile);
        assert_eq!(first.parent(), Some(dir.path()));
    }
}
