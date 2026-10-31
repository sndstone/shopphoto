//! External-editor round trip (UX-02): render a full-resolution 16-bit TIFF,
//! launch the chosen editor, detect its first save, then register the TIFF as
//! a catalog companion of the source RAW.

use super::*;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct Recipe {
    pub version: u32,
    pub name: String,
    pub editor: Option<PathBuf>,
    pub arguments: Vec<String>,
    pub color_space: sp_export::ExportColorSpace,
    pub depth: sp_export::TiffDepth,
}

impl Default for Recipe {
    fn default() -> Self {
        Self {
            version: 1,
            name: "16-bit TIFF".into(),
            editor: None,
            arguments: Vec::new(),
            color_space: sp_export::ExportColorSpace::AdobeRgb,
            depth: sp_export::TiffDepth::Sixteen,
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct RecipeState {
    pub open: bool,
    pub recipes: Vec<Recipe>,
    pub selected: Option<usize>,
    pub draft: Recipe,
    pub editor_draft: String,
    pub arguments_draft: String,
    pub dirty: bool,
    pub monitoring: bool,
    pub stop_requested: bool,
}

impl RecipeState {
    pub(crate) fn edit_selected(&mut self) {
        if let Some(recipe) = self.selected.and_then(|index| self.recipes.get(index)) {
            self.draft = recipe.clone();
            self.editor_draft = recipe
                .editor
                .as_ref()
                .map_or_else(String::new, |path| path.display().to_string());
            self.arguments_draft = recipe.arguments.join("\n");
        }
    }
}

fn recipes_path() -> Option<PathBuf> {
    dirs::config_dir().map(|path| path.join("shopphoto/external-recipes.json"))
}

pub(super) fn load_recipes() -> RecipeState {
    let recipes = recipes_path()
        .and_then(|path| std::fs::read(path).ok())
        .and_then(|bytes| serde_json::from_slice::<Vec<Recipe>>(&bytes).ok())
        .unwrap_or_default()
        .into_iter()
        .filter(|recipe| recipe.version == 1)
        .collect();
    RecipeState {
        recipes,
        ..Default::default()
    }
}

pub(super) fn recipe_window(ctx: &egui::Context, state: &mut RecipeState) {
    if !state.open {
        return;
    }
    egui::Window::new(crate::i18n::text("External editor recipes")).open(&mut state.open).resizable(true).show(ctx,|ui| {
        ui.label(crate::i18n::text("Named TIFF settings and editor commands for photographic round trips."));
        for (index,recipe) in state.recipes.iter().enumerate() {
            if ui.selectable_label(state.selected==Some(index),&recipe.name).clicked() {
                state.selected=Some(index);state.draft=recipe.clone();state.editor_draft=recipe.editor.as_ref().map_or_else(String::new,|path|path.display().to_string());state.arguments_draft=recipe.arguments.join("\n");
            }
        }
        ui.separator();
        ui.horizontal(|ui| {ui.label(crate::i18n::text("Name"));ui.text_edit_singleline(&mut state.draft.name);});
        ui.horizontal(|ui| {ui.label(crate::i18n::text("Editor path (blank: system default)"));ui.text_edit_singleline(&mut state.editor_draft);});
        ui.label(crate::i18n::text("Arguments: one per line; {file} inserts the TIFF path. If omitted, the file is appended."));
        ui.text_edit_multiline(&mut state.arguments_draft);
        egui::ComboBox::from_id_salt("external-recipe-space").selected_text(format!("{:?}",state.draft.color_space)).show_ui(ui,|ui| {
            for (space,label) in [(sp_export::ExportColorSpace::Srgb,"sRGB"),(sp_export::ExportColorSpace::DisplayP3,"Display P3"),(sp_export::ExportColorSpace::AdobeRgb,"Adobe RGB"),(sp_export::ExportColorSpace::Rec2020,"Rec.2020")] {ui.selectable_value(&mut state.draft.color_space,space,label);}
        });
        ui.horizontal(|ui| {ui.selectable_value(&mut state.draft.depth,sp_export::TiffDepth::Sixteen,crate::i18n::text("16-bit TIFF"));ui.selectable_value(&mut state.draft.depth,sp_export::TiffDepth::Eight,crate::i18n::text("8-bit TIFF"));});
        ui.horizontal(|ui| {
            if ui.button(crate::i18n::text("New")).clicked() {state.selected=None;state.draft=Recipe::default();state.editor_draft.clear();state.arguments_draft.clear();}
            if ui.add_enabled(!state.draft.name.trim().is_empty(),egui::Button::new(crate::i18n::text("Save recipe"))).clicked() {
                state.draft.editor=(!state.editor_draft.trim().is_empty()).then(||PathBuf::from(state.editor_draft.trim()));
                state.draft.arguments=state.arguments_draft.lines().filter(|line|!line.is_empty()).map(str::to_owned).collect();
                if let Some(index)=state.selected {if let Some(recipe)=state.recipes.get_mut(index){*recipe=state.draft.clone();}}
                else {state.recipes.push(state.draft.clone());state.selected=Some(state.recipes.len()-1);}
                state.dirty=true;
            }
            if let Some(index)=state.selected {if ui.button(crate::i18n::text("Remove recipe")).clicked(){state.recipes.remove(index);state.selected=None;state.dirty=true;}}
        });
    });
}

impl ShopPhotoApp {
    pub(super) fn pick_external_editor(&mut self) {
        if self.external_editor_pick.is_some() {
            return;
        }
        let (tx, rx) = crossbeam_channel::bounded(1);
        std::thread::Builder::new()
            .name("sp-external-editor-dialog".into())
            .spawn(move || {
                let picked = rfd::FileDialog::new()
                    .set_title(crate::i18n::text("Choose an image editor"))
                    .pick_file();
                let _ = tx.send(picked);
            })
            .expect("spawn external editor dialog");
        self.external_editor_pick = Some(rx);
    }

    pub(super) fn start_external_edit(&mut self, ctx: &egui::Context) {
        if self.external_edit_source.is_some() {
            self.state
                .push_toast("An external edit is already in progress", false);
            return;
        }
        let Some(source) = self.state.open_image.clone() else {
            self.state
                .push_toast("Open a photograph before editing externally", true);
            return;
        };
        let Some(destination) = source.parent().map(PathBuf::from) else {
            self.state
                .push_toast("The source has no output folder", true);
            return;
        };

        // Reuse the normal, tested export worker with a temporary dedicated
        // configuration. `start_export` clones it before returning, so the
        // photographer's normal export preset can be restored immediately.
        let saved_settings = self.state.export_settings.clone();
        let saved_batch = self.state.export_batch;
        let recipe = self
            .state
            .external
            .recipes
            .get(self.state.external.selected.unwrap_or(usize::MAX))
            .cloned();
        self.state.export_settings = sp_export::ExportSettings {
            format: sp_export::ExportFormat::Tiff {
                bit_depth: recipe
                    .as_ref()
                    .map_or(sp_export::TiffDepth::Sixteen, |recipe| recipe.depth),
                compressed: true,
            },
            color_space: recipe
                .as_ref()
                .map_or(saved_settings.color_space, |recipe| recipe.color_space),
            size: sp_export::SizeMode::Original,
            output_sharpening: sp_export::OutputSharpening::None,
            strip_gps: saved_settings.strip_gps,
            copy_metadata: true,
            copy_descriptive_metadata: saved_settings.copy_descriptive_metadata,
            destination,
            filename_pattern: recipe.as_ref().map_or_else(
                || "{name}-external".into(),
                |recipe| {
                    format!(
                        "{{name}}-{}",
                        recipe
                            .name
                            .chars()
                            .map(|c| if matches!(c, '/' | '\\' | ':' | '\0') {
                                '_'
                            } else {
                                c
                            })
                            .collect::<String>()
                    )
                },
            ),
            watermark: None,
            border: None,
            ..sp_export::ExportSettings::default()
        };
        self.state.export_batch = false;
        self.start_export(ctx);
        self.state.export_settings = saved_settings;
        self.state.export_batch = saved_batch;
        if self.export_job.is_some() {
            self.external_edit_recipe = recipe;
            self.external_edit_source = Some(source);
            self.state
                .push_toast("Rendering a 16-bit TIFF for the external editor…", false);
        }
    }

    pub(super) fn begin_external_monitor(&mut self, source: PathBuf, derivative: PathBuf) {
        if let Some(cancel) = self.external_monitor_cancel.take() {
            cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        self.external_edit_job = None;
        self.state.external.monitoring = false;
        let mut tracker = SaveTracker::new(file_signature(&derivative));
        let recipe = self.external_edit_recipe.take();
        let editor = std::env::var_os("SHOPPHOTO_EXTERNAL_EDITOR")
            .map(PathBuf::from)
            .or_else(|| recipe.as_ref().and_then(|recipe| recipe.editor.clone()))
            .or_else(|| self.state.external_editor.clone());
        let arguments = recipe
            .as_ref()
            .map_or(&[][..], |recipe| recipe.arguments.as_slice());
        if let Err(error) = launch_editor(editor.as_deref(), &derivative, arguments) {
            self.state.push_toast(
                crate::i18n::format(
                    "Could not launch external editor: {0}",
                    &[error.to_string()],
                ),
                true,
            );
            return;
        }
        let catalog = self.catalog_path.clone();
        let (tx, rx) = crossbeam_channel::bounded(8);
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.external_monitor_cancel = Some(cancel.clone());
        self.external_edit_seen_save = false;
        self.state.external.monitoring = true;
        std::thread::Builder::new()
            .name("sp-external-edit-watch".into())
            .spawn(move || {
                // Twelve hours permits a long retouching session without a
                // permanently parked watcher if an editor never saves.
                for _ in 0..86_400 {
                    std::thread::sleep(std::time::Duration::from_millis(500));
                    if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                        return;
                    }
                    let now = file_signature(&derivative);
                    if tracker.observe(now) {
                        // Wait for a stable save and a valid decodable image.
                        // Editors often replace the TIFF in several writes.
                        if sp_raw::decode_source_preview(&derivative).is_err() {
                            continue;
                        }
                        let result = catalog
                            .as_deref()
                            .ok_or_else(|| "Catalog storage is unavailable".to_owned())
                            .and_then(|catalog| {
                                let db = sp_catalog::CatalogDb::open(catalog)
                                    .map_err(|error| error.to_string())?;
                                db.add_companion(&source, &derivative)
                                    .map_err(|error| error.to_string())
                            });
                        let event = match result {
                            Ok(()) => ExternalEditEvent::Saved {
                                source: source.clone(),
                                derivative: derivative.clone(),
                            },
                            Err(error) => ExternalEditEvent::Failed(error),
                        };
                        if tx.send(event).is_err() {
                            return;
                        }
                        tracker.accept(now);
                    }
                }
                let _ = tx.send(ExternalEditEvent::Failed(
                    "External edit was not saved within 12 hours".into(),
                ));
            })
            .expect("spawn external edit watcher");
        self.external_edit_job = Some(rx);
    }

    pub(super) fn poll_external_edit(&mut self, ctx: &egui::Context) {
        if std::mem::take(&mut self.state.external.stop_requested) {
            if let Some(cancel) = self.external_monitor_cancel.take() {
                cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            }
            self.external_edit_job = None;
            self.state.external.monitoring = false;
        }
        if std::mem::take(&mut self.state.external.dirty) {
            let result = (|| -> Result<(), String> {
                let path = recipes_path()
                    .ok_or_else(|| "Configuration directory unavailable".to_owned())?;
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                }
                let temporary = path.with_extension("json.tmp");
                std::fs::write(
                    &temporary,
                    serde_json::to_vec_pretty(&self.state.external.recipes)
                        .map_err(|e| e.to_string())?,
                )
                .map_err(|e| e.to_string())?;
                std::fs::rename(temporary, path).map_err(|e| e.to_string())
            })();
            if let Err(error) = result {
                self.state.push_toast(
                    crate::i18n::format(
                        "Could not save editor recipes: {0}",
                        std::slice::from_ref(&error),
                    ),
                    true,
                );
            }
        }
        if let Some(rx) = &self.external_editor_pick {
            if let Ok(picked) = rx.try_recv() {
                self.external_editor_pick = None;
                if let Some(path) = picked {
                    self.state.external_editor = Some(path);
                }
            }
        }
        let event = self
            .external_edit_job
            .as_ref()
            .and_then(|receiver| receiver.try_recv().ok());
        if let Some(event) = event {
            match event {
                ExternalEditEvent::Saved { source, derivative } => {
                    self.state.preview_quality.remove(&derivative);
                    self.thumbs.remove(&derivative);
                    self.thumbnails_requested.remove(&derivative);
                    self.state.thumbnail_failures.remove(&derivative);
                    if self.state.open_image.as_ref() == Some(&derivative) {
                        self.start_decode(derivative.clone(), ctx);
                    }
                    if !self.external_edit_seen_save {
                        self.state.selected_photos.clear();
                        self.state.selected_photos.insert(derivative.clone());
                        self.state.mode = Mode::Library;
                        self.external_edit_seen_save = true;
                    }
                    self.state.push_toast(
                        {
                            let __l10n_args = (
                                &(derivative.file_name().unwrap_or_default().to_string_lossy()),
                                &(source.file_name().unwrap_or_default().to_string_lossy()),
                            );
                            crate::i18n::format(
                                "External edit saved · {0} is linked to {1}",
                                &[__l10n_args.0.to_string(), __l10n_args.1.to_string()],
                            )
                        },
                        false,
                    );
                    self.refresh_catalog(Vec::new(), None);
                }
                ExternalEditEvent::Failed(error) => {
                    self.external_edit_job = None;
                    self.state.external.monitoring = false;
                    if let Some(cancel) = self.external_monitor_cancel.take() {
                        cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                    self.state.push_toast(
                        crate::i18n::format(
                            "External edit failed: {0}",
                            std::slice::from_ref(&error),
                        ),
                        true,
                    );
                }
            }
        }
        if self.external_edit_job.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(500));
        }
    }
}

type FileSignature = (std::time::SystemTime, u64);

struct SaveTracker {
    baseline: Option<FileSignature>,
    pending: Option<FileSignature>,
}
impl SaveTracker {
    fn new(baseline: Option<FileSignature>) -> Self {
        Self {
            baseline,
            pending: None,
        }
    }
    fn observe(&mut self, now: Option<FileSignature>) -> bool {
        if now.is_none() || now == self.baseline {
            self.pending = None;
            return false;
        }
        if self.pending == now {
            true
        } else {
            self.pending = now;
            false
        }
    }
    fn accept(&mut self, now: Option<FileSignature>) {
        self.baseline = now;
        self.pending = None;
    }
}

fn file_signature(path: &std::path::Path) -> Option<FileSignature> {
    let metadata = std::fs::metadata(path).ok()?;
    Some((metadata.modified().ok()?, metadata.len()))
}

fn resolved_arguments(arguments: &[String], file: &std::path::Path) -> Vec<std::ffi::OsString> {
    let mut contains_file = false;
    let mut result = arguments
        .iter()
        .map(|argument| {
            contains_file |= argument.contains("{file}");
            if argument == "{file}" {
                file.as_os_str().to_owned()
            } else {
                argument.replace("{file}", &file.to_string_lossy()).into()
            }
        })
        .collect::<Vec<_>>();
    if !contains_file {
        result.push(file.as_os_str().to_owned());
    }
    result
}

fn launch_editor(
    editor: Option<&std::path::Path>,
    file: &std::path::Path,
    arguments: &[String],
) -> std::io::Result<()> {
    let mut command = if let Some(editor) = editor {
        #[cfg(target_os = "macos")]
        if editor
            .extension()
            .is_some_and(|extension| extension == "app")
        {
            let mut command = std::process::Command::new("open");
            command.arg("-a").arg(editor);
            if arguments.is_empty() {
                command.arg(file);
            } else {
                command
                    .arg("--args")
                    .args(resolved_arguments(arguments, file));
            }
            command
        } else {
            let mut command = std::process::Command::new(editor);
            command.args(resolved_arguments(arguments, file));
            command
        }
        #[cfg(not(target_os = "macos"))]
        {
            let mut command = std::process::Command::new(editor);
            command.args(resolved_arguments(arguments, file));
            command
        }
    } else {
        #[cfg(target_os = "macos")]
        {
            let mut command = std::process::Command::new("open");
            command.arg(file);
            command
        }
        #[cfg(target_os = "windows")]
        {
            let mut command = std::process::Command::new("cmd");
            command.arg("/C").arg("start").arg("").arg(file);
            command
        }
        #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
        {
            let mut command = std::process::Command::new("xdg-open");
            command.arg(file);
            command
        }
    };
    command.spawn().map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn editor_arguments_preserve_literal_shell_characters_and_spaces() {
        let file = std::path::Path::new("/photos/portrait $(literal); title.tif");
        let arguments = vec!["--new-window".into(), "{file}".into()];
        let resolved = resolved_arguments(&arguments, file);
        assert_eq!(
            resolved,
            [
                std::ffi::OsString::from("--new-window"),
                file.as_os_str().to_owned()
            ]
        );
        assert_eq!(resolved_arguments(&[], file), [file.as_os_str().to_owned()]);
        let recipe = Recipe {
            arguments,
            ..Default::default()
        };
        let restored: Recipe =
            serde_json::from_slice(&serde_json::to_vec(&recipe).unwrap()).unwrap();
        assert_eq!(restored.arguments, recipe.arguments);
        assert_eq!(restored.depth, sp_export::TiffDepth::Sixteen);
    }
    #[test]
    fn repeated_saves_are_reported_only_after_a_stable_signature() {
        let old = Some((std::time::UNIX_EPOCH, 100));
        let first = Some((std::time::UNIX_EPOCH, 200));
        let next = Some((std::time::UNIX_EPOCH, 300));
        let mut tracker = SaveTracker::new(old);
        assert!(!tracker.observe(old));
        assert!(!tracker.observe(first));
        assert!(tracker.observe(first));
        tracker.accept(first);
        assert!(!tracker.observe(first));
        assert!(!tracker.observe(next));
        assert!(!tracker.observe(None));
        assert!(!tracker.observe(next));
        assert!(tracker.observe(next));
    }
}
