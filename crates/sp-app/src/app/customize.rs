//! Persistent shortcut overrides, named workspaces and accessible presentation.
use super::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub(crate) enum Action {
    Open,
    Develop,
    Library,
    Compare,
    Zoom,
    Shadows,
    Highlights,
    Before,
    CompareMode,
    Export,
    Undo,
    Redo,
    Pick,
    Reject,
    Unflag,
    Mask,
    Debug,
}
impl Action {
    pub const ALL: [Self; 17] = [
        Self::Open,
        Self::Develop,
        Self::Library,
        Self::Compare,
        Self::Zoom,
        Self::Shadows,
        Self::Highlights,
        Self::Before,
        Self::CompareMode,
        Self::Export,
        Self::Undo,
        Self::Redo,
        Self::Pick,
        Self::Reject,
        Self::Unflag,
        Self::Mask,
        Self::Debug,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Open => "Open photo…",
            Self::Develop => "Develop",
            Self::Library => "Library grid",
            Self::Compare => "Compare 2–4 selected photos side by side",
            Self::Zoom => "Fit ↔ 100%",
            Self::Shadows => "Shadow clipping overlay",
            Self::Highlights => "Highlight clipping overlay",
            Self::Before => "Before / after (whole frame)",
            Self::CompareMode => "Cycle split / side-by-side / off",
            Self::Export => "Export…",
            Self::Undo => "Undo",
            Self::Redo => "Redo",
            Self::Pick => "Pick",
            Self::Reject => "Reject · Crop: swap ratio",
            Self::Unflag => "Unflag",
            Self::Mask => "Show AI mask coverage",
            Self::Debug => "Performance overlay",
        }
    }
    fn default_binding(self) -> Binding {
        let (key, command, shift) = match self {
            Self::Open => ("O", true, false),
            Self::Develop => ("D", false, false),
            Self::Library => ("G", false, false),
            Self::Compare => ("C", false, false),
            Self::Zoom => ("Z", false, false),
            Self::Shadows => ("J", false, false),
            Self::Highlights => ("O", false, false),
            Self::Before => ("Backslash", false, false),
            Self::CompareMode => ("Y", false, false),
            Self::Export => ("E", false, false),
            Self::Undo => ("Z", true, false),
            Self::Redo => ("Z", true, true),
            Self::Pick => ("P", false, false),
            Self::Reject => ("X", false, false),
            Self::Unflag => ("U", false, false),
            Self::Mask => ("M", false, false),
            Self::Debug => ("F12", false, false),
        };
        Binding {
            key: key.into(),
            command,
            shift,
            alt: false,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Binding {
    key: String,
    command: bool,
    shift: bool,
    alt: bool,
}
impl Binding {
    fn same_chord(&self, other: &Self) -> bool {
        egui::Key::from_name(&self.key) == egui::Key::from_name(&other.key)
            && self.command == other.command
            && self.shift == other.shift
            && self.alt == other.alt
    }
    fn matches(&self, i: &egui::InputState) -> bool {
        egui::Key::from_name(&self.key).is_some_and(|key| i.key_pressed(key))
            && i.modifiers.command == self.command
            && i.modifiers.shift == self.shift
            && i.modifiers.alt == self.alt
            && (self.command || !i.modifiers.ctrl)
    }
    fn display(&self) -> String {
        format!(
            "{}{}{}{}",
            if self.command {
                if cfg!(target_os = "macos") {
                    "⌘"
                } else {
                    "Ctrl+"
                }
            } else {
                ""
            },
            if self.alt { "Alt+" } else { "" },
            if self.shift { "Shift+" } else { "" },
            self.key
        )
    }
}
pub(crate) type Bindings = BTreeMap<Action, Binding>;
pub(crate) fn pressed(bindings: &Bindings, action: Action, input: &egui::InputState) -> bool {
    bindings
        .get(&action)
        .cloned()
        .unwrap_or_else(|| action.default_binding())
        .matches(input)
}
pub(crate) fn label_for(bindings: &Bindings, label: &str, fallback: &str) -> String {
    Action::ALL
        .into_iter()
        .find(|a| a.label() == label)
        .map_or_else(
            || fallback.into(),
            |a| {
                bindings
                    .get(&a)
                    .cloned()
                    .unwrap_or_else(|| a.default_binding())
                    .display()
            },
        )
}
fn validate(bindings: &Bindings) -> std::result::Result<(), String> {
    let resolved = Action::ALL.map(|a| {
        (
            a,
            bindings
                .get(&a)
                .cloned()
                .unwrap_or_else(|| a.default_binding()),
        )
    });
    for (i, (a, b)) in resolved.iter().enumerate() {
        let key = egui::Key::from_name(&b.key).ok_or("Unknown key")?;
        if matches!(
            key,
            egui::Key::Escape
                | egui::Key::Tab
                | egui::Key::Enter
                | egui::Key::ArrowLeft
                | egui::Key::ArrowRight
                | egui::Key::ArrowUp
                | egui::Key::ArrowDown
                | egui::Key::Num0
                | egui::Key::Num1
                | egui::Key::Num2
                | egui::Key::Num3
                | egui::Key::Num4
                | egui::Key::Num5
                | egui::Key::Num6
                | egui::Key::Num7
                | egui::Key::Num8
                | egui::Key::Num9
                | egui::Key::Questionmark
        ) {
            return Err("Navigation, ratings and help keys are reserved".into());
        }
        if b.command
            && matches!(
                key,
                egui::Key::A
                    | egui::Key::C
                    | egui::Key::F
                    | egui::Key::V
                    | egui::Key::X
                    | egui::Key::Q
                    | egui::Key::W
            )
        {
            return Err("That command is reserved for standard system or text actions".into());
        }
        if let Some((other, _)) = resolved[..i].iter().find(|(_, other)| other.same_chord(b)) {
            return Err({
                let __l10n_args = (&(a.label()), &(other.label()));
                crate::i18n::format(
                    "{0} conflicts with {1}",
                    &[__l10n_args.0.to_string(), __l10n_args.1.to_string()],
                )
            });
        }
    }
    Ok(())
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Workspace {
    name: String,
    panels: crate::state::PanelOpen,
    #[serde(default)]
    left_rail_tab: crate::state::LeftRailTab,
    rails_hidden: bool,
    library: bool,
    ui_scale: f32,
}
#[derive(Debug, Default)]
pub(crate) struct State {
    pub open: bool,
    pub bindings: Bindings,
    pub workspaces: Vec<Workspace>,
    pub high_contrast: bool,
    pub reduced_motion: bool,
    pub(super) draft: Option<Bindings>,
    pub(super) workspace_name: String,
    pub(super) error: Option<String>,
}
pub(super) fn window(ctx: &egui::Context, state: &mut AppState) {
    if !state.customize.open {
        return;
    }
    let mut open = true;
    egui::Window::new(crate::i18n::text("Settings")).open(&mut open).default_width(650.0).default_height(620.0).show(ctx,|ui|{
        egui::ScrollArea::vertical().max_height(560.0).show(ui,|ui|{
            ui.strong(crate::i18n::text("Workspaces"));ui.label(crate::i18n::text("Save the sidebar, panel expansion, visibility, Library/Develop mode and interface scale."));
            ui.horizontal(|ui|{ui.text_edit_singleline(&mut state.customize.workspace_name);if ui.add_enabled(!state.customize.workspace_name.trim().is_empty(),egui::Button::new(crate::i18n::text("Save current"))).clicked(){let name=state.customize.workspace_name.trim().to_owned();state.customize.workspaces.retain(|w|w.name!=name);state.customize.workspaces.push(Workspace{name,panels:state.panels.clone(),left_rail_tab:state.left_rail_tab,rails_hidden:state.rails_hidden,library:state.mode==Mode::Library,ui_scale:state.ui_scale});}});
            let mut load=None;let mut remove=None;
            for(index,workspace)in state.customize.workspaces.iter().enumerate(){ui.horizontal(|ui|{ui.label(&workspace.name);if ui.small_button(crate::i18n::text("Load")).clicked(){load=Some(index);}else if ui.small_button(crate::i18n::text("Delete")).clicked(){remove=Some(index);}});}
            if let Some(index)=load{let w=&state.customize.workspaces[index];state.panels=w.panels.clone();state.left_rail_tab=w.left_rail_tab;state.preset_preview=None;state.rails_hidden=w.rails_hidden;state.mode=if w.library{Mode::Library}else{Mode::Develop};state.ui_scale=w.ui_scale.clamp(0.75,2.0);state.ui_scale_draft=state.ui_scale;}
            if let Some(index)=remove{state.customize.workspaces.remove(index);}
            ui.separator();
            ui.strong(crate::i18n::text("Camera RAW denoise"));
            egui::ComboBox::from_id_salt("raw-denoise-compute")
                .selected_text(crate::i18n::text(state.raw_denoise_compute.label()))
                .show_ui(ui, |ui| {
                    for units in [sp_ai::ComputeUnits::Automatic, sp_ai::ComputeUnits::CpuGpu, sp_ai::ComputeUnits::CpuNeuralEngine, sp_ai::ComputeUnits::CpuOnly] {
                        ui.add_enabled_ui(units.supported(), |ui| {
                            ui.selectable_value(&mut state.raw_denoise_compute, units, crate::i18n::text(units.label()));
                        });
                    }
                });
            ui.label(crate::i18n::text("Core ML may use the CPU for unsupported layers. Neural Engine requires Apple Silicon. Changing this reruns active RAW denoise."));
            ui.separator();ui.strong(crate::i18n::text("Accessibility"));ui.checkbox(&mut state.customize.high_contrast,crate::i18n::text("Stronger control boundaries and keyboard focus"));ui.checkbox(&mut state.customize.reduced_motion,crate::i18n::text("Reduce interface animation"));ui.label(crate::i18n::text("Tab moves keyboard focus. Arrow keys adjust focused sliders; text fields keep typing shortcuts. Interface scale is in View."));
            ui.separator();ui.strong(crate::i18n::text("Keyboard shortcuts"));
            let draft=state.customize.draft.get_or_insert_with(||state.customize.bindings.clone());
            for action in Action::ALL{let binding=draft.entry(action).or_insert_with(||action.default_binding());ui.push_id(action,|ui|{ui.horizontal(|ui|{ui.label(crate::i18n::text(action.label()));egui::ComboBox::from_id_salt("key").selected_text(&binding.key).width(80.0).show_ui(ui,|ui|{for key in egui::Key::ALL{let name=key.name();ui.selectable_value(&mut binding.key,name.into(),name);}});ui.checkbox(&mut binding.command,if cfg!(target_os="macos"){"⌘"}else{"Ctrl"});ui.checkbox(&mut binding.shift,crate::i18n::text("Shift"));ui.checkbox(&mut binding.alt,crate::i18n::text("Alt"));});});}
            let result=validate(draft);if let Err(error)=&result{ui.colored_label(theme::ERROR,error);}
            ui.horizontal(|ui|{if ui.add_enabled(result.is_ok(),egui::Button::new(crate::i18n::text("Apply shortcuts"))).clicked(){state.customize.bindings=draft.clone();}else if ui.button(crate::i18n::text("Reset defaults")).clicked(){draft.clear();state.customize.bindings.clear();}});
            if let Some(error)=&state.customize.error{ui.colored_label(theme::ERROR,error);}
        });
    });
    state.customize.open = open;
}
pub(super) fn apply_presentation(ctx: &egui::Context, high_contrast: bool, reduced_motion: bool) {
    theme::apply(ctx);
    ctx.style_mut_of(egui::Theme::Dark, |s| {
        if reduced_motion {
            s.animation_time = 0.0;
        }
        if high_contrast {
            s.visuals.widgets.inactive.bg_stroke = egui::Stroke::new(1.5, theme::TEXT_SECONDARY);
            s.visuals.widgets.hovered.bg_stroke = egui::Stroke::new(2.0, theme::TEXT_PRIMARY);
            s.visuals.widgets.active.bg_stroke = egui::Stroke::new(2.5, theme::TEXT_PRIMARY);
        }
    });
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn customization_rejects_collisions_and_reserved_navigation() {
        let mut map = Bindings::new();
        assert!(validate(&map).is_ok());
        map.insert(Action::Export, Action::Open.default_binding());
        assert!(validate(&map).is_err());
        map.insert(
            Action::Export,
            Binding {
                key: "ArrowRight".into(),
                command: false,
                alt: false,
                shift: false,
            },
        );
        assert!(validate(&map).is_err());
        map.insert(
            Action::Export,
            Binding {
                key: "F8".into(),
                command: false,
                alt: false,
                shift: false,
            },
        );
        assert!(validate(&map).is_ok());
    }
}
