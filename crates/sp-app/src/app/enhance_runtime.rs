//! Explicit optional-runtime installation and offline readiness inspection.
use crate::state::AppState;
use std::{
    path::Path,
    process::{Command, Stdio},
};

#[derive(Debug, Clone, Copy)]
pub(crate) enum Action {
    Check,
    Install,
}

#[derive(Debug)]
pub(crate) struct State {
    pub open: bool,
    pub action: Option<Action>,
    pub busy: bool,
    pub message: String,
    pub report: Option<serde_json::Value>,
    pub bootstrap_python: String,
    pub download_masks: bool,
    pub download_upscale: bool,
    pub download_semantic: bool,
    pub download_text: bool,
}

impl Default for State {
    fn default() -> Self {
        Self {
            open: false,
            action: None,
            busy: false,
            message: String::new(),
            report: None,
            bootstrap_python: if cfg!(windows) { "python" } else { "python3" }.into(),
            download_masks: false,
            download_upscale: false,
            download_semantic: false,
            download_text: false,
        }
    }
}

enum Event {
    Progress(String),
    Finished(Result<serde_json::Value, String>),
}

#[derive(Default)]
pub(super) struct Controller {
    job: Option<crossbeam_channel::Receiver<Event>>,
}

fn materialize(directory: &Path) -> Result<(), String> {
    for (name, content) in [
        ("enhance.py", include_str!("../../../../scripts/enhance.py")),
        (
            "setup-enhance.py",
            include_str!("../../../../scripts/setup-enhance.py"),
        ),
        (
            "enhance-requirements.txt",
            include_str!("../../../../scripts/enhance-requirements.txt"),
        ),
    ] {
        std::fs::write(directory.join(name), content).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn check(directory: &Path) -> Result<serde_json::Value, String> {
    let python = super::enhance_worker::python();
    if !python.is_file() {
        return Err({
            let __l10n_args = (&(python.display()),);
            crate::i18n::format("Runtime is not installed at {0}. Select a Python 3.11+ executable and use Install runtime.", &[__l10n_args.0.to_string()])
        });
    }
    let output = Command::new(python)
        .arg(directory.join("enhance.py"))
        .arg("--status")
        .env("HF_HUB_OFFLINE", "1")
        .env("TRANSFORMERS_OFFLINE", "1")
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|e| crate::i18n::format("Could not read runtime status: {0}", &[e.to_string()]))
}

impl Controller {
    pub(super) fn upkeep(&mut self, state: &mut AppState, ctx: &egui::Context) {
        if let Some(receiver) = &self.job {
            let mut done = false;
            loop {
                match receiver.try_recv() {
                    Ok(Event::Progress(message)) => state.enhance_runtime.message = message,
                    Ok(Event::Finished(result)) => {
                        match result {
                            Ok(report) => {
                                state.enhance_runtime.message = if report["ready"] == true {
                                    "Runtime ready. Model availability is listed below."
                                } else {
                                    "Runtime needs attention; see missing packages or errors below."
                                }
                                .into();
                                state.enhance_runtime.report = Some(report);
                            }
                            Err(error) => {
                                state.enhance_runtime.message = error;
                                state.enhance_runtime.report = None;
                            }
                        }
                        done = true;
                        break;
                    }
                    Err(crossbeam_channel::TryRecvError::Empty) => break,
                    Err(crossbeam_channel::TryRecvError::Disconnected) => {
                        state.enhance_runtime.message =
                            "Runtime task stopped unexpectedly. Retry the readiness check.".into();
                        done = true;
                        break;
                    }
                }
            }
            if done {
                self.job = None;
                state.enhance_runtime.busy = false;
            }
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        if self.job.is_some() {
            return;
        }
        let Some(action) = state.enhance_runtime.action.take() else {
            return;
        };
        if state.export_busy || state.enhancement_busy {
            state.enhance_runtime.message =
                "Wait for the current enhancement or export to finish before changing its runtime."
                    .into();
            return;
        }
        let python = state.enhance_runtime.bootstrap_python.clone();
        let masks = state.enhance_runtime.download_masks;
        let upscale = state.enhance_runtime.download_upscale;
        let semantic = state.enhance_runtime.download_semantic;
        let text = state.enhance_runtime.download_text;
        let (tx, rx) = crossbeam_channel::unbounded();
        let repaint = ctx.clone();
        std::thread::spawn(move || {
            let result = (|| {
                let directory = tempfile::tempdir().map_err(|e| e.to_string())?;
                materialize(directory.path())?;
                if matches!(action, Action::Install) {
                    // An override belongs to the user: the managed installer
                    // must not overwrite an arbitrary existing environment.
                    if std::env::var_os("SHOPPHOTO_ENHANCE_PYTHON").is_some() {
                        return Err("SHOPPHOTO_ENHANCE_PYTHON points to a custom environment. Unset it to install the managed runtime, or install enhancement requirements in that environment yourself.".into());
                    }
                    let log_path = directory.path().join("install.log");
                    let log = std::fs::File::create(&log_path).map_err(|e| e.to_string())?;
                    let mut command = Command::new(python);
                    command
                        .arg("-u")
                        .arg(directory.path().join("setup-enhance.py"));
                    if !masks {
                        command.arg("--no-models");
                    }
                    if text {
                        command.arg("--text");
                    }
                    if semantic {
                        command.arg("--semantic");
                    }
                    if upscale {
                        command.arg("--upscale-swinir").arg("--upscale-swinir-4x");
                    }
                    command
                        .stdin(Stdio::null())
                        .stdout(log.try_clone().map_err(|e| e.to_string())?)
                        .stderr(log);
                    let mut child = command.spawn().map_err(|e| {
                        crate::i18n::format("Could not launch Python 3.11+: {0}", &[e.to_string()])
                    })?;
                    let mut previous = String::new();
                    loop {
                        let log = std::fs::read_to_string(&log_path).unwrap_or_default();
                        let latest = log
                            .lines()
                            .rev()
                            .find(|line| !line.trim().is_empty())
                            .unwrap_or("Installing runtime…")
                            .to_owned();
                        if latest != previous {
                            let _ = tx.send(Event::Progress(latest.clone()));
                            previous = latest;
                            repaint.request_repaint();
                        }
                        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
                            if !status.success() {
                                let tail = log
                                    .lines()
                                    .rev()
                                    .take(8)
                                    .collect::<Vec<_>>()
                                    .into_iter()
                                    .rev()
                                    .collect::<Vec<_>>()
                                    .join("\n");
                                return Err(crate::i18n::format(
                                    "Installation failed: {0}",
                                    std::slice::from_ref(&tail),
                                ));
                            }
                            break;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(300));
                    }
                }
                check(directory.path())
            })();
            let _ = tx.send(Event::Finished(result));
            repaint.request_repaint();
        });
        self.job = Some(rx);
        state.enhance_runtime.busy = true;
        state.enhance_runtime.message = match action {
            Action::Check => "Checking installed packages and cached models…",
            Action::Install => "Installing separate enhancement runtime…",
        }
        .into();
    }
}

pub(crate) fn window(ctx: &egui::Context, state: &mut State) {
    if !state.open {
        return;
    }
    egui::Window::new(crate::i18n::text("AI enhancement runtime")).open(&mut state.open).default_width(560.0).vscroll(true).show(ctx, |ui| {
        ui.label(crate::i18n::text("Object, sky and depth masks · RGB super resolution"));
        if sp_ai::enhancement::available() {
            ui.label(crate::i18n::text("Native Core ML models run locally without Python:"));
            let sky = if super::native_enhance::bundle("oneformer-ade").is_some() { ("oneformer-ade", "Sky (MIT model)") } else { ("maskformer-ade", "Sky (noncommercial model)") };
            for (id, name) in [sky, ("ssdlite-coco", "Individual people and vehicles"), ("slimsam-encoder", "Object segmentation encoder"), ("slimsam-box", "Object selection"), ("slimsam-points", "Include/exclude points"), ("vitmatte-small", "Full-resolution edge matting"), ("swinir-2x", "Faithful 2×"), ("swinir-4x", "Faithful 4×")] {
                ui.label(format!("{}: {}", crate::i18n::text(name), crate::i18n::text(if super::native_enhance::bundle(id).is_some() { "installed" } else { "not installed" })));
            }
            ui.label(crate::i18n::text("The personal macOS build includes native models. Python below supports additional models and portable fallback."));
            ui.separator();
        }
        ui.label(crate::i18n::text("This optional environment is separate from camera RAW denoise. Checks and inference use local models."));
        ui.monospace(super::enhance_worker::python().display().to_string());
        if ui.add_enabled(!state.busy, egui::Button::new(crate::i18n::text("Check readiness"))).clicked() { state.action = Some(Action::Check); }
        if let Some(report) = &state.report {
            ui.label({ let __l10n_args = (&(report["python"].as_str().unwrap_or("?")), &(report["device"].as_str().unwrap_or("?")),); crate::i18n::format("Python {0} · Device: {1}", &[__l10n_args.0.to_string(), __l10n_args.1.to_string()]) });
            if let Some(missing) = report["missing_packages"].as_array().filter(|missing| !missing.is_empty()) { ui.label({ let __l10n_args = (&(missing.iter().filter_map(|value| value.as_str()).collect::<Vec<_>>().join(", ")),); crate::i18n::format("Missing packages: {0}", std::slice::from_ref(__l10n_args.0)) }); }
            if let Some(error) = report["error"].as_str() { ui.colored_label(egui::Color32::LIGHT_RED, error); }
            if let Some(models) = report["models"].as_object() {
                for (name, ready) in models { ui.label(format!("{}: {}", name, crate::i18n::text(if *ready == true { "cached" } else { "not installed" }))); }
            }
        }
        ui.separator();
        ui.label(crate::i18n::text("Python 3.11+ executable (name or full path)"));
        ui.text_edit_singleline(&mut state.bootstrap_python);
        ui.label(crate::i18n::text("Install downloads Python packages. Select any model downloads explicitly:"));
        ui.checkbox(&mut state.download_masks, crate::i18n::text("Download object, depth and matting models"));
        ui.horizontal_wrapped(|ui| {
            ui.hyperlink_to(crate::i18n::text("SlimSAM license"), "https://huggingface.co/Zigeng/SlimSAM-uniform-77");
            ui.hyperlink_to(crate::i18n::text("Depth Anything license"), "https://huggingface.co/depth-anything/Depth-Anything-V2-Small-hf");
            ui.hyperlink_to(crate::i18n::text("ViTMatte license"), "https://huggingface.co/hustvl/vitmatte-small-composition-1k");
        });
        ui.checkbox(&mut state.download_semantic,crate::i18n::text("Download automatic people/sky model (noncommercial use only)"));
        ui.hyperlink_to(crate::i18n::text("People/sky model and license"),"https://huggingface.co/facebook/maskformer-swin-tiny-ade");
        ui.hyperlink_to(crate::i18n::text("Noncommercial license terms"),"https://github.com/facebookresearch/MaskFormer/blob/main/LICENSE");
        ui.checkbox(&mut state.download_text, crate::i18n::text("Download text selection and matting models"));
        ui.hyperlink_to(crate::i18n::text("Text selection model and license"), "https://huggingface.co/CIDAS/clipseg-rd64-refined");
        ui.checkbox(&mut state.download_upscale, crate::i18n::text("Download official SwinIR 2× and 4× checkpoints (129 MiB)"));
        ui.hyperlink_to(crate::i18n::text("SwinIR license and model"), "https://github.com/JingyunLiang/SwinIR");
        if ui.add_enabled(!state.busy && !state.bootstrap_python.trim().is_empty(), egui::Button::new(crate::i18n::text("Install runtime and selected models"))).clicked() { state.action = Some(Action::Install); }
        if state.busy { ui.spinner(); }
        if !state.message.is_empty() { ui.label(crate::i18n::text(&state.message)); }
    });
}
