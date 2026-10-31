//! Background decode pool (BUILD.md §3.3): high-priority opens and
//! generation-cancellable thumbnail work on half the available CPU cores,
//! plus a single persist worker for every disk write the UI used to do in
//! the frame (sidecars, XMP, catalog rows, thumbnail cache).
//!
//! The UI thread sends [`JobRequest`]s and drains [`JobResult`]s once per
//! frame in `App::logic`. Nothing here touches egui.

use crossbeam_channel::{unbounded, Receiver, Sender};
use sp_raw::SourceOutput as DecodeOutput;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub enum JobRequest {
    Decode {
        path: PathBuf,
        generation: u64,
    },
    Prefetch {
        path: PathBuf,
        generation: u64,
    },
    /// Decode for a side-by-side culling cell; does not replace the open
    /// image.
    DecodeCompare(PathBuf),
    AiDenoise {
        path: PathBuf,
        raw: Arc<sp_core::RawImage>,
        meta: sp_core::ImageMeta,
        mode: sp_core::edit::CameraDenoiseMode,
        compute: sp_ai::ComputeUnits,
        /// The model's unblended output for `raw` from an earlier run, when
        /// only the blend mode changed. Skips the 13–26 s inference.
        inferred: Option<Arc<sp_core::RawImage>>,
        generation: u64,
    },
    Thumbnail {
        path: PathBuf,
        generation: u64,
    },
}

pub enum JobResult {
    PreviewReady {
        path: PathBuf,
        preview: Arc<sp_raw::Preview>,
        generation: u64,
    },
    Annotated {
        path: PathBuf,
        annotations: sp_catalog::Annotations,
        color_label: sp_catalog::ColorLabel,
    },
    Decoded {
        generation: u64,
        path: PathBuf,
        decode_path: PathBuf,
        network_cached: bool,
        offline: bool,
        pinned: bool,
        xmp_revision: Option<sp_catalog::xmp::XmpRevision>,
        output: Box<DecodeOutput>,
    },
    DecodeFailed {
        generation: u64,
        path: PathBuf,
        error: String,
    },
    CompareDecoded {
        path: PathBuf,
        decode_path: PathBuf,
        output: Box<DecodeOutput>,
    },
    CompareDecodeFailed {
        path: PathBuf,
        error: String,
    },
    AiDenoised {
        path: PathBuf,
        /// Blended result for `mode`.
        raw: Arc<sp_core::RawImage>,
        /// Unblended model output, kept by the app for later mode changes.
        inferred: Arc<sp_core::RawImage>,
        mode: sp_core::edit::CameraDenoiseMode,
        generation: u64,
        elapsed: std::time::Duration,
        /// Whether this result ran the model or only re-blended a cached
        /// inference.
        inferred_now: bool,
    },
    AiDenoiseFailed {
        path: PathBuf,
        mode: sp_core::edit::CameraDenoiseMode,
        generation: u64,
        error: String,
    },
    ThumbnailReady {
        path: PathBuf,
        preview: sp_raw::Preview,
        /// Cached PNGs contain pixels only. Fresh RAW decodes also carry
        /// metadata and may update the catalog.
        meta: Option<sp_core::ImageMeta>,
        generation: u64,
    },
    /// The file has no decodable preview (unsupported codec, corrupt file).
    /// The grid stops showing a loading shimmer and explains why instead.
    ThumbnailFailed {
        path: PathBuf,
        error: String,
        /// The file is gone from disk (moved or deleted outside ShopPhoto).
        missing: bool,
        generation: u64,
    },
    SidecarSyncFailed {
        path: PathBuf,
        error: String,
    },
    AutoFinishSaved {
        path: PathBuf,
        before: Box<sp_catalog::Sidecar>,
        after: Box<sp_catalog::Sidecar>,
        preview: sp_raw::Preview,
        error: Option<String>,
    },
    /// A write queued through [`JobSystem::persist`] failed. `retry` is set
    /// when the `.spedit` itself did not land, so the app should mark the
    /// edit dirty again; XMP and catalog failures are informational.
    PersistFailed {
        path: PathBuf,
        error: String,
        retry: bool,
    },
}

/// One debounced sidecar save, with everything the persist worker needs to
/// finish the job without touching app state.
pub struct SidecarWrite {
    pub sidecar: sp_catalog::Sidecar,
    /// The photo as the user knows it (the network original when cached).
    pub path: PathBuf,
    /// Where the `.spedit` lives: the local cache path for network sources,
    /// otherwise the same as `path`.
    pub edit_source: PathBuf,
    pub virtual_copy: Option<String>,
    pub write_xmp: bool,
    pub xmp_annotations_only: bool,
    pub xmp_revision: Option<Arc<std::sync::Mutex<sp_catalog::xmp::XmpRevision>>>,
    pub annotations: sp_catalog::Annotations,
    pub metadata: sp_catalog::DescriptiveMetadata,
    pub catalog: Option<PathBuf>,
}

pub enum PersistRequest {
    AutoFinish {
        source: PathBuf,
        edit_source: PathBuf,
        before: Box<sp_catalog::Sidecar>,
        after: Box<sp_catalog::Sidecar>,
        preview: sp_raw::Preview,
        thumbnail_cache: Option<PathBuf>,
    },
    Sidecar(Box<SidecarWrite>),
    CheckedSidecar {
        before: Option<Box<sp_catalog::Sidecar>>,
        write: Box<SidecarWrite>,
    },
    CatalogMetadata {
        catalog: PathBuf,
        path: PathBuf,
        meta: sp_core::ImageMeta,
    },
    /// Replied to once every request queued before it has been processed.
    Flush(Sender<()>),
    /// A barrier that also verifies the latest save of this exact edit version.
    ConfirmSidecar {
        sidecar_path: PathBuf,
        reply: Sender<Result<(), String>>,
    },
}

pub struct JobSystem {
    decode_queue: Arc<crate::work_queue::WorkQueue<JobRequest>>,
    open_generation: Arc<std::sync::atomic::AtomicU64>,
    previews: Arc<std::sync::Mutex<crate::preview_cache::PreviewCache>>,
    budget: crate::work_queue::MemoryBudget,
    ai_requests: Sender<JobRequest>,
    persist_requests: Sender<PersistRequest>,
    results: Receiver<JobResult>,
    thumbnail_generation: std::sync::Arc<std::sync::atomic::AtomicU64>,
    ai_generation: std::sync::Arc<std::sync::atomic::AtomicU64>,
}

impl JobSystem {
    pub fn start(source_cache_dir: Option<PathBuf>, thumbnail_cache_dir: Option<PathBuf>) -> Self {
        let decode_queue = Arc::new(crate::work_queue::WorkQueue::default());
        let open_generation = Arc::new(std::sync::atomic::AtomicU64::new(1));
        let budget = crate::work_queue::MemoryBudget::configured();
        let memory = crate::work_queue::MemoryGate::new(budget.decode_bytes);
        let previews = Arc::new(std::sync::Mutex::new(
            crate::preview_cache::PreviewCache::new(budget.preview_bytes),
        ));
        let (ai_tx, ai_rx) = unbounded::<JobRequest>();
        let (sidecar_sync_tx, sidecar_sync_rx) = unbounded::<(PathBuf, PathBuf)>();
        let (persist_tx, persist_rx) = unbounded::<PersistRequest>();
        let (res_tx, res_rx) = unbounded::<JobResult>();
        let thumbnail_generation = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(1));
        let ai_generation = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(1));
        let workers =
            std::thread::available_parallelism().map_or(2, |value| (value.get() / 2).clamp(2, 8));
        for index in 0..workers {
            let queue = decode_queue.clone();
            let open_generation = open_generation.clone();
            let previews = previews.clone();
            let memory = memory.clone();
            let results = res_tx.clone();
            let generation = thumbnail_generation.clone();
            let source_cache = source_cache_dir.clone();
            let thumbnail_cache = thumbnail_cache_dir.clone();
            std::thread::Builder::new()
                .name(format!("sp-decode-{index}"))
                .spawn(move || {
                    decode_worker(
                        DecodeScheduler {
                            queue: &queue,
                            open_generation: &open_generation,
                            previews: &previews,
                            memory: &memory,
                            foreground_only: index == 0,
                        },
                        &results,
                        &generation,
                        (source_cache.as_deref(), thumbnail_cache.as_deref()),
                    )
                })
                .expect("spawn decode worker");
        }
        let ai_results = res_tx.clone();
        let current_ai_generation = ai_generation.clone();
        std::thread::Builder::new()
            .name("sp-ai-denoise".into())
            .spawn(move || {
                for request in ai_rx {
                    run_ai_request(request, &ai_results, &current_ai_generation);
                }
            })
            .expect("spawn AI denoise worker");
        let sync_results = res_tx.clone();
        std::thread::Builder::new()
            .name("sp-network-sidecar-sync".into())
            .spawn(move || {
                for (source, cached) in sidecar_sync_rx {
                    if let Err(error) = sp_catalog::source_cache::sync_sidecar(&cached, &source) {
                        let _ = sync_results.send(JobResult::SidecarSyncFailed {
                            path: source,
                            error: error.to_string(),
                        });
                    }
                }
            })
            .expect("spawn network sidecar sync worker");
        let persist_results = res_tx.clone();
        let persist_sync = sidecar_sync_tx;
        let network_cache_enabled = source_cache_dir.is_some();
        std::thread::Builder::new()
            .name("sp-persist".into())
            .spawn(move || {
                persist_worker(
                    &persist_rx,
                    &persist_results,
                    &persist_sync,
                    network_cache_enabled,
                );
            })
            .expect("spawn persist worker");
        if let Some(cache_dir) = thumbnail_cache_dir {
            // Bound the cache once per launch even if this session never
            // writes enough to trigger the incremental walk.
            let _ = std::thread::Builder::new()
                .name("sp-thumb-budget".into())
                .spawn(move || {
                    let _ = sp_catalog::thumbnails::enforce_budget(
                        &cache_dir,
                        sp_catalog::thumbnails::THUMB_CACHE_BUDGET_BYTES,
                    );
                });
        }

        Self {
            decode_queue,
            open_generation,
            previews,
            budget,
            ai_requests: ai_tx,
            persist_requests: persist_tx,
            results: res_rx,
            thumbnail_generation,
            ai_generation,
        }
    }

    pub fn request_decode(&self, path: PathBuf) {
        let generation = self
            .open_generation
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            + 1;
        self.decode_queue
            .open(JobRequest::Decode { path, generation });
        self.decode_queue.neighbors([]);
    }

    pub fn request_compare_decode(&self, path: PathBuf) {
        self.decode_queue.compare(JobRequest::DecodeCompare(path));
    }

    pub fn prefetch_neighbors(&self, paths: impl IntoIterator<Item = PathBuf>) {
        let generation = self
            .open_generation
            .load(std::sync::atomic::Ordering::Relaxed);
        self.decode_queue.neighbors(
            paths
                .into_iter()
                .map(|path| JobRequest::Prefetch { path, generation }),
        );
    }

    pub fn cached_preview(&self, path: &Path) -> Option<Arc<sp_raw::Preview>> {
        self.previews.lock().ok()?.get(path)
    }

    pub fn preview_is_current(&self, generation: u64) -> bool {
        generation
            == self
                .open_generation
                .load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn thumbnail_budget(&self) -> usize {
        self.budget.thumbnail_bytes
    }

    pub fn preview_cache_bytes(&self) -> usize {
        self.previews.lock().map_or(0, |cache| cache.bytes())
    }

    pub fn request_ai_denoise(
        &self,
        path: PathBuf,
        raw: Arc<sp_core::RawImage>,
        meta: sp_core::ImageMeta,
        mode: sp_core::edit::CameraDenoiseMode,
        compute: sp_ai::ComputeUnits,
        inferred: Option<Arc<sp_core::RawImage>>,
    ) -> u64 {
        let generation = self
            .ai_generation
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            + 1;
        let _ = self.ai_requests.send(JobRequest::AiDenoise {
            path,
            raw,
            meta,
            mode,
            compute,
            inferred,
            generation,
        });
        generation
    }

    pub fn cancel_ai_denoise(&self) {
        self.ai_generation
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    /// Cancel queued/scrolled-away thumbnails and return a fresh generation.
    pub fn begin_thumbnail_generation(&self) -> u64 {
        self.decode_queue.clear_visible();
        self.thumbnail_generation
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            + 1
    }

    pub fn request_thumbnail(&self, path: PathBuf, generation: u64) {
        let key = path.clone();
        self.decode_queue.visible(
            JobRequest::Thumbnail { path, generation },
            |job| matches!(job, JobRequest::Thumbnail { path, .. } if *path == key),
        );
    }

    /// Queue a disk write on the persist worker. Requests for the same path
    /// are applied in the order they were queued.
    pub fn persist(&self, request: PersistRequest) {
        if self.persist_requests.send(request).is_err() {
            tracing::error!("persist worker is gone; write dropped");
        }
    }

    /// Wait until every write queued so far has been applied. Used before
    /// reading a sidecar back from disk and on exit. Returns `false` on
    /// timeout, in which case the write may still land later.
    pub fn flush_persist(&self, timeout: std::time::Duration) -> bool {
        let (reply_tx, reply_rx) = crossbeam_channel::bounded(1);
        if self
            .persist_requests
            .send(PersistRequest::Flush(reply_tx))
            .is_err()
        {
            return false;
        }
        reply_rx.recv_timeout(timeout).is_ok()
    }

    /// Wait for prior writes and require the latest local save of this
    /// document to have succeeded. Remote mirrors and optional XMP do not
    /// prevent switching once the local edit is safe.
    pub fn confirm_sidecar(
        &self,
        edit_source: &Path,
        virtual_copy: Option<&str>,
        timeout: std::time::Duration,
    ) -> Result<(), String> {
        let sidecar_path = virtual_copy.map_or_else(
            || sp_catalog::sidecar_path(edit_source),
            |id| sp_catalog::virtual_copy_sidecar_path(edit_source, id),
        );
        let (reply_tx, reply_rx) = crossbeam_channel::bounded(1);
        self.persist_requests
            .send(PersistRequest::ConfirmSidecar {
                sidecar_path,
                reply: reply_tx,
            })
            .map_err(|_| "The edit save worker stopped; your edits remain open".to_owned())?;
        reply_rx
            .recv_timeout(timeout)
            .map_err(|_| "Edits have not finished saving; try again shortly".to_owned())?
    }

    /// Non-blocking drain, called once per frame.
    pub fn poll(&self) -> Vec<JobResult> {
        self.results.try_iter().collect()
    }
}

// Disk-size estimate includes decoder intermediates. Unknown files reserve a RAW-sized slot.
fn decode_memory_estimate(path: &Path) -> usize {
    std::fs::metadata(path)
        .ok()
        .map(|metadata| {
            let factor = if sp_raw::is_supported(path) { 8 } else { 64 };
            (metadata.len().saturating_mul(factor).min(usize::MAX as u64) as usize)
                .max(128 * crate::work_queue::MIB)
        })
        .unwrap_or(384 * crate::work_queue::MIB)
}

struct DecodeScheduler<'a> {
    queue: &'a crate::work_queue::WorkQueue<JobRequest>,
    open_generation: &'a std::sync::atomic::AtomicU64,
    previews: &'a std::sync::Mutex<crate::preview_cache::PreviewCache>,
    memory: &'a Arc<crate::work_queue::MemoryGate>,
    foreground_only: bool,
}

fn decode_worker(
    scheduler: DecodeScheduler<'_>,
    results: &Sender<JobResult>,
    current_generation: &std::sync::atomic::AtomicU64,
    cache_dirs: (Option<&Path>, Option<&Path>),
) {
    let DecodeScheduler {
        queue,
        open_generation,
        previews,
        memory,
        foreground_only,
    } = scheduler;
    let (source_cache_dir, thumbnail_cache_dir) = cache_dirs;
    loop {
        let Some(req) = (if foreground_only {
            queue.next_foreground()
        } else {
            queue.next()
        }) else {
            return;
        };
        match req {
            JobRequest::DecodeCompare(path) => {
                let _lease = memory.acquire(decode_memory_estimate(&path));
                let prepared = source_cache_dir
                    .and_then(|cache| sp_catalog::source_cache::prepare(cache, &path).ok())
                    .map_or_else(|| path.clone(), |prepared| prepared.decode_path);
                let msg = match sp_raw::decode_source(&prepared) {
                    Ok(output) => JobResult::CompareDecoded {
                        path,
                        decode_path: prepared,
                        output: Box::new(output),
                    },
                    Err(e) => JobResult::CompareDecodeFailed {
                        path,
                        error: e.to_string(),
                    },
                };
                if results.send(msg).is_err() {
                    return;
                }
            }
            JobRequest::Prefetch { path, generation } => {
                if generation != open_generation.load(std::sync::atomic::Ordering::Relaxed) {
                    continue;
                }
                // Neighbors retain only preview pixels, never decoded mosaics or network originals.
                let _lease = memory.acquire_background(64 * crate::work_queue::MIB);
                if generation != open_generation.load(std::sync::atomic::Ordering::Relaxed) {
                    continue;
                }
                if !sp_raw::is_supported(&path) {
                    continue;
                }
                if let Ok(output) = sp_raw::decode_preview(&path) {
                    if generation == open_generation.load(std::sync::atomic::Ordering::Relaxed) {
                        if let Some(preview) = output
                            .preview
                            .and_then(crate::preview_cache::bounded_preview)
                        {
                            if let Ok(mut cache) = previews.lock() {
                                cache.insert(path, preview);
                            }
                        }
                    }
                }
            }
            JobRequest::Decode { path, generation } => {
                if generation != open_generation.load(std::sync::atomic::Ordering::Relaxed) {
                    continue;
                }
                let _lease = memory.acquire(decode_memory_estimate(&path));
                if generation != open_generation.load(std::sync::atomic::Ordering::Relaxed) {
                    continue;
                }
                if sp_raw::is_supported(&path) {
                    if let Ok(output) = sp_raw::decode_preview(&path) {
                        if let Some(preview) = output
                            .preview
                            .and_then(crate::preview_cache::bounded_preview)
                        {
                            if let Ok(mut cache) = previews.lock() {
                                cache.insert(path.clone(), preview);
                            }
                            if let Some(preview) =
                                previews.lock().ok().and_then(|mut cache| cache.get(&path))
                            {
                                let _ = results.send(JobResult::PreviewReady {
                                    path: path.clone(),
                                    preview,
                                    generation,
                                });
                            }
                        }
                    }
                }
                if generation != open_generation.load(std::sync::atomic::Ordering::Relaxed) {
                    continue;
                }
                let started = std::time::Instant::now();
                let prepared = source_cache_dir
                    .map(|cache| sp_catalog::source_cache::prepare(cache, &path))
                    .transpose();
                let msg = match prepared.and_then(|prepared| {
                    let prepared =
                        prepared.unwrap_or_else(|| sp_catalog::source_cache::PreparedSource {
                            decode_path: path.clone(),
                            cached: false,
                            offline: false,
                        });
                    sp_raw::decode_source(&prepared.decode_path).map(|output| (prepared, output))
                }) {
                    Ok((prepared, output)) => {
                        tracing::info!(
                            path = %path.display(),
                            elapsed_ms = started.elapsed().as_millis() as u64,
                            mp = { let (w, h) = output.source.dimensions(); w as f32 * h as f32 / 1e6 },
                            "decoded"
                        );
                        JobResult::Decoded {
                            generation,
                            offline: prepared.offline,
                            pinned: source_cache_dir.is_some_and(|cache| {
                                sp_catalog::source_cache::is_pinned(cache, &path)
                            }),
                            xmp_revision: sp_catalog::xmp::revision(&path).ok(),
                            path,
                            decode_path: prepared.decode_path,
                            network_cached: prepared.cached,
                            output: Box::new(output),
                        }
                    }
                    Err(e) => JobResult::DecodeFailed {
                        generation,
                        path,
                        error: e.to_string(),
                    },
                };
                if generation != open_generation.load(std::sync::atomic::Ordering::Relaxed) {
                    continue;
                }
                if results.send(msg).is_err() {
                    return; // UI gone; shut down quietly.
                }
            }
            JobRequest::AiDenoise { .. } => unreachable!("AI jobs use their dedicated worker"),
            JobRequest::Thumbnail { path, generation } => {
                let _lease = memory.acquire_background(if sp_raw::is_supported(&path) {
                    64 * crate::work_queue::MIB
                } else {
                    decode_memory_estimate(&path)
                });
                if generation != current_generation.load(std::sync::atomic::Ordering::Relaxed) {
                    continue;
                }
                if let Some(preview) = thumbnail_cache_dir
                    .and_then(|cache_dir| load_cached_thumbnail(cache_dir, &path))
                {
                    let _ = results.send(JobResult::ThumbnailReady {
                        path,
                        preview,
                        meta: None,
                        generation,
                    });
                    continue;
                }
                // Thumbnail extraction must not call source_cache::prepare:
                // that copies the complete RAW before showing one SMB image.
                // The lightweight path lazily reads just the embedded preview.
                // Full local caching remains reserved for opening/developing.
                let decoded = sp_raw::decode_source_preview(&path)
                    .map(|output| (output.preview, output.meta))
                    .or_else(|_| {
                        drop(_lease);
                        let _full_lease = memory.acquire_background(decode_memory_estimate(&path));
                        if generation
                            != current_generation.load(std::sync::atomic::Ordering::Relaxed)
                        {
                            return Err(sp_core::SpError::decode(
                                path.display().to_string(),
                                "Thumbnail superseded",
                            ));
                        }
                        // A minority of RAW formats have no embedded image.
                        // Preserve their old full-decode fallback, including
                        // a local network copy when one is actually required.
                        let decode_path = source_cache_dir
                            .and_then(|cache| sp_catalog::source_cache::prepare(cache, &path).ok())
                            .map_or_else(|| path.clone(), |prepared| prepared.decode_path);
                        sp_raw::decode_source(&decode_path)
                            .map(|output| (output.preview, output.meta))
                    });
                if generation != current_generation.load(std::sync::atomic::Ordering::Relaxed) {
                    continue;
                }
                match decoded {
                    Ok((Some(preview), meta)) => {
                        if let Some(cache_dir) = thumbnail_cache_dir {
                            cache_thumbnail(cache_dir, &path, &preview, false);
                        }
                        let _ = results.send(JobResult::ThumbnailReady {
                            path,
                            preview,
                            meta: Some(meta),
                            generation,
                        });
                    }
                    Ok((None, _)) => {
                        let _ = results.send(JobResult::ThumbnailFailed {
                            path,
                            error: "The file has no preview image".into(),
                            missing: false,
                            generation,
                        });
                    }
                    Err(error) => {
                        // Stat here on the worker, never in the frame.
                        let missing = matches!(
                            std::fs::metadata(&path),
                            Err(e) if e.kind() == std::io::ErrorKind::NotFound
                        );
                        let _ = results.send(JobResult::ThumbnailFailed {
                            path,
                            error: error.to_string(),
                            missing,
                            generation,
                        });
                    }
                }
            }
        }
    }
}

/// Cache lookup, file stat, PNG read and decode all stay on the thumbnail
/// worker. In particular, `cache_key` may need to stat an SMB source, which
/// must never happen inside an egui frame.
fn load_cached_thumbnail(cache_dir: &Path, source: &Path) -> Option<sp_raw::Preview> {
    let key = sp_catalog::thumbnails::cache_key(cache_dir, source)?;
    let bytes = sp_catalog::thumbnails::load(&key).ok()?;
    let rgba = image::load_from_memory(&bytes).ok()?.into_rgba8();
    let (width, height) = rgba.dimensions();
    Some(sp_raw::Preview {
        width,
        height,
        rgba8: rgba.into_raw(),
    })
}

/// Encode the 512 px cache entry and write it, here on the worker. The UI
/// used to do this per thumbnail inside the frame, plus a full cache walk.
fn cache_thumbnail(cache_dir: &Path, source: &Path, preview: &sp_raw::Preview, prepared: bool) {
    // A prepared render wins over an embedded-preview decode that was
    // already in flight when its sidecar landed.
    static WRITES: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let Ok(_write) = WRITES.lock() else {
        return;
    };
    let Some(key) = sp_catalog::thumbnails::cache_key(cache_dir, source) else {
        return;
    };
    if key.exists() && !prepared {
        return;
    }
    let Some(image) =
        image::RgbaImage::from_raw(preview.width, preview.height, preview.rgba8.clone())
    else {
        return;
    };
    let image = image::DynamicImage::ImageRgba8(image).thumbnail(512, 512);
    let mut bytes = std::io::Cursor::new(Vec::new());
    if image.write_to(&mut bytes, image::ImageFormat::Png).is_err() {
        return;
    }
    if let Err(error) = sp_catalog::thumbnails::store(cache_dir, &key, bytes.get_ref()) {
        tracing::debug!("thumbnail cache write failed: {error}");
        return;
    }
    if let Err(error) = sp_catalog::thumbnails::enforce_budget_if_due(
        cache_dir,
        sp_catalog::thumbnails::THUMB_CACHE_BUDGET_BYTES,
    ) {
        tracing::debug!("thumbnail cache budget walk failed: {error}");
    }
}

fn persist_worker(
    requests: &Receiver<PersistRequest>,
    results: &Sender<JobResult>,
    sidecar_sync: &Sender<(PathBuf, PathBuf)>,
    network_cache_enabled: bool,
) {
    // One connection per catalog path for the life of the worker. Opening
    // per write ran the migration check every time.
    let mut catalog: Option<(PathBuf, sp_catalog::CatalogDb)> = None;
    // Keep only failed local writes, keyed by the actual primary/version
    // sidecar. A successful retry removes the error. A processing barrier
    // alone cannot tell the UI whether it is safe to discard its document.
    let mut failed_sidecars = std::collections::HashMap::<PathBuf, String>::new();
    for request in requests {
        match request {
            PersistRequest::AutoFinish {
                source,
                edit_source,
                before,
                after,
                preview,
                thumbnail_cache,
            } => {
                let result = sp_catalog::load_sidecar(&edit_source).and_then(|current| {
                    if current.unwrap_or_default() != *before {
                        return Err(sp_core::SpError::CorruptData {
                            path: source.display().to_string(),
                            reason: "Auto Finish skipped: edit changed during analysis".into(),
                        });
                    }
                    sp_catalog::save_sidecar(&edit_source, &after)
                });
                if result.is_ok() {
                    if edit_source != source {
                        let _ = sidecar_sync.send((source.clone(), edit_source));
                    }
                    if let Some(cache) = thumbnail_cache {
                        cache_thumbnail(&cache, &source, &preview, true);
                    }
                }
                let _ = results.send(JobResult::AutoFinishSaved {
                    path: source,
                    before,
                    after,
                    preview,
                    error: result.err().map(|e| e.to_string()),
                });
            }
            PersistRequest::Sidecar(write) => {
                let result =
                    write_sidecar(&write, &mut catalog, sidecar_sync, network_cache_enabled);
                let sidecar_path = write.virtual_copy.as_deref().map_or_else(
                    || sp_catalog::sidecar_path(&write.edit_source),
                    |id| sp_catalog::virtual_copy_sidecar_path(&write.edit_source, id),
                );
                if let Err((error, true)) = &result {
                    failed_sidecars.insert(sidecar_path, error.clone());
                } else {
                    failed_sidecars.remove(&sidecar_path);
                }
                if let Err((error, retry)) = result {
                    let _ = results.send(JobResult::PersistFailed {
                        path: write.path.clone(),
                        error,
                        retry,
                    });
                }
            }
            PersistRequest::CheckedSidecar { before, write } => {
                let result=sp_catalog::load_sidecar(&write.edit_source).map_err(|e|(e.to_string(),true)).and_then(|current|{
                    if current.as_ref()!=before.as_deref() {Err(("Annotation skipped: this photo's edit changed before the save completed".into(),true))} else {write_sidecar(&write,&mut catalog,sidecar_sync,network_cache_enabled)}
                });
                let saved = match &result {
                    Ok(()) => true,
                    Err((_, not_saved)) => !*not_saved,
                };
                if saved {
                    let _ = results.send(JobResult::Annotated {
                        path: write.path.clone(),
                        annotations: write.annotations,
                        color_label: write.metadata.color_label,
                    });
                }
                if let Err((error, _)) = result {
                    let _ = results.send(JobResult::PersistFailed {
                        path: write.path.clone(),
                        error,
                        retry: false,
                    });
                }
            }
            PersistRequest::CatalogMetadata {
                catalog: db_path,
                path,
                meta,
            } => {
                if let Some(db) = open_catalog(&mut catalog, &db_path) {
                    if let Err(error) = db.set_metadata(&path, &meta) {
                        tracing::debug!("catalog metadata write failed: {error}");
                    }
                }
            }
            PersistRequest::Flush(reply) => {
                // Maintenance callers may restore the catalog after this
                // barrier. Release the worker's idle SQLite handle first.
                catalog = None;
                let _ = reply.send(());
            }
            PersistRequest::ConfirmSidecar {
                sidecar_path,
                reply,
            } => {
                catalog = None;
                let _ = reply.send(
                    failed_sidecars
                        .get(&sidecar_path)
                        .cloned()
                        .map_or(Ok(()), Err),
                );
            }
        }
    }
}

fn open_catalog<'a>(
    slot: &'a mut Option<(PathBuf, sp_catalog::CatalogDb)>,
    db_path: &Path,
) -> Option<&'a sp_catalog::CatalogDb> {
    if slot.as_ref().is_none_or(|(path, _)| path != db_path) {
        match sp_catalog::CatalogDb::open(db_path) {
            Ok(db) => *slot = Some((db_path.to_path_buf(), db)),
            Err(error) => {
                tracing::warn!("catalog open failed: {error}");
                *slot = None;
                return None;
            }
        }
    }
    slot.as_ref().map(|(_, db)| db)
}

/// Apply one sidecar write. `Err((message, retry))`: `retry` means the
/// `.spedit` did not land and the app should keep the edit dirty.
fn write_sidecar(
    write: &SidecarWrite,
    catalog: &mut Option<(PathBuf, sp_catalog::CatalogDb)>,
    sidecar_sync: &Sender<(PathBuf, PathBuf)>,
    network_cache_enabled: bool,
) -> Result<(), (String, bool)> {
    let SidecarWrite {
        sidecar,
        path,
        edit_source,
        virtual_copy,
        write_xmp,
        xmp_annotations_only,
        xmp_revision,
        annotations,
        metadata,
        catalog: catalog_path,
    } = write;
    let saved = if let Some(copy_id) = virtual_copy {
        let cache_path = sp_catalog::virtual_copy_sidecar_path(edit_source, copy_id);
        sp_catalog::save_sidecar_path(&cache_path, sidecar)
    } else {
        sp_catalog::save_sidecar(edit_source, sidecar)
    };
    if let Err(error) = saved {
        tracing::error!("sidecar save failed: {error}");
        return Err((error.to_string(), true));
    }
    if edit_source != path && virtual_copy.is_none() && network_cache_enabled {
        let _ = sidecar_sync.send((path.clone(), edit_source.clone()));
    }
    let mirror_error = if edit_source != path {
        virtual_copy.as_ref().and_then(|copy_id| {
            sp_catalog::save_sidecar_path(
                &sp_catalog::virtual_copy_sidecar_path(path, copy_id),
                sidecar,
            )
            .err()
        })
    } else {
        None
    };
    if let Some(db_path) = catalog_path {
        if let Some(db) = open_catalog(catalog, db_path) {
            let _ = db.set_annotations(path, *annotations);
        }
    }
    if *write_xmp {
        // XMP is opt-in interoperability data. Keep its canonical location
        // beside the source; the full ShopPhoto edit was already committed
        // to fast local storage above, so this failure is non-fatal.
        let result = if *xmp_annotations_only {
            sp_catalog::xmp::save(path, *annotations)
        } else if let Some(revision) = xmp_revision {
            match revision.lock() {
                Ok(mut revision) => sp_catalog::xmp::save_with_metadata_if_unchanged(
                    path,
                    *annotations,
                    metadata,
                    &revision,
                )
                .map(|saved| *revision = saved),
                Err(_) => Err(sp_core::SpError::Catalog(
                    "XMP revision lock unavailable".into(),
                )),
            }
        } else {
            sp_catalog::xmp::save_with_metadata(path, *annotations, metadata)
        };
        if let Err(error) = result {
            tracing::warn!("XMP sidecar write failed: {error}");
            return Err((
                crate::i18n::format("Could not write XMP sidecar ({0})", &[error.to_string()]),
                false,
            ));
        }
    }
    if let Some(error) = mirror_error {
        return Err((
            crate::i18n::format(
                "Version saved locally; reconnect to sync it on the next save ({0})",
                &[error.to_string()],
            ),
            false,
        ));
    }
    Ok(())
}

fn run_ai_request(
    request: JobRequest,
    results: &Sender<JobResult>,
    current_generation: &std::sync::atomic::AtomicU64,
) {
    let JobRequest::AiDenoise {
        path,
        raw,
        meta,
        mode,
        compute,
        inferred,
        generation,
    } = request
    else {
        return;
    };
    if generation != current_generation.load(std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    let started = std::time::Instant::now();
    if mode == sp_core::edit::CameraDenoiseMode::Off {
        return;
    }
    let strength = mode.strength(meta.iso);
    let inferred_now = inferred.is_none();
    let result = match inferred {
        Some(inferred) => Ok(inferred),
        None => sp_ai::model_root()
            .ok_or_else(|| sp_ai::AiError::CoreMl("model folder not found".into()))
            .and_then(|root| crate::inference_cache::infer_with_compute(&raw, &root, compute)),
    };
    if generation != current_generation.load(std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    let event = match result {
        Ok(inferred) => JobResult::AiDenoised {
            path,
            raw: Arc::new(sp_ai::blend(&raw, &inferred, strength)),
            inferred,
            mode,
            generation,
            elapsed: started.elapsed(),
            inferred_now,
        },
        Err(error) => JobResult::AiDenoiseFailed {
            path,
            mode,
            generation,
            error: error.to_string(),
        },
    };
    let _ = results.send(event);
}

impl Drop for JobSystem {
    fn drop(&mut self) {
        self.decode_queue.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queued_xmp_saves_share_revision_and_refuse_later_foreign_edits() {
        let temp = tempfile::tempdir().unwrap();
        let photo = temp.path().join("photo.dng");
        std::fs::write(&photo, b"raw").unwrap();
        let baseline = Arc::new(std::sync::Mutex::new(
            sp_catalog::xmp::revision(&photo).unwrap(),
        ));
        let jobs = JobSystem::start(None, None);
        let save = |rating: u8| {
            let mut sidecar = sp_catalog::Sidecar::default();
            sidecar.annotations.rating = rating;
            jobs.persist(PersistRequest::Sidecar(Box::new(SidecarWrite {
                annotations: sidecar.annotations,
                metadata: sidecar.metadata.clone(),
                sidecar,
                path: photo.clone(),
                edit_source: photo.clone(),
                virtual_copy: None,
                write_xmp: true,
                xmp_annotations_only: false,
                xmp_revision: Some(baseline.clone()),
                catalog: None,
            })));
        };
        save(2);
        save(4);
        assert!(jobs.flush_persist(std::time::Duration::from_secs(5)));
        assert!(jobs.poll().is_empty());
        assert_eq!(sp_catalog::xmp::load(&photo).unwrap().unwrap().rating, 4);
        let foreign = sp_catalog::xmp::to_packet(sp_catalog::Annotations {
            rating: 1,
            flag: sp_catalog::Flag::None,
        });
        std::fs::write(sp_catalog::xmp::xmp_path(&photo), &foreign).unwrap();
        save(5);
        assert!(jobs.flush_persist(std::time::Duration::from_secs(5)));
        assert!(matches!(
            jobs.poll().as_slice(),
            [JobResult::PersistFailed { retry: false, .. }]
        ));
        assert_eq!(
            std::fs::read_to_string(sp_catalog::xmp::xmp_path(&photo)).unwrap(),
            foreign
        );
        assert_eq!(
            sp_catalog::load_sidecar(&photo)
                .unwrap()
                .unwrap()
                .annotations
                .rating,
            5
        );
        // The local edit is committed even though preserving a foreign XMP
        // revision failed. This must not strand the photographer on the photo.
        jobs.confirm_sidecar(&photo, None, std::time::Duration::from_secs(5))
            .unwrap();
    }

    #[test]
    fn auto_finish_commits_before_reporting_and_rejects_stale_edits() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("batch.3FR");
        std::fs::write(&path, b"raw").unwrap();
        let jobs = JobSystem::start(None, None);
        let before = sp_catalog::Sidecar::default();
        let mut after = before.clone();
        after.edit.tone.exposure_ev = 1.0;
        for stale in [false, true] {
            jobs.persist(PersistRequest::AutoFinish {
                source: path.clone(),
                edit_source: path.clone(),
                before: Box::new(before.clone()),
                after: Box::new(after.clone()),
                preview: sp_raw::Preview {
                    width: 1,
                    height: 1,
                    rgba8: vec![0, 0, 0, 255],
                },
                thumbnail_cache: None,
            });
            assert!(jobs.flush_persist(std::time::Duration::from_secs(5)));
            assert_eq!(sp_catalog::load_sidecar(&path).unwrap().unwrap(), after);
            let results = jobs.poll();
            assert!(
                matches!(results.as_slice(), [JobResult::AutoFinishSaved { error, .. }] if error.is_some() == stale)
            );
        }
    }

    #[test]
    fn auto_finish_syncs_the_new_cached_edit_to_the_original() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("original.dng");
        let cached = temp.path().join("cached.dng");
        let before = sp_catalog::Sidecar::default();
        let mut after = before.clone();
        after.edit.tone.exposure_ev = 1.0;
        sp_catalog::save_sidecar(&source, &before).unwrap();
        sp_catalog::save_sidecar(&cached, &before).unwrap();
        let (requests_tx, requests_rx) = unbounded();
        let (results_tx, results_rx) = unbounded();
        let (sync_tx, sync_rx) = unbounded();
        requests_tx
            .send(PersistRequest::AutoFinish {
                source: source.clone(),
                edit_source: cached.clone(),
                before: Box::new(before),
                after: Box::new(after.clone()),
                preview: sp_raw::Preview {
                    width: 1,
                    height: 1,
                    rgba8: vec![0, 0, 0, 255],
                },
                thumbnail_cache: None,
            })
            .unwrap();
        drop(requests_tx);
        persist_worker(&requests_rx, &results_tx, &sync_tx, true);
        assert!(matches!(
            results_rx.try_recv().unwrap(),
            JobResult::AutoFinishSaved { error: None, .. }
        ));
        // Follow the same (original, cached) contract as the sync worker.
        let (original, local) = sync_rx.try_recv().unwrap();
        assert_eq!(original, source);
        assert_eq!(local, cached);
        sp_catalog::source_cache::sync_sidecar(&local, &original).unwrap();
        assert_eq!(sp_catalog::load_sidecar(&source).unwrap().unwrap(), after);
        assert_eq!(sp_catalog::load_sidecar(&cached).unwrap().unwrap(), after);
    }

    #[test]
    fn flush_replies_after_earlier_writes_are_applied() {
        let temp = tempfile::tempdir().expect("tempdir");
        let jobs = JobSystem::start(None, None);
        let photo = temp.path().join("shot.3FR");
        std::fs::write(&photo, b"raw").expect("write");
        let sidecar = sp_catalog::Sidecar {
            version: sp_core::edit::EDIT_SCHEMA_VERSION,
            edit: sp_core::edit::EditState::default(),
            annotations: sp_catalog::Annotations::default(),
            metadata: sp_catalog::DescriptiveMetadata::default(),
            snapshots: Vec::new(),
            history: Vec::new(),
            virtual_copy: None,
            virtual_copy_name: None,
        };
        jobs.persist(PersistRequest::Sidecar(Box::new(SidecarWrite {
            sidecar,
            path: photo.clone(),
            edit_source: photo.clone(),
            virtual_copy: None,
            write_xmp: false,
            xmp_annotations_only: false,
            xmp_revision: None,
            annotations: sp_catalog::Annotations::default(),
            metadata: sp_catalog::DescriptiveMetadata::default(),
            catalog: None,
        })));
        assert!(jobs.flush_persist(std::time::Duration::from_secs(5)));
        assert!(sp_catalog::sidecar_path(&photo).exists());
        assert!(jobs.poll().is_empty(), "a successful write reports nothing");
    }

    #[test]
    fn a_failed_sidecar_write_asks_for_a_retry() {
        let jobs = JobSystem::start(None, None);
        let photo = PathBuf::from("/definitely/not/a/dir/shot.3FR");
        let sidecar = sp_catalog::Sidecar {
            version: sp_core::edit::EDIT_SCHEMA_VERSION,
            edit: sp_core::edit::EditState::default(),
            annotations: sp_catalog::Annotations::default(),
            metadata: sp_catalog::DescriptiveMetadata::default(),
            snapshots: Vec::new(),
            history: Vec::new(),
            virtual_copy: None,
            virtual_copy_name: None,
        };
        jobs.persist(PersistRequest::Sidecar(Box::new(SidecarWrite {
            sidecar,
            path: photo.clone(),
            edit_source: photo.clone(),
            virtual_copy: None,
            write_xmp: false,
            xmp_annotations_only: false,
            xmp_revision: None,
            annotations: sp_catalog::Annotations::default(),
            metadata: sp_catalog::DescriptiveMetadata::default(),
            catalog: None,
        })));
        assert!(jobs.flush_persist(std::time::Duration::from_secs(5)));
        let results = jobs.poll();
        assert!(matches!(
            results.as_slice(),
            [JobResult::PersistFailed { retry: true, path, .. }] if *path == photo
        ));
    }

    #[test]
    fn confirming_a_document_refuses_failed_saves_until_that_version_is_retried() {
        let temp = tempfile::tempdir().unwrap();
        let jobs = JobSystem::start(None, None);
        for version in [None, Some("copy-0001")] {
            let photo = temp.path().join(if version.is_some() {
                "version.dng"
            } else {
                "primary.dng"
            });
            let mut sidecar = sp_catalog::Sidecar {
                virtual_copy: version.map(str::to_owned),
                ..Default::default()
            };
            sidecar.edit.tone.exposure_ev = 1.25;
            let destination = version.map_or_else(
                || sp_catalog::sidecar_path(&photo),
                |id| sp_catalog::virtual_copy_sidecar_path(&photo, id),
            );
            // A directory at the sidecar destination deterministically
            // rejects the atomic save, including on Windows and as root.
            std::fs::create_dir(&destination).unwrap();
            let save = || {
                jobs.persist(PersistRequest::Sidecar(Box::new(SidecarWrite {
                    sidecar: sidecar.clone(),
                    path: photo.clone(),
                    edit_source: photo.clone(),
                    virtual_copy: version.map(str::to_owned),
                    write_xmp: false,
                    xmp_annotations_only: false,
                    xmp_revision: None,
                    annotations: sidecar.annotations,
                    metadata: sidecar.metadata.clone(),
                    catalog: None,
                })))
            };
            save();
            // Merely processing the save is insufficient evidence to close
            // the outgoing document, even after its error event was drained.
            assert!(jobs.flush_persist(std::time::Duration::from_secs(5)));
            assert!(matches!(
                jobs.poll().as_slice(),
                [JobResult::PersistFailed { retry: true, .. }]
            ));
            assert!(jobs
                .confirm_sidecar(&photo, version, std::time::Duration::from_secs(5))
                .is_err());
            let other_version = if version.is_some() {
                None
            } else {
                Some("copy-0001")
            };
            assert!(jobs
                .confirm_sidecar(&photo, other_version, std::time::Duration::from_secs(5))
                .is_ok());
            std::fs::remove_dir(&destination).unwrap();
            save();
            // A successful retry clears only this version's failed outcome.
            jobs.confirm_sidecar(&photo, version, std::time::Duration::from_secs(5))
                .unwrap();
            assert_eq!(
                sp_catalog::load_sidecar_path(&destination)
                    .unwrap()
                    .unwrap(),
                sidecar
            );
        }
    }

    #[test]
    fn an_offline_virtual_copy_keeps_local_edit_when_remote_sync_fails() {
        let dir = tempfile::tempdir().unwrap();
        let local = dir.path().join("cached.dng");
        let remote = dir.path().join("unmounted/original.dng");
        let jobs = JobSystem::start(None, None);
        let copy = "copy-0001".to_owned();
        let mut sidecar = sp_catalog::Sidecar {
            virtual_copy: Some(copy.clone()),
            virtual_copy_name: Some("Portrait".into()),
            ..Default::default()
        };
        sidecar.edit.tone.exposure_ev = 1.0;
        jobs.persist(PersistRequest::Sidecar(Box::new(SidecarWrite {
            sidecar: sidecar.clone(),
            path: remote,
            edit_source: local.clone(),
            virtual_copy: Some(copy.clone()),
            write_xmp: false,
            xmp_annotations_only: false,
            xmp_revision: None,
            annotations: sidecar.annotations,
            metadata: sidecar.metadata.clone(),
            catalog: None,
        })));
        assert!(jobs.flush_persist(std::time::Duration::from_secs(5)));
        let saved =
            sp_catalog::load_sidecar_path(&sp_catalog::virtual_copy_sidecar_path(&local, &copy))
                .unwrap()
                .unwrap();
        assert_eq!(saved, sidecar);
        // A failed remote mirror does not make a safe local version unsafe
        // to leave; the full edit can still be read from its local sidecar.
        jobs.confirm_sidecar(&local, Some(&copy), std::time::Duration::from_secs(5))
            .unwrap();
        assert!(matches!(
            jobs.poll().as_slice(),
            [JobResult::PersistFailed { retry: false, .. }]
        ));
    }

    #[test]
    fn checked_annotations_refuse_foreign_changes_and_malformed_sidecars() {
        let dir = tempfile::tempdir().unwrap();
        let photo = dir.path().join("photo.dng");
        std::fs::write(&photo, b"raw").unwrap();
        let before = sp_catalog::Sidecar::default();
        let mut after = before.clone();
        after.annotations.rating = 4;
        after.annotations.flag = sp_catalog::Flag::Pick;
        let jobs = JobSystem::start(None, None);
        let write = || SidecarWrite {
            sidecar: after.clone(),
            path: photo.clone(),
            edit_source: photo.clone(),
            virtual_copy: None,
            write_xmp: false,
            xmp_annotations_only: true,
            xmp_revision: None,
            annotations: after.annotations,
            metadata: after.metadata.clone(),
            catalog: None,
        };
        let mut foreign = before.clone();
        foreign.edit.tone.exposure_ev = 1.5;
        sp_catalog::save_sidecar(&photo, &foreign).unwrap();
        jobs.persist(PersistRequest::CheckedSidecar {
            before: Some(Box::new(before.clone())),
            write: Box::new(write()),
        });
        assert!(jobs.flush_persist(std::time::Duration::from_secs(5)));
        assert!(matches!(
            jobs.poll().as_slice(),
            [JobResult::PersistFailed { retry: false, .. }]
        ));
        assert_eq!(sp_catalog::load_sidecar(&photo).unwrap().unwrap(), foreign);
        std::fs::write(sp_catalog::sidecar_path(&photo), b"malformed json").unwrap();
        jobs.persist(PersistRequest::CheckedSidecar {
            before: Some(Box::new(before.clone())),
            write: Box::new(write()),
        });
        assert!(jobs.flush_persist(std::time::Duration::from_secs(5)));
        assert!(matches!(
            jobs.poll().as_slice(),
            [JobResult::PersistFailed { retry: false, .. }]
        ));
        assert_eq!(
            std::fs::read(sp_catalog::sidecar_path(&photo)).unwrap(),
            b"malformed json"
        );
        sp_catalog::save_sidecar(&photo, &before).unwrap();
        jobs.persist(PersistRequest::CheckedSidecar {
            before: Some(Box::new(before)),
            write: Box::new(write()),
        });
        assert!(jobs.flush_persist(std::time::Duration::from_secs(5)));
        assert!(
            matches!(jobs.poll().as_slice(),[JobResult::Annotated{annotations,..}] if annotations.rating==4 && annotations.flag==sp_catalog::Flag::Pick)
        );
    }
}
