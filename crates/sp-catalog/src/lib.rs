//! # sp-catalog
//!
//! Non-destructive sidecar IO plus the SQLite/WAL library catalog, collection,
//! thumbnail-cache and filesystem-watcher services.

use serde::{Deserialize, Serialize};
use sp_core::{EditState, SpError};
use std::path::{Path, PathBuf};

pub mod catalog;
pub mod project;
pub mod quality;
pub mod search;
pub mod source_cache;
pub mod thumbnails;
pub mod watcher;
pub mod workflow;
pub mod xmp;

pub use catalog::{
    CatalogDb, CatalogPhoto, Collection, FolderRecord, LibraryFilter, LibrarySort, SmartCollection,
};
pub use watcher::FolderWatcher;

/// Five interoperable color labels plus no label.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ColorLabel {
    #[default]
    None,
    Red,
    Yellow,
    Green,
    Blue,
    Purple,
}

/// Searchable descriptive and location metadata owned by the sidecar.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DescriptiveMetadata {
    pub caption: String,
    pub keywords: Vec<String>,
    pub color_label: ColorLabel,
    pub gps: Option<[f64; 2]>,
    #[serde(default)]
    pub creator: String,
    #[serde(default)]
    pub copyright: String,
    #[serde(default)]
    pub rights: String,
    /// Creator's contact email (IPTC Core CiEmailWork).
    #[serde(default)]
    pub contact: String,
}

/// A named, persistent edit checkpoint.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NamedSnapshot {
    pub name: String,
    pub created_s: i64,
    pub edit: EditState,
}

/// Persistent history entry. The bounded app undo stack remains optimized
/// for gestures; this compact list restores meaningful steps after reopen.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub label: String,
    pub created_s: i64,
    pub edit: EditState,
}

/// Star rating 0–5 plus pick/reject flags, shared by filmstrip and library.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Annotations {
    /// 0 = unrated, 1..=5 stars.
    pub rating: u8,
    pub flag: Flag,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Flag {
    #[default]
    None,
    Pick,
    Reject,
}

/// The on-disk sidecar document. Written as pretty JSON next to the
/// original: `IMG_0001.3FR` → `IMG_0001.3FR.spedit`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Sidecar {
    /// Schema version — [`sp_core::edit::EDIT_SCHEMA_VERSION`] at write time.
    pub version: u32,
    pub edit: EditState,
    pub annotations: Annotations,
    #[serde(default)]
    pub metadata: DescriptiveMetadata,
    #[serde(default)]
    pub snapshots: Vec<NamedSnapshot>,
    #[serde(default)]
    pub history: Vec<HistoryEntry>,
    /// Virtual-copy identifier. `None` is the source image's primary edit.
    #[serde(default)]
    pub virtual_copy: Option<String>,
    /// Display name of a virtual copy; the stable file identifier is unchanged.
    #[serde(default)]
    pub virtual_copy_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VirtualCopyRecord {
    pub version: u32,
    pub id: String,
    pub name: String,
}

/// Sidecar path for an original image path (append `.spedit`).
#[must_use]
pub fn sidecar_path(original: &Path) -> PathBuf {
    let mut os = original.as_os_str().to_owned();
    os.push(".spedit");
    PathBuf::from(os)
}

/// Sidecar path for a virtual copy without duplicating the original RAW.
#[must_use]
pub fn virtual_copy_sidecar_path(original: &Path, copy_id: &str) -> PathBuf {
    let safe: String = copy_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let mut os = original.as_os_str().to_owned();
    os.push(format!(".{safe}.spedit"));
    PathBuf::from(os)
}

/// Load the sidecar for `original`, if one exists.
///
/// Out-of-range values are clamped ([`EditState::clamped`]); a corrupt file
/// yields [`SpError::CorruptData`] so the caller can quarantine it (rename to
/// `.spedit.corrupt`) and continue with defaults — original files and bad
/// sidecars are never deleted (BUILD.md Phase 8 step 4).
pub fn load_sidecar(original: &Path) -> Result<Option<Sidecar>, SpError> {
    load_sidecar_path(&sidecar_path(original))
}

/// Load a sidecar document from an exact path (used by virtual copies).
pub fn load_sidecar_path(path: &Path) -> Result<Option<Sidecar>, SpError> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(SpError::io(path.display().to_string(), e)),
    };
    check_sidecar_version(path, &bytes)?;
    let mut sidecar: Sidecar =
        serde_json::from_slice(&bytes).map_err(|e| SpError::CorruptData {
            path: path.display().to_string(),
            reason: e.to_string(),
        })?;
    sidecar.edit = sidecar.edit.clamped();
    Ok(Some(sidecar))
}

// Inspect the envelope first: a future version may also change the edit shape.
fn check_sidecar_version(path: &Path, bytes: &[u8]) -> Result<(), SpError> {
    #[derive(Deserialize)]
    struct Envelope {
        version: u64,
    }
    if let Ok(envelope) = serde_json::from_slice::<Envelope>(bytes) {
        if envelope.version > u64::from(sp_core::edit::EDIT_SCHEMA_VERSION) {
            return Err(SpError::UnsupportedSidecarVersion {
                path: path.display().to_string(),
                version: envelope.version,
                supported: sp_core::edit::EDIT_SCHEMA_VERSION,
            });
        }
    }
    Ok(())
}

/// Write the sidecar for `original` atomically (temp file + rename), so a
/// crash mid-write can never destroy the previous good sidecar.
pub fn save_sidecar(original: &Path, sidecar: &Sidecar) -> Result<(), SpError> {
    save_sidecar_path(&sidecar_path(original), sidecar)
}

/// Atomically save a sidecar document to an exact path.
pub fn save_sidecar_path(path: &Path, sidecar: &Sidecar) -> Result<(), SpError> {
    // A failed load must never allow a later default edit to replace newer data.
    match std::fs::read(path) {
        Ok(bytes) => check_sidecar_version(path, &bytes)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(SpError::io(path.display().to_string(), error)),
    }
    let json = serde_json::to_vec_pretty(sidecar).map_err(|e| SpError::CorruptData {
        path: path.display().to_string(),
        reason: e.to_string(),
    })?;
    atomic_write(path, &json)
}

/// Publish a complete, flushed document using a private sibling temporary file.
/// Concurrent writers must never truncate or rename one another's staging file.
pub(crate) fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), SpError> {
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};

    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let (temporary, mut output) = loop {
        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let mut name = path.as_os_str().to_owned();
        name.push(format!(".{}.{sequence}.tmp", std::process::id()));
        let temporary = PathBuf::from(name);
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
        {
            Ok(output) => break (temporary, output),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(SpError::io(temporary.display().to_string(), error)),
        }
    };
    let result = output.write_all(bytes).and_then(|_| output.sync_all());
    drop(output);
    let result = result
        .map_err(|error| SpError::io(temporary.display().to_string(), error))
        .and_then(|()| {
            std::fs::rename(&temporary, path)
                .map_err(|error| SpError::io(path.display().to_string(), error))
        });
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    } else if let Some(parent) = path.parent() {
        // Directory syncing is unavailable on some platforms/filesystems.
        if let Ok(directory) = std::fs::File::open(parent) {
            let _ = directory.sync_all();
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use sp_core::edit::EDIT_SCHEMA_VERSION;

    #[test]
    fn sidecar_path_appends_full_extension() {
        assert_eq!(
            sidecar_path(Path::new("/a/IMG_0001.3FR")),
            PathBuf::from("/a/IMG_0001.3FR.spedit")
        );
    }

    #[test]
    fn round_trip_and_missing() {
        let dir = std::env::temp_dir().join("sp-catalog-test-rt");
        std::fs::create_dir_all(&dir).expect("mkdir");
        let original = dir.join("shot.3FR");

        assert!(load_sidecar(&original).expect("load none").is_none());

        let mut sc = Sidecar {
            version: EDIT_SCHEMA_VERSION,
            ..Default::default()
        };
        sc.edit.tone.exposure_ev = 0.7;
        sc.annotations.rating = 4;
        save_sidecar(&original, &sc).expect("save");

        let back = load_sidecar(&original).expect("load").expect("some");
        assert_eq!(back, sc);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn corrupt_sidecar_is_reported_not_swallowed() {
        let dir = std::env::temp_dir().join("sp-catalog-test-corrupt");
        std::fs::create_dir_all(&dir).expect("mkdir");
        let original = dir.join("shot.3FR");
        std::fs::write(sidecar_path(&original), b"{ not json").expect("write");
        let err = load_sidecar(&original).expect_err("must error");
        assert!(matches!(err, SpError::CorruptData { .. }));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn future_sidecar_version_is_preserved_and_not_corruption() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("photo.spedit");
        let bytes = format!(
            r#"{{"version":{},"edit":{{"future_field":true}}}}"#,
            EDIT_SCHEMA_VERSION + 1
        );
        std::fs::write(&path, &bytes).unwrap();
        assert!(matches!(
            load_sidecar_path(&path),
            Err(SpError::UnsupportedSidecarVersion { .. })
        ));
        assert!(matches!(
            save_sidecar_path(&path, &Sidecar::default()),
            Err(SpError::UnsupportedSidecarVersion { .. })
        ));
        assert_eq!(std::fs::read_to_string(path).unwrap(), bytes);
    }

    #[test]
    fn concurrent_sidecar_saves_publish_complete_documents() {
        let dir = tempfile::tempdir().unwrap();
        let original = dir.path().join("photo.dng");
        // An interrupted write from an older version must not be overwritten.
        let abandoned = sidecar_path(&original).with_extension("spedit.tmp");
        std::fs::write(&abandoned, b"recoverable edits").unwrap();
        std::thread::scope(|scope| {
            let writers: Vec<_> = (0..8)
                .map(|index| {
                    let original = &original;
                    scope.spawn(move || {
                        let mut sidecar = Sidecar::default();
                        sidecar.metadata.caption = index.to_string().repeat(16 * 1024);
                        for _ in 0..8 {
                            save_sidecar(original, &sidecar).unwrap();
                        }
                    })
                })
                .collect();
            for writer in writers {
                writer.join().unwrap();
            }
        });
        let saved = load_sidecar(&original).unwrap().unwrap();
        assert_eq!(saved.metadata.caption.len(), 16 * 1024);
        assert!(saved
            .metadata
            .caption
            .bytes()
            .all(|byte| byte == saved.metadata.caption.as_bytes()[0]));
        assert_eq!(std::fs::read(abandoned).unwrap(), b"recoverable edits");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
    }

    #[test]
    fn failed_sidecar_publication_cleans_staging_without_removing_target() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("edits.spedit");
        std::fs::create_dir(&path).unwrap();
        assert!(save_sidecar_path(&path, &Sidecar::default()).is_err());
        assert!(path.is_dir());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }
}
