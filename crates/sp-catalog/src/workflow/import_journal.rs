//! Crash-resumable per-photo import publication. Complete verified staging
//! precedes a durable manifest; final names are created atomically without
//! replacement. Replaying a checkpoint accepts only identical final bytes.
use super::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
struct Part {
    name: String,
    hash: String,
    targets: Vec<PathBuf>,
}
#[derive(Debug, Serialize, Deserialize)]
pub(super) struct Manifest {
    version: u32,
    source_root: PathBuf,
    source: PathBuf,
    source_hash: String,
    destination: PathBuf,
    backup_root: Option<PathBuf>,
    parts: Vec<Part>,
}
pub(super) struct Session {
    directory: PathBuf,
    source_root: PathBuf,
    destination_root: PathBuf,
    backup_root: Option<PathBuf>,
    _lock: std::fs::File,
}
fn io(path: &Path, e: std::io::Error) -> SpError {
    SpError::io(path.display().to_string(), e)
}
fn sync_directory(path: &Path) {
    if let Ok(dir) = std::fs::File::open(path) {
        let _ = dir.sync_all();
    }
}

fn publish_new(temporary: &Path, target: &Path) -> Result<(), std::io::Error> {
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::ffi::OsStrExt;
        let source = std::ffi::CString::new(temporary.as_os_str().as_bytes())?;
        let destination = std::ffi::CString::new(target.as_os_str().as_bytes())?;
        // RENAME_EXCL is atomic, supports exFAT as well as APFS, and cannot
        // replace a file created by another application after preflight.
        let result =
            unsafe { libc::renamex_np(source.as_ptr(), destination.as_ptr(), libc::RENAME_EXCL) };
        if result == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    }
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::ffi::OsStrExt;
        let source = std::ffi::CString::new(temporary.as_os_str().as_bytes())?;
        let destination = std::ffi::CString::new(target.as_os_str().as_bytes())?;
        let result = unsafe {
            libc::renameat2(
                libc::AT_FDCWD,
                source.as_ptr(),
                libc::AT_FDCWD,
                destination.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        };
        if result == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    }
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::ffi::OsStrExt;
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn MoveFileExW(source: *const u16, destination: *const u16, flags: u32) -> i32;
        }
        let source: Vec<u16> = temporary.as_os_str().encode_wide().chain(Some(0)).collect();
        let destination: Vec<u16> = target.as_os_str().encode_wide().chain(Some(0)).collect();
        if source[..source.len() - 1].contains(&0)
            || destination[..destination.len() - 1].contains(&0)
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "NUL in import path",
            ));
        }
        // WRITE_THROUGH, deliberately omitting REPLACE_EXISTING.
        if unsafe { MoveFileExW(source.as_ptr(), destination.as_ptr(), 8) } != 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        std::fs::hard_link(temporary, target)
    }
}

impl Session {
    pub(super) fn open(source: &Path, options: &ImportOptions) -> Result<Self, SpError> {
        let destination_root = normalized_source_path(&options.destination);
        let source_root = normalized_source_path(source);
        if destination_root.starts_with(&source_root)
            || options
                .backup
                .as_ref()
                .is_some_and(|p| normalized_source_path(p).starts_with(&source_root))
        {
            return Err(SpError::Catalog(
                "Choose import and backup folders outside the source folder".into(),
            ));
        }
        let directory = destination_root.join(".shopphoto-import");
        std::fs::create_dir_all(&directory).map_err(|e| io(&directory, e))?;
        let lock_path = directory.join("lock");
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .map_err(|e| io(&lock_path, e))?;
        lock.try_lock().map_err(|_| {
            SpError::Catalog(
                "Another import is using this destination; finish it before resuming".into(),
            )
        })?;
        Ok(Self {
            directory,
            source_root,
            destination_root,
            backup_root: options.backup.as_ref().map(|p| normalized_source_path(p)),
            _lock: lock,
        })
    }

    pub(super) fn resume(
        &self,
        selected: Option<&HashSet<PathBuf>>,
    ) -> Result<Vec<(PathBuf, PathBuf, String)>, SpError> {
        let mut resumed = Vec::new();
        for entry in std::fs::read_dir(&self.directory).map_err(|e| io(&self.directory, e))? {
            let path = entry.map_err(|e| io(&self.directory, e))?.path();
            if !path.is_dir() {
                continue;
            }
            let manifest_path = path.join("manifest.json");
            if !manifest_path.exists() {
                continue;
            }
            let file = std::fs::File::open(&manifest_path).map_err(|e| io(&manifest_path, e))?;
            if file.metadata().map_err(|e| io(&manifest_path, e))?.len() > 4 * 1024 * 1024 {
                return Err(SpError::Catalog("Import checkpoint is too large".into()));
            }
            let manifest: Manifest = serde_json::from_reader(file)
                .map_err(|e| SpError::Catalog(format!("Cannot read import checkpoint: {e}")))?;
            if manifest.source_root != self.source_root
                || selected.is_some_and(|items| {
                    !items
                        .iter()
                        .any(|p| normalized_source_path(p) == manifest.source)
                })
            {
                continue;
            }
            self.commit(&path, &manifest)?;
            resumed.push((manifest.source, manifest.destination, manifest.source_hash));
        }
        Ok(resumed)
    }

    pub(super) fn stage(
        &self,
        source: &Path,
        destination: &Path,
        backup: Option<&Path>,
        expected_hash: &str,
        options: &ImportOptions,
    ) -> Result<(PathBuf, Manifest), SpError> {
        let id = format!(
            "{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        );
        let directory = self.directory.join(id);
        std::fs::create_dir(&directory).map_err(|e| io(&directory, e))?;
        let result = (|| {
            let staged = directory.join(destination.file_name().unwrap_or_default());
            if copy_verified(source, &staged)? != expected_hash {
                return Err(SpError::Catalog(
                    "Source changed during import staging".into(),
                ));
            }
            let mut companions = vec![
                (sidecar_path(source), sidecar_path(&staged)),
                (xmp::xmp_path(source), xmp::xmp_path(&staged)),
            ];
            for id in virtual_copies(source)? {
                companions.push((
                    virtual_copy_sidecar_path(source, &id),
                    virtual_copy_sidecar_path(&staged, &id),
                ));
            }
            for (from, to) in companions {
                if from.exists() {
                    copy_verified(&from, &to)?;
                }
            }
            super::apply_import_metadata(&staged, options)?;
            let mut parts = Vec::new();
            for entry in std::fs::read_dir(&directory).map_err(|e| io(&directory, e))? {
                let path = entry.map_err(|e| io(&directory, e))?.path();
                let name = path
                    .file_name()
                    .and_then(|v| v.to_str())
                    .ok_or_else(|| {
                        SpError::Catalog("Import companion has an unsupported filename".into())
                    })?
                    .to_owned();
                let mut targets =
                    vec![
                        normalized_source_path(destination.parent().unwrap_or(Path::new(".")))
                            .join(&name),
                    ];
                if let Some(backup) = backup {
                    targets.push(normalized_source_path(backup).join(&name));
                }
                parts.push(Part {
                    name,
                    hash: content_hash(&path)?,
                    targets,
                });
            }
            // Publish the original last so catalog watchers cannot discover
            // a photo before its primary edit/XMP/version files exist.
            let original = staged.file_name().unwrap_or_default().to_string_lossy();
            parts.sort_by_key(|part| part.name == original);
            let manifest = Manifest {
                version: 1,
                source_root: self.source_root.clone(),
                source: normalized_source_path(source),
                source_hash: expected_hash.into(),
                destination: normalized_source_path(destination),
                backup_root: self.backup_root.clone(),
                parts,
            };
            let bytes = serde_json::to_vec_pretty(&manifest)
                .map_err(|e| SpError::Catalog(e.to_string()))?;
            let temporary = directory.join("manifest.tmp");
            let manifest_path = directory.join("manifest.json");
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)
                .map_err(|e| io(&temporary, e))?;
            file.write_all(&bytes)
                .and_then(|_| file.sync_all())
                .map_err(|e| io(&temporary, e))?;
            std::fs::rename(&temporary, &manifest_path).map_err(|e| io(&manifest_path, e))?;
            sync_directory(&directory);
            sync_directory(&self.directory);
            Ok(manifest)
        })();
        match result {
            Ok(manifest) => Ok((directory, manifest)),
            Err(error) => {
                let _ = std::fs::remove_dir_all(directory);
                Err(error)
            }
        }
    }

    pub(super) fn commit(&self, directory: &Path, manifest: &Manifest) -> Result<(), SpError> {
        if manifest.version != 1
            || manifest.backup_root != self.backup_root
            || !manifest.destination.starts_with(&self.destination_root)
            || manifest.parts.is_empty()
        {
            return Err(SpError::Catalog(
                "Import checkpoint does not match this destination/backup configuration".into(),
            ));
        }
        // Validate every staged/final file before writing any missing pieces.
        for part in &manifest.parts {
            let name = Path::new(&part.name);
            if name.components().count() != 1
                || !matches!(
                    name.components().next(),
                    Some(std::path::Component::Normal(_))
                )
            {
                return Err(SpError::Catalog(
                    "Invalid import checkpoint companion path".into(),
                ));
            }
            let staged = directory.join(name);
            if content_hash(&staged)? != part.hash {
                return Err(SpError::Catalog(format!(
                    "Import staging changed: {}",
                    staged.display()
                )));
            }
            for target in &part.targets {
                let target = normalized_source_path(target);
                if !target.starts_with(&self.destination_root)
                    && !self
                        .backup_root
                        .as_ref()
                        .is_some_and(|root| target.starts_with(root))
                {
                    return Err(SpError::Catalog(
                        "Import checkpoint target is outside the selected folders".into(),
                    ));
                }
                if target.exists() && content_hash(&target)? != part.hash {
                    return Err(SpError::Catalog(format!("Cannot resume: {} changed outside the import. Move that file aside or choose another destination; it was not overwritten.",target.display())));
                }
            }
        }
        for part in &manifest.parts {
            for target in &part.targets {
                if target.exists() {
                    continue;
                }
                let parent = target
                    .parent()
                    .ok_or_else(|| SpError::Catalog("Import target has no parent".into()))?;
                std::fs::create_dir_all(parent).map_err(|e| io(parent, e))?;
                let temporary = parent.join(format!(
                    ".shopphoto-copy-{}-{}",
                    std::process::id(),
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_nanos()
                ));
                copy_verified(&directory.join(&part.name), &temporary)?;
                let result = publish_new(&temporary, target).map_err(|e| io(target, e));
                let _ = std::fs::remove_file(&temporary);
                result?;
                sync_directory(parent);
            }
        }
        // Recheck before forgetting recovery information, including already
        // published pieces that another application could edit during resume.
        for part in &manifest.parts {
            for target in &part.targets {
                if content_hash(target)? != part.hash {
                    return Err(SpError::Catalog(format!(
                        "Imported file changed before verification: {}",
                        target.display()
                    )));
                }
            }
        }
        std::fs::remove_dir_all(directory).map_err(|e| io(directory, e))?;
        sync_directory(&self.directory);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resumes_missing_companions_and_backup_without_replacing_foreign_edits() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("card");
        let destination = temp.path().join("photos");
        let backup = temp.path().join("backup");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::create_dir_all(&destination).unwrap();
        let original = source.join("frame.dng");
        std::fs::write(&original, b"capture").unwrap();
        save_sidecar(&original, &Sidecar::default()).unwrap();
        let options = ImportOptions {
            destination: destination.clone(),
            backup: Some(backup.clone()),
            skip_duplicates: true,
            ..Default::default()
        };
        let session = Session::open(&source, &options).unwrap();
        let target = destination.join("frame.dng");
        let hash = content_hash(&original).unwrap();
        let (stage, _) = session
            .stage(&original, &target, Some(&backup), &hash, &options)
            .unwrap();
        // Simulate a previous process which published only the original.
        copy_verified(&original, &target).unwrap();
        drop(session);
        let report = super::super::import_card(&source, &options, |_, _| true).unwrap();
        assert_eq!(report.resumed, 1);
        assert!(sidecar_path(&target).exists());
        assert!(sidecar_path(&backup.join("frame.dng")).exists());
        assert!(!stage.exists());
        let session = Session::open(&source, &options).unwrap();
        let target2 = destination.join("other.dng");
        let (_stage, _) = session
            .stage(&original, &target2, Some(&backup), &hash, &options)
            .unwrap();
        std::fs::write(&target2, b"external edit").unwrap();
        assert!(session.resume(None).is_err());
        assert_eq!(std::fs::read(&target2).unwrap(), b"external edit");
    }
    #[test]
    fn refuses_simultaneous_import_into_the_same_destination() {
        let temp = tempfile::tempdir().unwrap();
        let options = ImportOptions {
            destination: temp.path().join("photos"),
            ..Default::default()
        };
        let source = temp.path().join("card");
        let _first = Session::open(&source, &options).unwrap();
        assert!(Session::open(&source, &options).is_err());
    }
}
