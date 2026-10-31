//! Local working cache for RAWs opened from network-mounted filesystems.
//!
//! Decoding repeatedly seeks through large RAW files, and edit sidecars are
//! saved often. Both operations feel sluggish over SMB/NFS. Network sources
//! are therefore copied to the app cache once and all interactive work uses
//! that local copy. The original path remains the image identity used by the
//! catalog and UI.

use crate::sidecar_path;
use serde::{Deserialize, Serialize};
use sp_core::SpError;
use std::path::{Path, PathBuf};

static TEMP_SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn temporary_path(dir: &Path, name: &str) -> PathBuf {
    let sequence = TEMP_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    dir.join(format!(
        "{name}.{}.{}.copying",
        std::process::id(),
        sequence
    ))
}

/// Maximum amount of cached RAW sensor data. Edit sidecars are never evicted.
pub const SOURCE_CACHE_BUDGET_BYTES: u64 = 20 * 1024 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct PreparedSource {
    /// Local path passed to the RAW decoder (or the original path for local files).
    pub decode_path: PathBuf,
    /// True when `decode_path` is an app-owned local cache entry.
    pub cached: bool,
    /// Original is unavailable; decoding uses a verified local copy.
    pub offline: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct SourceStamp {
    len: u64,
    modified_ns: Option<u128>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Manifest {
    #[serde(default = "manifest_version")]
    version: u32,
    source: String,
    stamp: SourceStamp,
    #[serde(default)]
    content_hash: Option<String>,
}

fn manifest_version() -> u32 {
    1
}

fn entry_directory(cache_root: &Path, source: &Path) -> PathBuf {
    let key = blake3::hash(source.as_os_str().as_encoded_bytes())
        .to_hex()
        .to_string();
    cache_root.join(key)
}

/// Path of the cached original; does not access the source or create files.
#[must_use]
pub fn cached_path(cache_root: &Path, source: &Path) -> PathBuf {
    let extension = source
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("raw");
    entry_directory(cache_root, source).join(format!("source.{extension}"))
}

/// Prepare a source for interactive decoding. Local files pass through;
/// network-mounted files are materialized below `cache_root`.
pub fn prepare(cache_root: &Path, source: &Path) -> Result<PreparedSource, SpError> {
    if !is_network_path(source)
        && !entry_directory(cache_root, source)
            .join("manifest.json")
            .is_file()
    {
        return Ok(PreparedSource {
            decode_path: source.to_owned(),
            cached: false,
            offline: false,
        });
    }
    prepare_cached(cache_root, source)
}

fn prepare_cached(cache_root: &Path, source: &Path) -> Result<PreparedSource, SpError> {
    let source_display = source.display().to_string();
    let entry_dir = entry_directory(cache_root, source);
    let extension = source
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("raw");
    let decode_path = entry_dir.join(format!("source.{extension}"));
    let manifest_path = entry_dir.join("manifest.json");
    let cached_manifest = std::fs::read(&manifest_path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Manifest>(&bytes).ok());
    let metadata = match std::fs::metadata(source) {
        Ok(metadata) => metadata,
        Err(error) => {
            let valid = cached_manifest.as_ref().is_some_and(|manifest| {
                manifest.version == manifest_version()
                    && manifest.source == source_display
                    && manifest.content_hash.as_ref().is_some_and(|expected| {
                        crate::workflow::content_hash(&decode_path)
                            .is_ok_and(|actual| actual == *expected)
                    })
            });
            if !valid {
                return Err(SpError::io(&source_display, error));
            }
            let _ = std::fs::write(entry_dir.join("last-access"), []);
            return Ok(PreparedSource {
                decode_path,
                cached: true,
                offline: true,
            });
        }
    };
    let stamp = SourceStamp {
        len: metadata.len(),
        modified_ns: metadata.modified().ok().and_then(|time| {
            time.duration_since(std::time::UNIX_EPOCH)
                .ok()
                .map(|duration| duration.as_nanos())
        }),
    };

    std::fs::create_dir_all(&entry_dir)
        .map_err(|e| SpError::io(entry_dir.display().to_string(), e))?;
    let cache_is_current = std::fs::metadata(&decode_path)
        .is_ok_and(|metadata| metadata.is_file() && metadata.len() == stamp.len)
        && cached_manifest.as_ref().is_some_and(|manifest| {
            manifest.version == manifest_version()
                && manifest.source == source_display
                && manifest.stamp == stamp
                && manifest.content_hash.is_some()
        });

    if !cache_is_current {
        let temp = temporary_path(&entry_dir, &format!("source.{extension}"));
        let content_hash = crate::workflow::copy_verified(source, &temp)?;
        if let Err(error) = std::fs::rename(&temp, &decode_path) {
            let _ = std::fs::remove_file(&temp);
            return Err(SpError::io(decode_path.display().to_string(), error));
        }
        let manifest = Manifest {
            version: manifest_version(),
            source: source_display,
            stamp,
            content_hash: Some(content_hash),
        };
        let bytes = serde_json::to_vec(&manifest).map_err(|e| SpError::CorruptData {
            path: manifest_path.display().to_string(),
            reason: e.to_string(),
        })?;
        crate::atomic_write(&manifest_path, &bytes)?;
    }

    // Seed the local edit cache once. Thereafter the local sidecar wins: if
    // a share is temporarily unavailable, unsynced edits must remain usable.
    let local_sidecar = sidecar_path(&decode_path);
    if !local_sidecar.exists() {
        let remote_sidecar = sidecar_path(source);
        match std::fs::copy(&remote_sidecar, &local_sidecar) {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => tracing::warn!(
                path = %remote_sidecar.display(),
                "could not seed network sidecar cache: {error}"
            ),
        }
    }
    let local_xmp = crate::xmp::xmp_path(&decode_path);
    if let Ok(copies) = crate::workflow::virtual_copies(source) {
        for id in copies {
            let local = crate::virtual_copy_sidecar_path(&decode_path, &id);
            if !local.exists() {
                let remote = crate::virtual_copy_sidecar_path(source, &id);
                if let Err(error) = crate::workflow::copy_verified(&remote, &local) {
                    tracing::warn!(path = %remote.display(), "could not cache virtual-copy edits: {error}");
                }
            }
        }
    }
    if !local_xmp.exists() {
        let remote_xmp = crate::xmp::xmp_path(source);
        match std::fs::copy(&remote_xmp, &local_xmp) {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => tracing::warn!(
                path = %remote_xmp.display(),
                "could not seed network XMP cache: {error}"
            ),
        }
    }

    // Access marker is the eviction clock; touching it never mutates the RAW.
    let access = entry_dir.join("last-access");
    let _ = std::fs::write(access, []);
    let _ = enforce_budget(cache_root, SOURCE_CACHE_BUDGET_BYTES, Some(&decode_path));

    Ok(PreparedSource {
        decode_path,
        cached: true,
        offline: false,
    })
}

/// Make a verified full-resolution offline copy and exempt it from LRU eviction.
/// Works for local/removable originals as well as network sources.
pub fn pin(cache_root: &Path, source: &Path) -> Result<PreparedSource, SpError> {
    let prepared = prepare_cached(cache_root, source)?;
    let marker = entry_directory(cache_root, source).join("pinned");
    std::fs::write(&marker, []).map_err(|e| SpError::io(marker.display().to_string(), e))?;
    Ok(prepared)
}

pub fn is_pinned(cache_root: &Path, source: &Path) -> bool {
    entry_directory(cache_root, source).join("pinned").is_file()
}

/// Permit eviction again; does not delete cached pixels or edit sidecars.
pub fn unpin(cache_root: &Path, source: &Path) -> Result<(), SpError> {
    let marker = entry_directory(cache_root, source).join("pinned");
    match std::fs::remove_file(&marker) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(SpError::io(marker.display().to_string(), error)),
    }
}

/// Copy the locally saved edit sidecar back beside the network original.
/// The local file remains authoritative if this fails and will be retried on
/// the next edit.
pub fn sync_sidecar(local_original: &Path, network_original: &Path) -> Result<(), SpError> {
    let local = sidecar_path(local_original);
    let remote = sidecar_path(network_original);
    let bytes = std::fs::read(&local).map_err(|e| SpError::io(local.display().to_string(), e))?;
    crate::atomic_write(&remote, &bytes)
}

/// Evict least-recently-used RAW copies while retaining every edit sidecar.
pub fn enforce_budget(
    cache_root: &Path,
    budget: u64,
    protected: Option<&Path>,
) -> Result<(), SpError> {
    let Ok(entries) = std::fs::read_dir(cache_root) else {
        return Ok(());
    };
    let mut cached = Vec::new();
    for entry in entries.flatten() {
        let dir = entry.path();
        if dir.join("pinned").is_file() {
            continue;
        }
        let Some(manifest) = std::fs::read(dir.join("manifest.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Manifest>(&bytes).ok())
        else {
            continue;
        };
        let extension = Path::new(&manifest.source)
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("raw");
        let raw = dir.join(format!("source.{extension}"));
        let Ok(meta) = std::fs::metadata(&raw) else {
            continue;
        };
        let accessed = std::fs::metadata(dir.join("last-access"))
            .and_then(|value| value.modified())
            .or_else(|_| meta.modified())
            .unwrap_or(std::time::UNIX_EPOCH);
        cached.push((raw, meta.len(), accessed));
    }
    let mut total: u64 = cached.iter().map(|(_, size, _)| *size).sum();
    cached.sort_by_key(|(_, _, accessed)| *accessed);
    for (path, size, _) in cached {
        if total <= budget {
            break;
        }
        if protected.is_some_and(|protected| protected == path) {
            continue;
        }
        if std::fs::remove_file(&path).is_ok() {
            total = total.saturating_sub(size);
        }
    }
    Ok(())
}

/// Detect whether a path resides on a network-mounted filesystem.
#[must_use]
pub fn is_network_path(path: &Path) -> bool {
    is_network_path_platform(path)
}

#[cfg(target_os = "macos")]
fn is_network_path_platform(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;

    // Query the kernel's cached mount table instead of calling `statfs(path)`.
    // The latter can itself block for many seconds when an SMB server is gone,
    // which defeated the point of detecting network paths defensively.
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir().unwrap_or_default().join(path)
    };
    let mut mounts = std::ptr::null_mut::<libc::statfs>();
    unsafe extern "C" {
        // Darwin's reentrant variant allocates a private buffer with malloc.
        #[cfg_attr(not(target_arch = "aarch64"), link_name = "getmntinfo_r_np$INODE64")]
        fn getmntinfo_r_np(mounts: *mut *mut libc::statfs, flags: libc::c_int) -> libc::c_int;
    }
    struct MountBuffer(*mut libc::statfs);
    impl Drop for MountBuffer {
        fn drop(&mut self) {
            // SAFETY: this is the allocation returned by getmntinfo_r_np.
            unsafe { libc::free(self.0.cast()) };
        }
    }
    // SAFETY: mounts is a valid out-pointer; the returned allocation is owned.
    let count = unsafe { getmntinfo_r_np(&mut mounts, libc::MNT_NOWAIT) };
    let _buffer = MountBuffer(mounts);
    if count <= 0 || mounts.is_null() {
        return false;
    }
    // SAFETY: the pointer and positive count were returned together above.
    let mounts = unsafe { std::slice::from_raw_parts(mounts, count as usize) };
    let mut best: Option<(usize, bool)> = None;
    for mount in mounts {
        // SAFETY: Darwin statfs mount names are fixed NUL-terminated arrays.
        let name = unsafe { std::ffi::CStr::from_ptr(mount.f_mntonname.as_ptr()) };
        let mount_path = Path::new(std::ffi::OsStr::from_bytes(name.to_bytes()));
        if absolute.starts_with(mount_path)
            && best.is_none_or(|(length, _)| mount_path.as_os_str().len() > length)
        {
            best = Some((
                mount_path.as_os_str().len(),
                mount.f_flags & libc::MNT_LOCAL as u32 == 0,
            ));
        }
    }
    best.is_some_and(|(_, network)| network)
}

#[cfg(target_os = "linux")]
fn is_network_path_platform(path: &Path) -> bool {
    let absolute = path.canonicalize().unwrap_or_else(|_| path.to_owned());
    let Ok(mounts) = std::fs::read_to_string("/proc/self/mountinfo") else {
        return false;
    };
    let mut best: Option<(usize, &str)> = None;
    for line in mounts.lines() {
        let Some((left, right)) = line.split_once(" - ") else {
            continue;
        };
        let fields: Vec<_> = left.split_whitespace().collect();
        let Some(mount) = fields.get(4) else { continue };
        let mount = mount.replace("\\040", " ").replace("\\134", "\\");
        if absolute.starts_with(&mount) {
            let fs_type = right.split_whitespace().next().unwrap_or("");
            if best.is_none_or(|(length, _)| mount.len() > length) {
                best = Some((mount.len(), fs_type));
            }
        }
    }
    best.is_some_and(|(_, kind)| {
        matches!(
            kind,
            "nfs"
                | "nfs4"
                | "cifs"
                | "smb3"
                | "sshfs"
                | "davfs"
                | "9p"
                | "ceph"
                | "glusterfs"
                | "fuse.sshfs"
                | "fuse.rclone"
                | "fuse.gvfsd-fuse"
        )
    })
}

#[cfg(target_os = "windows")]
fn is_network_path_platform(path: &Path) -> bool {
    // UNC paths cover SMB shares. Mapped drives are intentionally left to
    // the OS cache unless/until we can query GetDriveTypeW here.
    path.as_os_str().to_string_lossy().starts_with("\\\\")
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
fn is_network_path_platform(_path: &Path) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pinning_keeps_named_versions_available_offline() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source/photo.dng");
        std::fs::create_dir(source.parent().unwrap()).unwrap();
        std::fs::write(&source, b"raw").unwrap();
        let id = crate::workflow::create_virtual_copy(&source, &crate::Sidecar::default()).unwrap();
        crate::workflow::rename_virtual_copy(&source, &id, "Monochrome").unwrap();
        let cache = dir.path().join("cache");
        let pinned = pin(&cache, &source).unwrap();
        std::fs::rename(source.parent().unwrap(), dir.path().join("unmounted")).unwrap();
        let offline = prepare(&cache, &source).unwrap();
        assert!(offline.offline);
        assert_eq!(offline.decode_path, pinned.decode_path);
        let records = crate::workflow::virtual_copy_records(&offline.decode_path).unwrap();
        assert_eq!(records[0].id, id);
        assert_eq!(records[0].name, "Monochrome");
    }

    #[test]
    fn pinned_original_opens_offline_and_survives_budget_eviction() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("capture.dng");
        let cache = dir.path().join("cache");
        std::fs::write(&source, b"sensor data").unwrap();
        let pinned = pin(&cache, &source).unwrap();
        assert!(is_pinned(&cache, &source));
        assert!(!pinned.offline);
        std::fs::remove_file(&source).unwrap();
        enforce_budget(&cache, 0, None).unwrap();
        let offline = prepare(&cache, &source).unwrap();
        assert!(offline.offline);
        assert_eq!(std::fs::read(&offline.decode_path).unwrap(), b"sensor data");
        unpin(&cache, &source).unwrap();
        enforce_budget(&cache, 0, None).unwrap();
        assert!(!offline.decode_path.exists());
        assert!(prepare(&cache, &source).is_err());
    }

    #[test]
    fn offline_cache_rejects_changed_pixels_and_refreshes_online_source() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("capture.dng");
        let cache = dir.path().join("cache");
        std::fs::write(&source, b"first capture").unwrap();
        let initial = pin(&cache, &source).unwrap();
        std::fs::write(&source, b"new sensor capture").unwrap();
        let refreshed = prepare(&cache, &source).unwrap();
        assert_eq!(
            std::fs::read(&refreshed.decode_path).unwrap(),
            b"new sensor capture"
        );
        std::fs::remove_file(&source).unwrap();
        std::fs::write(&initial.decode_path, b"corrupt bytes").unwrap();
        assert!(prepare(&cache, &source).is_err());
    }

    #[test]
    fn online_cache_repairs_truncated_pixels_without_losing_local_edits() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("capture.dng");
        let cache = dir.path().join("cache");
        std::fs::write(&source, b"complete sensor data").unwrap();
        let initial = pin(&cache, &source).unwrap();
        let edits = sidecar_path(&initial.decode_path);
        std::fs::write(&edits, b"unsynced edits").unwrap();
        std::fs::write(&initial.decode_path, b"truncated").unwrap();

        let repaired = prepare(&cache, &source).unwrap();
        assert_eq!(
            std::fs::read(repaired.decode_path).unwrap(),
            b"complete sensor data"
        );
        assert_eq!(std::fs::read(edits).unwrap(), b"unsynced edits");
    }

    #[test]
    fn unsupported_cache_manifest_is_not_trusted_offline() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("capture.dng");
        let cache = dir.path().join("cache");
        std::fs::write(&source, b"sensor data").unwrap();
        pin(&cache, &source).unwrap();
        let manifest_path = entry_directory(&cache, &source).join("manifest.json");
        let mut manifest: Manifest =
            serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
        manifest.version = manifest_version() + 1;
        std::fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        std::fs::remove_file(&source).unwrap();
        assert!(prepare(&cache, &source).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_source_paths_have_distinct_cache_identities() {
        use std::os::unix::ffi::OsStrExt;

        let cache = Path::new("cache");
        let first = Path::new(std::ffi::OsStr::from_bytes(b"photo-\xff.dng"));
        let second = Path::new(std::ffi::OsStr::from_bytes(b"photo-\xfe.dng"));
        assert_eq!(first.to_string_lossy(), second.to_string_lossy());
        assert_ne!(cached_path(cache, first), cached_path(cache, second));
    }

    #[test]
    fn cached_copy_is_reused_and_keeps_local_edits() {
        let temp = tempfile::tempdir().expect("tempdir");
        let source = temp.path().join("photo.nef");
        std::fs::write(&source, b"raw-one").expect("source");
        std::fs::write(sidecar_path(&source), b"edit-one").expect("sidecar");
        let cache = temp.path().join("cache");

        let first = prepare_cached(&cache, &source).expect("prepare");
        assert_eq!(std::fs::read(&first.decode_path).unwrap(), b"raw-one");
        assert_eq!(
            std::fs::read(sidecar_path(&first.decode_path)).unwrap(),
            b"edit-one"
        );

        std::fs::write(sidecar_path(&first.decode_path), b"local-newer").unwrap();
        let second = prepare_cached(&cache, &source).expect("reuse");
        assert_eq!(second.decode_path, first.decode_path);
        assert_eq!(
            std::fs::read(sidecar_path(&second.decode_path)).unwrap(),
            b"local-newer"
        );
    }

    #[test]
    fn sidecar_sync_is_atomic_and_targets_original() {
        let temp = tempfile::tempdir().expect("tempdir");
        let local = temp.path().join("cached/source.arw");
        let remote = temp.path().join("share/photo.arw");
        std::fs::create_dir_all(local.parent().unwrap()).unwrap();
        std::fs::create_dir_all(remote.parent().unwrap()).unwrap();
        std::fs::write(sidecar_path(&local), b"settings").unwrap();
        sync_sidecar(&local, &remote).expect("sync");
        assert_eq!(std::fs::read(sidecar_path(&remote)).unwrap(), b"settings");
    }

    #[test]
    fn temporary_directory_is_not_a_network_mount() {
        let temp = tempfile::tempdir().expect("tempdir");
        assert!(!is_network_path(temp.path()));
    }
}
