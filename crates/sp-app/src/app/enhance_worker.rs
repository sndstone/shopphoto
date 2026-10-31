//! Cancellable Python workers. Batch threads own their worker; the app's
//! serial interactive executor retains one between actions and releases it
//! after two minutes idle, on cancellation/failure, or on shutdown.
use std::{
    cell::RefCell,
    io::Write,
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

enum SourceIdentity {
    Raw(std::sync::Weak<sp_core::RawImage>),
    Raster(std::sync::Weak<sp_core::source::RasterImage>),
}
impl PartialEq for SourceIdentity {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Raw(a), Self::Raw(b)) => a.ptr_eq(b),
            (Self::Raster(a), Self::Raster(b)) => a.ptr_eq(b),
            _ => false,
        }
    }
}
/// Weak identities do not retain decoded RAW/RGB pixels and prevent reused
/// allocator addresses from masquerading as an unchanged source.
#[derive(PartialEq)]
pub(super) struct CanonicalKey {
    generation: u64,
    source: SourceIdentity,
    edit: sp_core::EditState,
    meta: serde_json::Value,
    profile: Option<sp_core::profile::CameraProfile>,
    look: Option<(String, sp_core::cube::CubeLut)>,
    lens: Option<(sp_core::lens::LensProfile, Option<f32>)>,
    ai_applied: bool,
}
impl CanonicalKey {
    #[allow(clippy::too_many_arguments)] // All resolved render dependencies form the cache key.
    pub fn new(
        generation: u64,
        source: &sp_core::source::ImageSource,
        edit: sp_core::EditState,
        meta: &sp_core::ImageMeta,
        profile: Option<sp_core::profile::CameraProfile>,
        look: Option<(String, sp_core::cube::CubeLut)>,
        lens: Option<(sp_core::lens::LensProfile, Option<f32>)>,
        ai_applied: bool,
    ) -> Result<Self, String> {
        Ok(Self {
            generation,
            source: match source {
                sp_core::source::ImageSource::Raw(raw) => SourceIdentity::Raw(Arc::downgrade(raw)),
                sp_core::source::ImageSource::Raster(rgb) => {
                    SourceIdentity::Raster(Arc::downgrade(rgb))
                }
            },
            edit,
            meta: serde_json::to_value(meta).map_err(|e| e.to_string())?,
            profile,
            look,
            lens,
            ai_applied,
        })
    }
}
pub(super) struct CanonicalInput {
    _directory: tempfile::TempDir,
    pub path: PathBuf,
    pub width: u32,
    pub height: u32,
}
thread_local! {
    static INPUT_CACHE: RefCell<Option<(CanonicalKey, Arc<CanonicalInput>)>> = const { RefCell::new(None) };
}
pub(super) fn clear_input_cache() {
    INPUT_CACHE.with(|cache| *cache.borrow_mut() = None);
    super::native_enhance::clear();
}
pub(super) fn canonical_input(
    key: CanonicalKey,
    cancel: &AtomicBool,
    render: impl FnOnce() -> Result<sp_pipeline::renderer::RenderedImage, String>,
) -> Result<Arc<CanonicalInput>, String> {
    let budget = std::env::var("SHOPPHOTO_MASK_INPUT_CACHE_MB")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(512)
        .clamp(64, 1024)
        * 1024
        * 1024;
    canonical_input_with_budget(key, cancel, budget, render)
}
fn canonical_input_with_budget(
    key: CanonicalKey,
    cancel: &AtomicBool,
    budget: u64,
    render: impl FnOnce() -> Result<sp_pipeline::renderer::RenderedImage, String>,
) -> Result<Arc<CanonicalInput>, String> {
    INPUT_CACHE.with(|slot| {
        let mut cache = slot.borrow_mut();
        if cancel.load(Ordering::Relaxed) {
            *cache = None;
            return Err("Enhancement cancelled".into());
        }
        if let Some((previous, input)) = &*cache {
            if previous == &key && input.path.is_file() {
                return Ok(input.clone());
            }
        }
        // Remove the previous file before rendering its replacement.
        *cache = None;
        let image = render()?;
        let bytes = u64::from(image.width) * u64::from(image.height) * 4;
        if image.width == 0 || image.height == 0 || bytes != image.rgba8.len() as u64 {
            return Err("Invalid canonical AI input".into());
        }
        if cancel.load(Ordering::Relaxed) {
            return Err("Enhancement cancelled".into());
        }
        let directory = tempfile::tempdir().map_err(|e| e.to_string())?;
        let path = directory.path().join("canonical.rgba8");
        std::fs::write(&path, &image.rgba8).map_err(|e| e.to_string())?;
        let input = Arc::new(CanonicalInput {
            _directory: directory,
            path,
            width: image.width,
            height: image.height,
        });
        if cancel.load(Ordering::Relaxed) {
            return Err("Enhancement cancelled".into());
        }
        if bytes <= budget {
            *cache = Some((key, input.clone()));
        }
        Ok(input)
    })
}

struct Task {
    cancel: Arc<AtomicBool>,
    run: Box<dyn FnOnce() + Send>,
}

/// Owns the interactive thread independently of individual mask render jobs.
/// Only a single pending action is allowed; GPU rendering remains off the UI.
pub(super) struct InteractiveWorker {
    sender: Option<crossbeam_channel::Sender<Task>>,
    stop: Arc<AtomicBool>,
    active: Arc<Mutex<Option<Arc<AtomicBool>>>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl InteractiveWorker {
    pub fn new() -> Result<Self, String> {
        Self::with_idle_timeout(Duration::from_secs(120))
    }

    fn with_idle_timeout(idle: Duration) -> Result<Self, String> {
        let (sender, receiver) = crossbeam_channel::bounded::<Task>(1);
        let stop = Arc::new(AtomicBool::new(false));
        let active = Arc::new(Mutex::new(None::<Arc<AtomicBool>>));
        let worker_stop = stop.clone();
        let worker_active = active.clone();
        let thread = std::thread::Builder::new()
            .name("sp-ai-enhancement".into())
            .spawn(move || {
                while !worker_stop.load(Ordering::Relaxed) {
                    match receiver.recv_timeout(idle) {
                        Ok(task) => {
                            let Ok(mut active) = worker_active.lock() else {
                                break;
                            };
                            // Publish cancellation before checking stop, covering a quit
                            // between dequeueing the action and starting its render.
                            *active = Some(task.cancel.clone());
                            if worker_stop.load(Ordering::Relaxed) {
                                task.cancel.store(true, Ordering::Relaxed);
                                break;
                            }
                            drop(active);
                            (task.run)();
                            if task.cancel.load(Ordering::Relaxed) {
                                clear_input_cache();
                            }
                            if let Ok(mut active) = worker_active.lock() {
                                *active = None;
                            }
                        }
                        Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                            WORKER.with(|slot| *slot.borrow_mut() = None);
                            clear_input_cache();
                        }
                        Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
                    }
                }
                WORKER.with(|slot| *slot.borrow_mut() = None);
                clear_input_cache();
            })
            .map_err(|error| format!("Could not start enhancement worker: {error}"))?;
        Ok(Self {
            sender: Some(sender),
            stop,
            active,
            thread: Some(thread),
        })
    }

    pub fn submit(
        &self,
        cancel: Arc<AtomicBool>,
        run: impl FnOnce() + Send + 'static,
    ) -> Result<(), String> {
        let Some(sender) = &self.sender else {
            return Err("Enhancement worker stopped".into());
        };
        sender
            .try_send(Task {
                cancel,
                run: Box::new(run),
            })
            .map_err(|error| match error {
                crossbeam_channel::TrySendError::Full(_) => "Enhancement worker is busy".into(),
                crossbeam_channel::TrySendError::Disconnected(_) => {
                    "Enhancement worker stopped".into()
                }
            })
    }

    pub fn is_stopped(&self) -> bool {
        self.thread
            .as_ref()
            .is_none_or(|thread| thread.is_finished())
    }
}

impl Drop for InteractiveWorker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Ok(active) = self.active.lock() {
            if let Some(cancel) = &*active {
                cancel.store(true, Ordering::Relaxed);
            }
        }
        self.sender.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

pub(super) fn python() -> PathBuf {
    std::env::var_os("SHOPPHOTO_ENHANCE_PYTHON")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let root = dirs::data_local_dir()
                .unwrap_or_else(std::env::temp_dir)
                .join("shopphoto/enhance-venv");
            root.join(if cfg!(windows) {
                "Scripts/python.exe"
            } else {
                "bin/python"
            })
        })
}

struct Worker {
    executable: PathBuf,
    child: Child,
    stdin: ChildStdin,
    directory: tempfile::TempDir,
}

impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Worker {
    fn start(executable: PathBuf) -> Result<Self, String> {
        if !executable.is_file() {
            return Err(
                "Enhancement runtime is missing. Open AI runtime to install or check it.".into(),
            );
        }
        let directory = tempfile::tempdir().map_err(|e| e.to_string())?;
        let script = directory.path().join("enhance.py");
        std::fs::write(&script, include_str!("../../../../scripts/enhance.py"))
            .map_err(|e| e.to_string())?;
        let log = std::fs::File::create(directory.path().join("worker.log"))
            .map_err(|e| e.to_string())?;
        let mut child = Command::new(&executable)
            .arg(script)
            .arg("--serve")
            .env("PYTORCH_ENABLE_MPS_FALLBACK", "1")
            .env("HF_HUB_OFFLINE", "1")
            .env("TRANSFORMERS_OFFLINE", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(log)
            .spawn()
            .map_err(|e| e.to_string())?;
        let stdin = child.stdin.take().expect("piped stdin");
        Ok(Self {
            executable,
            child,
            stdin,
            directory,
        })
    }

    fn run(
        &mut self,
        directory: &Path,
        mut request: serde_json::Value,
        cancel: &AtomicBool,
    ) -> Result<(), String> {
        let response = directory.join("response.json");
        if response.exists() {
            std::fs::remove_file(&response).map_err(|e| e.to_string())?;
        }
        request["_response"] = serde_json::json!(response);
        let mut line = serde_json::to_vec(&request).map_err(|e| e.to_string())?;
        line.push(b'\n');
        self.stdin
            .write_all(&line)
            .and_then(|_| self.stdin.flush())
            .map_err(|e| e.to_string())?;
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err("Enhancement cancelled".into());
            }
            if response.is_file() {
                let value: serde_json::Value =
                    serde_json::from_slice(&std::fs::read(&response).map_err(|e| e.to_string())?)
                        .map_err(|e| e.to_string())?;
                return if value["ok"] == true {
                    Ok(())
                } else {
                    Err(value["error"]
                        .as_str()
                        .unwrap_or("Enhancement failed")
                        .to_owned())
                };
            }
            if self.child.try_wait().map_err(|e| e.to_string())?.is_some() {
                let log = std::fs::read_to_string(self.directory.path().join("worker.log"))
                    .unwrap_or_default();
                let tail = log
                    .lines()
                    .rev()
                    .take(4)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect::<Vec<_>>()
                    .join("\n");
                return Err(if tail.is_empty() {
                    "Enhancement worker stopped unexpectedly".into()
                } else {
                    tail
                });
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
}

thread_local! { static WORKER: RefCell<Option<Worker>> = const { RefCell::new(None) }; }

/// Absence of a selection is a valid model result, so keep the image and
/// model caches warm for another class or an include/exclude prompt. Other
/// errors still invalidate the optional worker and native session state.
pub(super) fn is_no_match(error: &str) -> bool {
    error.starts_with("No confidently detected ")
        || error.starts_with("AI could not create a usable mask.")
        || error.starts_with("AI returned an empty mask")
}

pub(super) fn run(
    directory: &Path,
    request: serde_json::Value,
    cancel: &AtomicBool,
) -> Result<(), String> {
    if cancel.load(Ordering::Relaxed) {
        WORKER.with(|slot| *slot.borrow_mut() = None);
        clear_input_cache();
        return Err("Enhancement cancelled".into());
    }
    if let Some(result) = super::native_enhance::try_mask(&request, cancel) {
        return result;
    }
    WORKER.with(|slot| {
        let mut slot = slot.borrow_mut();
        let executable = python();
        if slot
            .as_ref()
            .is_some_and(|worker| worker.executable != executable)
        {
            *slot = None;
        }
        if slot.is_none() {
            *slot = Some(Worker::start(executable)?);
        }
        let result = slot
            .as_mut()
            .expect("initialized worker")
            .run(directory, request, cancel);
        // A cancelled, failed or broken worker never poisons the next batch item.
        if result.as_ref().is_err_and(|error| !is_no_match(error)) {
            *slot = None;
        }
        result
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn no_match_keeps_healthy_models_but_failures_do_not() {
        for error in [
            "No confidently detected car; use a point",
            "AI returned an empty mask; no empty layer was created",
            "AI could not create a usable mask. Try an include point",
        ] {
            assert!(is_no_match(error));
        }
        for error in [
            "Enhancement cancelled",
            "Native model checksum failed",
            "Core ML output axis differs from manifest",
            "invalid point",
        ] {
            assert!(!is_no_match(error));
        }
    }

    fn canonical_fixture() -> sp_core::source::ImageSource {
        sp_core::source::ImageSource::Raster(Arc::new(sp_core::source::RasterImage {
            width: 1,
            height: 1,
            linear_rec2020: vec![[0.1, 0.2, 0.3, 1.0]],
            bits_per_channel: 16,
            embedded_icc: None,
        }))
    }
    fn canonical_key(
        source: &sp_core::source::ImageSource,
        generation: u64,
        edit: sp_core::EditState,
    ) -> CanonicalKey {
        CanonicalKey::new(
            generation,
            source,
            edit,
            &Default::default(),
            None,
            None,
            None,
            false,
        )
        .unwrap()
    }
    fn rendered_fixture() -> Result<sp_pipeline::renderer::RenderedImage, String> {
        Ok(sp_pipeline::renderer::RenderedImage {
            width: 1,
            height: 1,
            rgba8: vec![1, 2, 3, 255],
            rgba16: None,
        })
    }
    #[test]
    fn canonical_input_reuses_files_and_invalidates_source_edit_and_generation() {
        clear_input_cache();
        let source = canonical_fixture();
        let cancel = AtomicBool::new(false);
        let first = canonical_input(
            canonical_key(&source, 1, Default::default()),
            &cancel,
            rendered_fixture,
        )
        .unwrap();
        let path = first.path.clone();
        drop(first);
        let reused = canonical_input(
            canonical_key(&source, 1, Default::default()),
            &cancel,
            || panic!("must reuse rendered input"),
        )
        .unwrap();
        assert_eq!(reused.path, path);
        drop(reused);
        let generation = canonical_input(
            canonical_key(&source, 2, Default::default()),
            &cancel,
            rendered_fixture,
        )
        .unwrap();
        assert_ne!(generation.path, path);
        assert!(!path.exists());
        drop(generation);
        let mut edit = sp_core::EditState::default();
        edit.tone.exposure_ev = 1.0;
        let changed =
            canonical_input(canonical_key(&source, 2, edit), &cancel, rendered_fixture).unwrap();
        let other = canonical_fixture();
        let replacement = canonical_input(
            canonical_key(&other, 2, Default::default()),
            &cancel,
            rendered_fixture,
        )
        .unwrap();
        assert_ne!(changed.path, replacement.path);
        if let sp_core::source::ImageSource::Raster(rgb) = &other {
            assert_eq!(Arc::strong_count(rgb), 1);
        }
        drop(changed);
        drop(replacement);
        clear_input_cache();
    }
    #[test]
    fn canonical_input_budget_failure_cancel_and_cleanup_release_files() {
        clear_input_cache();
        let source = canonical_fixture();
        let cancel = AtomicBool::new(false);
        let uncached = canonical_input_with_budget(
            canonical_key(&source, 1, Default::default()),
            &cancel,
            3,
            rendered_fixture,
        )
        .unwrap();
        let path = uncached.path.clone();
        drop(uncached);
        assert!(!path.exists());
        let cached = canonical_input(
            canonical_key(&source, 1, Default::default()),
            &cancel,
            rendered_fixture,
        )
        .unwrap();
        let path = cached.path.clone();
        drop(cached);
        assert!(canonical_input(
            canonical_key(&source, 2, Default::default()),
            &cancel,
            || Err("render failed".into())
        )
        .is_err());
        assert!(!path.exists());
        let cached = canonical_input(
            canonical_key(&source, 1, Default::default()),
            &cancel,
            rendered_fixture,
        )
        .unwrap();
        let path = cached.path.clone();
        drop(cached);
        cancel.store(true, Ordering::Relaxed);
        assert!(canonical_input(
            canonical_key(&source, 1, Default::default()),
            &cancel,
            || panic!("cancelled")
        )
        .is_err());
        assert!(!path.exists());
    }

    #[test]
    fn canonical_input_is_released_by_interactive_idle_and_shutdown() {
        let worker = InteractiveWorker::with_idle_timeout(Duration::from_millis(50)).unwrap();
        let (tx, rx) = crossbeam_channel::bounded(1);
        for _ in 0..2 {
            let tx = tx.clone();
            worker
                .submit(Arc::new(AtomicBool::new(false)), move || {
                    let source = canonical_fixture();
                    let input = canonical_input(
                        canonical_key(&source, 1, Default::default()),
                        &AtomicBool::new(false),
                        rendered_fixture,
                    )
                    .unwrap();
                    tx.send(input.path.clone()).unwrap();
                })
                .unwrap();
            let path = rx.recv_timeout(Duration::from_secs(5)).unwrap();
            let until = std::time::Instant::now() + Duration::from_secs(5);
            while path.exists() && std::time::Instant::now() < until {
                std::thread::sleep(Duration::from_millis(10));
            }
            assert!(!path.exists(), "idle must delete canonical input");
        }
        let tx2 = tx.clone();
        worker
            .submit(Arc::new(AtomicBool::new(false)), move || {
                let source = canonical_fixture();
                let input = canonical_input(
                    canonical_key(&source, 1, Default::default()),
                    &AtomicBool::new(false),
                    rendered_fixture,
                )
                .unwrap();
                tx2.send(input.path.clone()).unwrap();
            })
            .unwrap();
        let path = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        drop(worker);
        assert!(!path.exists(), "shutdown must delete canonical input");
    }

    #[test]
    fn canonical_key_tracks_metadata_profiles_and_denoised_pixels() {
        clear_input_cache();
        let source = canonical_fixture();
        let cancel = AtomicBool::new(false);
        let first = canonical_input(
            canonical_key(&source, 1, Default::default()),
            &cancel,
            rendered_fixture,
        )
        .unwrap();
        let mut key = canonical_key(&source, 1, Default::default());
        key.meta["iso"] = serde_json::json!(6400);
        let meta = canonical_input(key, &cancel, rendered_fixture).unwrap();
        assert_ne!(first.path, meta.path);
        let mut key = canonical_key(&source, 1, Default::default());
        key.profile = Some(sp_core::profile::CameraProfile::standard(
            "cache",
            sp_core::color::REC2020_TO_XYZ,
        ));
        let profile = canonical_input(key, &cancel, rendered_fixture).unwrap();
        assert_ne!(meta.path, profile.path);
        let mut key = canonical_key(&source, 1, Default::default());
        key.ai_applied = true;
        let denoised = canonical_input(key, &cancel, rendered_fixture).unwrap();
        assert_ne!(profile.path, denoised.path);
        drop(first);
        drop(meta);
        drop(profile);
        drop(denoised);
        clear_input_cache();
    }
    fn assert_reaped(pid: Option<u32>) {
        #[cfg(unix)]
        if let Some(pid) = pid {
            let status = Command::new("kill")
                .args(["-0", &pid.to_string()])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .unwrap();
            assert!(
                !status.success(),
                "worker process must be killed and reaped"
            );
        }
        #[cfg(not(unix))]
        let _ = pid;
    }

    #[test]
    fn separate_actions_use_the_same_thread() {
        let worker = InteractiveWorker::new().unwrap();
        let (tx, rx) = crossbeam_channel::bounded(1);
        let mut previous = None;
        for _ in 0..2 {
            let tx = tx.clone();
            worker
                .submit(Arc::new(AtomicBool::new(false)), move || {
                    tx.send(std::thread::current().id()).unwrap();
                })
                .unwrap();
            let id = rx.recv_timeout(Duration::from_secs(5)).unwrap();
            assert_ne!(id, std::thread::current().id());
            if let Some(previous) = previous {
                assert_eq!(id, previous);
            }
            previous = Some(id);
        }
    }

    #[test]
    fn shutdown_cancels_active_action_and_discards_queued_action() {
        let worker = InteractiveWorker::new().unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let job_cancel = cancel.clone();
        let (started_tx, started_rx) = crossbeam_channel::bounded(1);
        worker
            .submit(cancel.clone(), move || {
                started_tx.send(()).unwrap();
                while !job_cancel.load(Ordering::Relaxed) {
                    std::thread::yield_now();
                }
            })
            .unwrap();
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let (queued_tx, queued_rx) = crossbeam_channel::bounded(1);
        worker
            .submit(Arc::new(AtomicBool::new(false)), move || {
                queued_tx.send(()).unwrap();
            })
            .unwrap();
        assert!(worker
            .submit(Arc::new(AtomicBool::new(false)), || {})
            .is_err());
        drop(worker);
        assert!(cancel.load(Ordering::Relaxed));
        assert!(queued_rx.recv().is_err());
    }

    #[test]
    #[ignore = "requires SHOPPHOTO_ENHANCE_PYTHON and the optional Python packages"]
    fn real_interactive_process_reuse_failure_cancellation_idle_and_shutdown() {
        let worker = InteractiveWorker::with_idle_timeout(Duration::from_millis(250)).unwrap();
        let source = tempfile::tempdir().unwrap();
        let input = source.path().join("input.png");
        image::GrayImage::from_pixel(16, 16, image::Luma([0]))
            .save(&input)
            .unwrap();
        let (tx, rx) = crossbeam_channel::bounded(1);
        let submit = |operation: &str, cancel: Arc<AtomicBool>| {
            let input = input.clone();
            let operation = operation.to_owned();
            let job_cancel = cancel.clone();
            let tx = tx.clone();
            worker
                .submit(cancel, move || {
                    let directory = tempfile::tempdir().unwrap();
                    let output = directory.path().join("faces.json");
                    let result = run(
                        directory.path(),
                        serde_json::json!({"operation":operation,"input":input,"output":output}),
                        &job_cancel,
                    );
                    let pid =
                        WORKER.with(|slot| slot.borrow().as_ref().map(|worker| worker.child.id()));
                    tx.send((result, pid)).unwrap();
                })
                .unwrap();
            rx.recv_timeout(Duration::from_secs(30)).unwrap()
        };
        let (result, first) = submit("analyze_faces", Arc::new(AtomicBool::new(false)));
        result.unwrap();
        assert!(first.is_some());
        let (result, second) = submit("analyze_faces", Arc::new(AtomicBool::new(false)));
        result.unwrap();
        assert_eq!(first, second);
        let (result, failed) = submit("invalid", Arc::new(AtomicBool::new(false)));
        assert!(result.is_err());
        assert!(failed.is_none());
        assert_reaped(first);
        let (result, replacement) = submit("analyze_faces", Arc::new(AtomicBool::new(false)));
        result.unwrap();
        assert_ne!(replacement, first);
        // A cancellation before work is sent must not start a replacement process.
        let (result, _) = submit("analyze_faces", Arc::new(AtomicBool::new(true)));
        assert!(result.is_err());
        std::thread::sleep(Duration::from_millis(500));
        let (result, after_idle) = submit("analyze_faces", Arc::new(AtomicBool::new(false)));
        result.unwrap();
        assert_ne!(after_idle, replacement);
        assert_reaped(replacement);
        // Cancellation must also release an allocated process when it reaches
        // the request gate before dispatch, rather than retaining idle weights.
        let (pid_tx, pid_rx) = crossbeam_channel::bounded(1);
        let (result_tx, result_rx) = crossbeam_channel::bounded(1);
        let cancel = Arc::new(AtomicBool::new(false));
        let job_cancel = cancel.clone();
        worker
            .submit(cancel.clone(), move || {
                WORKER.with(|slot| *slot.borrow_mut() = Some(Worker::start(python()).unwrap()));
                let pid = WORKER.with(|slot| slot.borrow().as_ref().unwrap().child.id());
                pid_tx.send(pid).unwrap();
                let directory = tempfile::tempdir().unwrap();
                let output = directory.path().join("faces.json");
                let result = run(
                    directory.path(),
                    serde_json::json!({"operation":"analyze_faces","input":input,"output":output}),
                    &job_cancel,
                );
                let remaining = WORKER.with(|slot| slot.borrow().is_some());
                result_tx.send((result, remaining)).unwrap();
            })
            .unwrap();
        let cancelled_pid = pid_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        cancel.store(true, Ordering::Relaxed);
        let (result, remaining) = result_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(result.is_err());
        assert!(!remaining);
        assert_reaped(Some(cancelled_pid));
        drop(worker);
        assert_reaped(after_idle);
    }

    #[test]
    #[ignore = "requires optional Python, SlimSAM and ViTMatte models"]
    fn real_interactive_masks_share_process_and_pixels_across_input_transports() {
        let worker = InteractiveWorker::new().unwrap();
        let directory = tempfile::tempdir().unwrap();
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/advanced-ai-features/ai-mask-scene.png");
        let pixels = image::open(fixture).unwrap().thumbnail(512, 512).to_rgba8();
        let input_png = directory.path().join("input.png");
        let input_raw = directory.path().join("input.rgba8");
        pixels.save(&input_png).unwrap();
        std::fs::write(&input_raw, pixels.as_raw()).unwrap();
        let mut previous = None;
        let (tx, rx) = crossbeam_channel::bounded(1);
        for input in [input_png, input_raw] {
            let tx = tx.clone();
            let dimensions = pixels.dimensions();
            worker.submit(Arc::new(AtomicBool::new(false)), move || {
                let directory = tempfile::tempdir().unwrap();
                let output = directory.path().join("mask.png");
                let mut request = serde_json::json!({"operation":"mask","kind":"object","input":input,"output":output,"points":[[0.5,0.5]]});
                if input.extension().is_some_and(|extension| extension == "rgba8") {
                    request["input_format"] = serde_json::json!("rgba8");
                    request["width"] = serde_json::json!(dimensions.0);
                    request["height"] = serde_json::json!(dimensions.1);
                }
                run(directory.path(), request, &AtomicBool::new(false)).unwrap();
                let mask = image::open(output).unwrap().to_luma16();
                assert_eq!(mask.dimensions(), dimensions);
                let pid = WORKER.with(|slot| slot.borrow().as_ref().unwrap().child.id());
                tx.send((pid, mask.into_raw())).unwrap();
            }).unwrap();
            let result = rx.recv_timeout(Duration::from_secs(60)).unwrap();
            if let Some(previous) = &previous {
                assert_eq!(
                    &result, previous,
                    "same process and identical 16-bit alpha across PNG/RGBA input"
                );
            }
            previous = Some(result);
        }
        let pid = previous.unwrap().0;
        drop(worker);
        assert_reaped(Some(pid));
    }

    #[test]
    #[ignore = "requires optional Python, SlimSAM and ViTMatte models"]
    fn real_interactive_canonical_masks_reuse_render_and_invalidate_edits() {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/advanced-ai-features/ai-mask-scene.png");
        let decoded = sp_raw::decode_source(&fixture).unwrap();
        let source = decoded.source;
        let meta = decoded.meta;
        let gpu = sp_gpu::GpuContext::headless().expect("real GPU required");
        let renderer = sp_pipeline::GpuRenderer::new(gpu.clone()).unwrap();
        let pipes = renderer.shared_pipelines();
        drop(renderer);
        let renders = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let worker = InteractiveWorker::new().unwrap();
        let (tx, rx) = crossbeam_channel::bounded(1);
        let mut first: Option<(u32, PathBuf, Vec<u16>, f64)> = None;
        let mut last_pid = None;
        for exposure in [0.0, 0.0, 0.5] {
            let (source, meta, gpu, pipes, tx, renders) = (
                source.clone(),
                meta.clone(),
                gpu.clone(),
                pipes.clone(),
                tx.clone(),
                renders.clone(),
            );
            worker.submit(Arc::new(AtomicBool::new(false)), move || {
                let mut edit = sp_core::EditState::default(); edit.tone.exposure_ev = exposure;
                let key = canonical_key(&source, 1, edit.clone());
                let started = std::time::Instant::now();
                let input = canonical_input(key, &AtomicBool::new(false), || {
                    renders.fetch_add(1, Ordering::Relaxed);
                    let mut renderer = sp_pipeline::GpuRenderer::with_pipelines(gpu, pipes).unwrap();
                    renderer.set_canonical_mask_input(true);
                    renderer.set_capture_metadata(Some(&meta));
                    renderer.open_source_with_edit(&source, &edit).unwrap();
                    renderer.render_export(&edit, false, false, sp_export::ExportColorSpace::Srgb,
                        &AtomicBool::new(false), |_| {}).map_err(|e| e.to_string())
                }).unwrap();
                let input_seconds = started.elapsed().as_secs_f64();
                drop(source);
                let directory = tempfile::tempdir().unwrap();
                let output = directory.path().join("mask.png");
                run(directory.path(), serde_json::json!({"operation":"mask","kind":"object",
                    "input":input.path,"input_format":"rgba8","width":input.width,"height":input.height,
                    "output":output,"points":[[0.5,0.5]]}), &AtomicBool::new(false)).unwrap();
                let pixels = image::open(output).unwrap().to_luma16().into_raw();
                let pid = WORKER.with(|slot| slot.borrow().as_ref().unwrap().child.id());
                tx.send((pid, input.path.clone(), pixels, input_seconds)).unwrap();
            }).unwrap();
            let result = rx.recv_timeout(Duration::from_secs(120)).unwrap();
            last_pid = Some(result.0);
            if let Some(first) = &first {
                assert_eq!(result.0, first.0, "one retained Python process");
                if exposure == 0.0 {
                    assert_eq!(result.1, first.1, "same canonical file");
                    assert_eq!(result.2, first.2, "identical complete 16-bit mask");
                    eprintln!(
                        "Canonical render/staging: fresh {:.6}s, reused {:.6}s",
                        first.3, result.3
                    );
                } else {
                    assert_ne!(result.1, first.1, "changed pixels need new input");
                }
            } else {
                first = Some(result);
            }
        }
        assert_eq!(renders.load(Ordering::Relaxed), 2);
        let original_path = first.unwrap().1;
        drop(worker);
        assert_reaped(last_pid);
        assert!(!original_path.exists());
    }
}
