//! Optional gphoto2 capture, gated by detected device capabilities.
use super::*;
use std::{
    ffi::{OsStr, OsString},
    io::Read,
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

#[derive(Debug, Clone)]
struct Camera {
    model: String,
    port: String,
    capture: bool,
    details: String,
}
#[derive(Debug, Clone, Copy)]
pub(crate) enum Action {
    Detect,
    ChooseFolder,
    Capture,
    WatchFolder,
}
#[derive(Debug, Default)]
pub(crate) struct State {
    pub open: bool,
    pub action: Option<Action>,
    busy: bool,
    status: String,
    cameras: Vec<Camera>,
    selected: usize,
    destination: Option<PathBuf>,
    version: String,
    worker: Option<Receiver<Result<Outcome, String>>>,
    cancel: Option<Arc<AtomicBool>>,
    pending_scan: Option<PathBuf>,
}
#[derive(Debug)]
enum Outcome {
    Detected(String, Vec<Camera>),
    Folder(Option<PathBuf>),
    Captured(Vec<PathBuf>, Vec<String>),
}
impl Drop for State {
    fn drop(&mut self) {
        if let Some(cancel) = &self.cancel {
            cancel.store(true, Ordering::Relaxed);
        }
    }
}

fn backend() -> OsString {
    std::env::var_os("SHOPPHOTO_GPHOTO2").unwrap_or_else(|| "gphoto2".into())
}
fn command(
    executable: &OsStr,
    args: &[&OsStr],
    directory: Option<&std::path::Path>,
    cancel: &AtomicBool,
    timeout: Duration,
) -> Result<String, String> {
    if cancel.load(Ordering::Relaxed) {
        return Err("Camera operation cancelled".into());
    }
    let log = tempfile::NamedTempFile::new().map_err(|e| e.to_string())?;
    let mut cmd = Command::new(executable);
    cmd.args(args)
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::from(log.reopen().map_err(|e| e.to_string())?))
        .stderr(Stdio::from(log.reopen().map_err(|e| e.to_string())?));
    // Independent append handles avoid concurrent stdout/stderr overwrites.
    let stdout = std::fs::OpenOptions::new()
        .append(true)
        .open(log.path())
        .map_err(|e| e.to_string())?;
    let stderr = stdout.try_clone().map_err(|e| e.to_string())?;
    cmd.stdout(stdout).stderr(stderr);
    if let Some(directory) = directory {
        cmd.current_dir(directory);
    }
    let mut child=cmd.spawn().map_err(|e|crate::i18n::format("gphoto2 backend unavailable: {0}. Install gphoto2 separately, or set SHOPPHOTO_GPHOTO2 to its executable.", &[e.to_string()]))?;
    let started = Instant::now();
    let status = loop {
        if cancel.load(Ordering::Relaxed)
            || started.elapsed() > timeout
            || log
                .as_file()
                .metadata()
                .map_or(true, |m| m.len() > 8 * 1024 * 1024)
        {
            let _ = child.kill();
            let _ = child.wait();
            return Err(if cancel.load(Ordering::Relaxed) {
                "Camera operation cancelled"
            } else {
                "Camera operation timed out or exceeded its diagnostic limit"
            }
            .into());
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error.to_string());
            }
        }
    };
    let mut output = String::new();
    log.reopen()
        .map_err(|e| e.to_string())?
        .take(8 * 1024 * 1024)
        .read_to_string(&mut output)
        .map_err(|e| e.to_string())?;
    if status.success() {
        Ok(output)
    } else {
        Err({
            let __l10n_args = (&(output.trim()),);
            crate::i18n::format(
                "Camera backend failed ({0}): {1}",
                &[status.to_string(), __l10n_args.0.to_string()],
            )
        })
    }
}
fn detected(output: &str) -> Vec<Camera> {
    output
        .lines()
        .filter_map(|line| {
            let port = line.split_whitespace().last()?;
            if !["usb:", "ptpip:", "serial:"]
                .iter()
                .any(|prefix| port.starts_with(prefix))
            {
                return None;
            }
            let model = line.strip_suffix(port)?.trim();
            if model.is_empty() {
                return None;
            }
            Some(Camera {
                model: model.into(),
                port: port.into(),
                capture: false,
                details: String::new(),
            })
        })
        .collect()
}
fn can_capture(output: &str) -> bool {
    let mut capture = false;
    for line in output.lines() {
        if let Some((key, value)) = line.split_once(':') {
            if key.trim() == "Capture choices" {
                capture = true;
                if value.trim() == "Image" {
                    return true;
                }
            } else if capture && key.trim().is_empty() {
                if value.trim() == "Image" {
                    return true;
                }
            } else if capture {
                break;
            }
        }
    }
    false
}
fn abilities(executable: &OsStr, camera: &Camera, cancel: &AtomicBool) -> Result<String, String> {
    command(
        executable,
        &[
            OsStr::new("--port"),
            OsStr::new(&camera.port),
            OsStr::new("--camera"),
            OsStr::new(&camera.model),
            OsStr::new("--abilities"),
        ],
        None,
        cancel,
        Duration::from_secs(20),
    )
}
fn detect(executable: &OsStr, cancel: &AtomicBool) -> Result<Outcome, String> {
    let version = command(
        executable,
        &[OsStr::new("--version")],
        None,
        cancel,
        Duration::from_secs(10),
    )?;
    let output = command(
        executable,
        &[OsStr::new("--auto-detect")],
        None,
        cancel,
        Duration::from_secs(20),
    )?;
    let mut cameras = detected(&output);
    for camera in &mut cameras {
        match abilities(executable, camera, cancel) {
            Ok(details) => {
                camera.capture = can_capture(&details);
                camera.details = details;
            }
            Err(error) => {
                camera.details = error;
            }
        }
    }
    Ok(Outcome::Detected(
        version.lines().next().unwrap_or("gphoto2").to_owned(),
        cameras,
    ))
}
fn capture(
    executable: &OsStr,
    camera: &Camera,
    destination: &std::path::Path,
    cancel: &AtomicBool,
) -> Result<Outcome, String> {
    if !camera.capture || !can_capture(&abilities(executable, camera, cancel)?) {
        return Err("Detected camera driver does not support still-image capture".into());
    }
    if !destination.is_dir() {
        return Err("Choose an existing capture destination folder".into());
    }
    let staging = tempfile::Builder::new()
        .prefix(".shopphoto-capture-")
        .tempdir_in(destination)
        .map_err(|e| e.to_string())?;
    command(
        executable,
        &[
            OsStr::new("--port"),
            OsStr::new(&camera.port),
            OsStr::new("--camera"),
            OsStr::new(&camera.model),
            OsStr::new("--capture-image-and-download"),
            OsStr::new("--keep"),
            OsStr::new("--filename"),
            OsStr::new("capture-%n.%C"),
        ],
        Some(staging.path()),
        cancel,
        Duration::from_secs(180),
    )?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_millis();
    let mut inputs = std::fs::read_dir(staging.path())
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    inputs.sort_by_key(|entry| entry.file_name());
    let mut saved = Vec::new();
    let mut warnings = Vec::new();
    for (index, entry) in inputs.into_iter().enumerate() {
        if !entry.file_type().map_err(|e| e.to_string())?.is_file() {
            continue;
        }
        let source = entry.path();
        let suffix = source.extension().and_then(|e| e.to_str()).unwrap_or("bin");
        let path = destination.join(format!("tether-{stamp}-{index}.{suffix}"));
        let result = (|| -> Result<(), String> {
            let mut temporary =
                tempfile::NamedTempFile::new_in(destination).map_err(|e| e.to_string())?;
            std::io::copy(
                &mut std::fs::File::open(&source).map_err(|e| e.to_string())?,
                &mut temporary,
            )
            .map_err(|e| e.to_string())?;
            temporary.as_file().sync_all().map_err(|e| e.to_string())?;
            temporary
                .persist_noclobber(&path)
                .map_err(|e| e.to_string())?;
            Ok(())
        })();
        match result {
            Ok(()) => {
                if !sp_raw::is_supported_image(&path) {
                    warnings.push({ let __l10n_args = (&(path.display()),); crate::i18n::format("Saved unsupported camera format: {0}", &[__l10n_args.0.to_string()]) });
                }
                saved.push(path);
            }
            Err(error) => warnings.push(crate::i18n::format("Could not save downloaded file: {0}. The backend was instructed to retain its camera copy.", std::slice::from_ref(&error))),
        }
    }
    if saved.is_empty() {
        return Err({
            let __l10n_args = (&(warnings.join("; ")),);
            crate::i18n::format(
                "Camera produced no saved files. {0}",
                std::slice::from_ref(__l10n_args.0),
            )
        });
    }
    Ok(Outcome::Captured(saved, warnings))
}

impl ShopPhotoApp {
    pub(super) fn tether_upkeep(&mut self, ctx: &egui::Context) {
        if let Some(receiver) = &self.state.tether.worker {
            let event = match receiver.try_recv() {
                Ok(value) => Some(value),
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    Some(Err("Camera worker stopped".into()))
                }
                Err(crossbeam_channel::TryRecvError::Empty) => None,
            };
            if let Some(event) = event {
                self.state.tether.worker = None;
                self.state.tether.cancel = None;
                self.state.tether.busy = false;
                match event {
                    Ok(Outcome::Detected(version, cameras)) => {
                        self.state.tether.status = if cameras.is_empty() {
                            "Backend ready; no connected camera detected".into()
                        } else {
                            {
                                let __l10n_args = (&(cameras.len()),);
                                crate::i18n::format(
                                    "Detected {0} camera(s)",
                                    &[__l10n_args.0.to_string()],
                                )
                            }
                        };
                        self.state.tether.version = version;
                        self.state.tether.cameras = cameras;
                        self.state.tether.selected = 0;
                    }
                    Ok(Outcome::Folder(folder)) => {
                        if let Some(folder) = folder {
                            self.state.tether.destination = Some(folder);
                        }
                    }
                    Ok(Outcome::Captured(paths, warnings)) => {
                        self.state.tether.status = {
                            let __l10n_args = (&(paths.len()), &(warnings.join("; ")));
                            crate::i18n::format(
                                "Saved {0} file(s). {1}",
                                &[__l10n_args.0.to_string(), __l10n_args.1.to_string()],
                            )
                        };
                        self.state.tether.pending_scan = self.state.tether.destination.clone();
                        if let Some(path) = paths
                            .iter()
                            .find(|p| sp_raw::is_supported(p))
                            .or_else(|| paths.iter().find(|p| sp_raw::is_supported_image(p)))
                        {
                            self.start_decode(path.clone(), ctx);
                        }
                    }
                    Err(error) => {
                        self.state.tether.status = error;
                    }
                }
            }
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        if self.catalog_job.is_none() && !self.state.catalog_tool_busy {
            if let Some(folder) = self.state.tether.pending_scan.take() {
                self.refresh_catalog(vec![folder], None);
            }
        }
        if let Some(action) = self.state.tether.action.take() {
            if self.state.tether.busy {
                return;
            }
            if matches!(action, Action::WatchFolder) {
                self.state.tether.pending_scan = self.state.tether.destination.clone();
                self.state.tether.status="Folder added to the Library scan; local folders use the existing file watcher. Save camera files here from Phocus or another capture application.".into();
                return;
            }
            let destination = self.state.tether.destination.clone();
            let camera = self
                .state
                .tether
                .cameras
                .get(self.state.tether.selected)
                .cloned();
            let cancel = Arc::new(AtomicBool::new(false));
            let cancelled = cancel.clone();
            let (tx, rx) = crossbeam_channel::bounded(1);
            let repaint = ctx.clone();
            std::thread::spawn(move || {
                let result = match action {
                    Action::Detect => detect(&backend(), &cancelled),
                    Action::ChooseFolder => Ok(Outcome::Folder(
                        rfd::FileDialog::new()
                            .set_title(crate::i18n::text("Choose tether capture folder"))
                            .pick_folder(),
                    )),
                    Action::Capture => match (camera, destination) {
                        (Some(camera), Some(folder)) => {
                            capture(&backend(), &camera, &folder, &cancelled)
                        }
                        _ => Err("Detect a camera and choose a folder first".into()),
                    },
                    Action::WatchFolder => unreachable!(),
                };
                let _ = tx.send(result);
                repaint.request_repaint();
            });
            self.state.tether.worker = Some(rx);
            self.state.tether.cancel = Some(cancel);
            self.state.tether.busy = true;
            self.state.tether.status = "Waiting for camera operation…".into();
        }
    }
}
pub(crate) fn window(ctx: &egui::Context, state: &mut State) {
    if !state.open {
        return;
    }
    egui::Window::new(crate::i18n::text("Tethered capture")).open(&mut state.open).default_width(560.0).show(ctx,|ui|{
        ui.label(crate::i18n::text("Capture through an installed gphoto2 backend, or watch a folder written by your camera software."));
        ui.add_enabled_ui(!state.busy,|ui|{
            if ui.button(crate::i18n::text("Detect backend and cameras")).clicked(){state.action=Some(Action::Detect);}
            if !state.version.is_empty(){ui.label(&state.version);}
            for (i,camera) in state.cameras.iter().enumerate(){ui.radio_value(&mut state.selected,i,format!("{} · {} · {}",camera.model,camera.port,if camera.capture{"still capture available"}else{"capture unavailable"}));}
            if let Some(camera)=state.cameras.get(state.selected){ui.collapsing(crate::i18n::text("Driver capabilities"),|ui|{ui.label(&camera.details);});}
            if ui.button(crate::i18n::text("Choose destination…")).clicked(){state.action=Some(Action::ChooseFolder);}
            if let Some(destination)=&state.destination{ui.label(destination.display().to_string());}
            if ui.add_enabled(state.destination.is_some() && state.cameras.get(state.selected).is_some_and(|c|c.capture),egui::Button::new(crate::i18n::text("Capture one image"))).clicked(){state.action=Some(Action::Capture);}
            if ui.add_enabled(state.destination.is_some(),egui::Button::new(crate::i18n::text("Watch folder from Phocus / other capture app"))).clicked(){state.action=Some(Action::WatchFolder);}
            ui.small(crate::i18n::text("Capture requests camera file retention. Close other applications using the camera before direct capture. Camera exposure settings stay on the camera; live view and remote configuration are unavailable."));
        });
        if state.busy{ui.spinner();if ui.button(crate::i18n::text("Cancel operation")).clicked(){if let Some(cancel)=&state.cancel{cancel.store(true,Ordering::Relaxed);}}}
        if !state.status.is_empty(){ui.label(crate::i18n::text(&state.status));}
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn capability_parser_requires_actual_still_capture() {
        let cameras=detected("Model                          Port\n----------------------------------------------------------\nTest Camera 1                  usb:001,002\nNetwork Camera                 ptpip:192.0.2.1\n");
        assert_eq!(cameras.len(), 2);
        assert_eq!(cameras[0].model, "Test Camera 1");
        assert!(can_capture("Capture choices                  :\n                                 : Image\nConfiguration support            : yes\n"));
        assert!(!can_capture(
            "Capture choices :\n : Preview\nConfiguration support : yes\n : Image\n"
        ));
        assert!(!can_capture(
            "Capture choices :\n : Capture not supported by the driver\n"
        ));
    }
    #[test]
    #[cfg(unix)]
    fn real_child_process_capture_retains_camera_files_and_publishes_complete_downloads() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let script = directory.path().join("fake gphoto2");
        std::fs::write(
            &script,
            r#"#!/bin/sh
for arg in "$@"; do
 case "$arg" in
 --version) printf 'gphoto2 fixture\n'; exit 0;;
 --auto-detect) printf 'Fixture Camera                 usb:001,002\n'; exit 0;;
 --abilities) printf 'Capture choices :\n : Image\n'; exit 0;;
 esac
done
keep=0
for arg in "$@"; do [ "$arg" = '--keep' ] && keep=1; done
[ "$keep" -eq 1 ] || exit 9
printf 'download-complete' > capture-1.jpg
"#,
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let cancel = AtomicBool::new(false);
        let Outcome::Detected(_, cameras) = detect(script.as_os_str(), &cancel).unwrap() else {
            panic!()
        };
        assert!(cameras[0].capture);
        let Outcome::Captured(paths, warnings) =
            capture(script.as_os_str(), &cameras[0], directory.path(), &cancel).unwrap()
        else {
            panic!()
        };
        assert!(warnings.is_empty());
        assert_eq!(paths.len(), 1);
        assert_eq!(std::fs::read(&paths[0]).unwrap(), b"download-complete");
        assert!(command(
            script.as_os_str(),
            &[],
            None,
            &AtomicBool::new(true),
            Duration::from_secs(1)
        )
        .is_err());
    }
}
