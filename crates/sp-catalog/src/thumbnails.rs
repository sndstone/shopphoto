//! Content-keyed thumbnail disk cache with an LRU byte budget.
//!
//! Writes are cheap: a temp file and a rename. The budget walk (read the
//! directory, stat every entry, sort by access time) only runs once enough
//! new bytes have landed since the previous walk, so importing thousands of
//! photos costs a handful of walks instead of one per photo. Callers should
//! do all of this from a worker thread; nothing here is safe to run inside a
//! UI frame on a large cache.

use sp_core::SpError;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

pub const THUMB_CACHE_BUDGET_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// New bytes written since the last budget walk before the next one runs.
pub const BUDGET_CHECK_INTERVAL_BYTES: u64 = 64 * 1024 * 1024;

/// Bytes stored since the last budget walk.
static PENDING_BYTES: AtomicU64 = AtomicU64::new(0);

#[must_use]
pub fn cache_path(cache_dir: &Path, source: &Path, modified_s: u64) -> PathBuf {
    let mut hasher = blake3::Hasher::new();
    hasher.update(source.as_os_str().to_string_lossy().as_bytes());
    hasher.update(&modified_s.to_le_bytes());
    cache_dir.join(format!("{}.png", hasher.finalize().to_hex()))
}

/// The cache path for `source` as it is on disk right now. `None` when the
/// file cannot be stat'ed, in which case there is nothing to cache.
#[must_use]
pub fn cache_key(cache_dir: &Path, source: &Path) -> Option<PathBuf> {
    let modified_s = std::fs::metadata(source)
        .ok()?
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    let mut hasher = blake3::Hasher::new();
    hasher.update(
        cache_path(cache_dir, source, modified_s)
            .as_os_str()
            .to_string_lossy()
            .as_bytes(),
    );
    // Edited previews must never survive a sidecar revision. Nanosecond
    // timestamps avoid collisions between quick consecutive slider changes.
    for path in [source.to_path_buf(), crate::sidecar_path(source)] {
        if let Ok(meta) = std::fs::metadata(path) {
            hasher.update(&meta.len().to_le_bytes());
            if let Ok(time) = meta.modified().and_then(|t| {
                t.duration_since(std::time::UNIX_EPOCH)
                    .map_err(std::io::Error::other)
            }) {
                hasher.update(&time.as_nanos().to_le_bytes());
            }
        }
    }
    Some(cache_dir.join(format!("{}.png", hasher.finalize().to_hex())))
}

/// Write one entry atomically and account for its size. Does not walk the
/// cache; call [`enforce_budget_if_due`] afterwards from the same worker.
pub fn store(cache_dir: &Path, key: &Path, png: &[u8]) -> Result<(), SpError> {
    std::fs::create_dir_all(cache_dir)
        .map_err(|source| SpError::io(cache_dir.display().to_string(), source))?;
    let tmp = key.with_extension("png.tmp");
    std::fs::write(&tmp, png).map_err(|source| SpError::io(tmp.display().to_string(), source))?;
    std::fs::rename(&tmp, key).map_err(|source| SpError::io(key.display().to_string(), source))?;
    PENDING_BYTES.fetch_add(png.len() as u64, Ordering::Relaxed);
    Ok(())
}

/// Read a cache entry and refresh its modification time, which is the LRU
/// access clock used by [`enforce_budget`].
pub fn load(key: &Path) -> Result<Vec<u8>, SpError> {
    let bytes =
        std::fs::read(key).map_err(|source| SpError::io(key.display().to_string(), source))?;
    if let Ok(file) = std::fs::OpenOptions::new().write(true).open(key) {
        let _ =
            file.set_times(std::fs::FileTimes::new().set_modified(std::time::SystemTime::now()));
    }
    Ok(bytes)
}

/// Run the budget walk only if at least [`BUDGET_CHECK_INTERVAL_BYTES`] were
/// stored since the previous walk. Returns whether a walk happened.
pub fn enforce_budget_if_due(cache_dir: &Path, budget: u64) -> Result<bool, SpError> {
    if PENDING_BYTES.load(Ordering::Relaxed) < BUDGET_CHECK_INTERVAL_BYTES {
        return Ok(false);
    }
    PENDING_BYTES.store(0, Ordering::Relaxed);
    enforce_budget(cache_dir, budget)?;
    Ok(true)
}

/// Walk the cache and delete least-recently-used entries until it fits.
pub fn enforce_budget(cache_dir: &Path, budget: u64) -> Result<(), SpError> {
    let Ok(entries) = std::fs::read_dir(cache_dir) else {
        return Ok(());
    };
    let mut files: Vec<_> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let meta = entry.metadata().ok()?;
            let modified = meta.modified().ok()?;
            Some((entry.path(), meta.len(), modified))
        })
        .collect();
    let mut total: u64 = files.iter().map(|(_, len, _)| *len).sum();
    files.sort_by_key(|(_, _, modified)| *modified);
    for (path, len, _) in files {
        if total <= budget {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            total = total.saturating_sub(len);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_changes_with_source_and_mtime() {
        let dir = Path::new("/cache");
        let a = cache_path(dir, Path::new("/x/a.3FR"), 1);
        let b = cache_path(dir, Path::new("/x/b.3FR"), 1);
        let c = cache_path(dir, Path::new("/x/a.3FR"), 2);
        assert_ne!(a, b);
        assert_ne!(a, c);
        assert!(a.starts_with(dir));
    }

    #[test]
    fn cache_key_is_none_for_missing_sources() {
        assert!(cache_key(Path::new("/cache"), Path::new("/definitely/not/here.3FR")).is_none());
    }

    #[test]
    fn edit_revision_invalidates_prepared_thumbnail() {
        let temp = tempfile::tempdir().unwrap();
        let raw = temp.path().join("a.raw");
        std::fs::write(&raw, b"raw").unwrap();
        let before = cache_key(temp.path(), &raw).unwrap();
        std::fs::write(crate::sidecar_path(&raw), b"new edit").unwrap();
        let after = cache_key(temp.path(), &raw).unwrap();
        assert_ne!(before, after);
        assert_eq!(after, cache_key(temp.path(), &raw).unwrap());
    }

    #[test]
    fn store_does_not_walk_until_the_interval_is_reached() {
        let temp = tempfile::tempdir().expect("tempdir");
        let dir = temp.path();
        PENDING_BYTES.store(0, Ordering::Relaxed);
        let key = dir.join("one.png");
        store(dir, &key, &[0u8; 1024]).expect("store");
        assert!(!enforce_budget_if_due(dir, 0).expect("check"));
        assert!(key.exists(), "no walk yet, so nothing was evicted");
        PENDING_BYTES.store(BUDGET_CHECK_INTERVAL_BYTES, Ordering::Relaxed);
        assert!(enforce_budget_if_due(dir, 0).expect("check"));
        assert!(!key.exists(), "a zero budget evicts everything on the walk");
        // The tally is process-global and other tests store too, so only
        // check that this walk consumed the interval it was given.
        assert!(PENDING_BYTES.load(Ordering::Relaxed) < BUDGET_CHECK_INTERVAL_BYTES);
    }

    #[test]
    fn budget_walk_evicts_oldest_first() {
        let temp = tempfile::tempdir().expect("tempdir");
        let dir = temp.path();
        let old = dir.join("old.png");
        let new = dir.join("new.png");
        store(dir, &old, &[0u8; 600]).expect("store");
        let past = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
        std::fs::OpenOptions::new()
            .write(true)
            .open(&old)
            .expect("open")
            .set_times(std::fs::FileTimes::new().set_modified(past))
            .expect("set mtime");
        store(dir, &new, &[0u8; 600]).expect("store");
        enforce_budget(dir, 800).expect("enforce");
        assert!(!old.exists());
        assert!(new.exists());
    }
}
