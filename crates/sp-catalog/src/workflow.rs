//! Reusable library workflows: settings sync, persistent snapshots/user
//! presets, safe file moves, duplicate detection, and cancellable card copy.

use crate::{
    load_sidecar, save_sidecar, save_sidecar_path, sidecar_path, virtual_copy_sidecar_path, xmp,
    HistoryEntry, NamedSnapshot, Sidecar,
};
use sp_core::{
    preset::{EditGroups, UserPreset},
    SpError,
};
use std::{
    collections::{HashMap, HashSet},
    io::{Read, Write},
    path::{Path, PathBuf},
};

mod import_journal;

fn now_s() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .min(i64::MAX as u64) as i64
}

/// Resolve aliases in the nearest existing ancestor, even after a photo or
/// its folder was moved. Useful for macOS `/var` versus `/private/var` paths.
#[must_use]
pub fn normalized_source_path(path: &Path) -> PathBuf {
    if let Ok(canonical) = path.canonicalize() {
        return canonical;
    }
    match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) if parent != path => normalized_source_path(parent).join(name),
        _ => path.to_owned(),
    }
}

/// Add or replace a named snapshot in a sidecar.
pub fn save_snapshot(sidecar: &mut Sidecar, name: impl Into<String>) {
    let name = name.into();
    sidecar.snapshots.retain(|item| item.name != name);
    sidecar.snapshots.push(NamedSnapshot {
        name,
        created_s: now_s(),
        edit: sidecar.edit.clone(),
    });
}

/// Enumerate virtual-copy identifiers stored beside an original.
pub fn virtual_copies(original: &Path) -> Result<Vec<String>, SpError> {
    let Some(parent) = original.parent() else {
        return Ok(Vec::new());
    };
    let Some(file_name) = original.file_name().and_then(|name| name.to_str()) else {
        return Ok(Vec::new());
    };
    let prefix = format!("{file_name}.");
    let mut copies = Vec::new();
    let entries = std::fs::read_dir(parent)
        .map_err(|error| SpError::io(parent.display().to_string(), error))?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some(rest) = name.strip_prefix(&prefix) else {
            continue;
        };
        let Some(copy_id) = rest.strip_suffix(".spedit") else {
            continue;
        };
        if !copy_id.is_empty() {
            copies.push(copy_id.to_owned());
        }
    }
    copies.sort();
    copies.dedup();
    Ok(copies)
}

/// Create a uniquely named virtual copy from the supplied edit document.
pub fn create_virtual_copy(original: &Path, source: &Sidecar) -> Result<String, SpError> {
    let base = format!("copy-{}", now_s());
    let mut copy_id = base.clone();
    let mut sequence = 2u32;
    while virtual_copy_sidecar_path(original, &copy_id).exists() {
        copy_id = format!("{base}-{sequence}");
        sequence += 1;
    }
    let mut copy = source.clone();
    copy.virtual_copy = Some(copy_id.clone());
    copy.virtual_copy_name = Some(format!("Copy {}", virtual_copies(original)?.len() + 1));
    save_sidecar_path(&virtual_copy_sidecar_path(original, &copy_id), &copy)?;
    Ok(copy_id)
}

pub fn virtual_copy_records(original: &Path) -> Result<Vec<crate::VirtualCopyRecord>, SpError> {
    virtual_copies(original)?
        .into_iter()
        .map(|id| {
            #[derive(serde::Deserialize)]
            struct Header {
                #[serde(default)]
                virtual_copy_name: Option<String>,
            }
            let path = virtual_copy_sidecar_path(original, &id);
            let bytes = std::fs::read(&path)
                .map_err(|error| SpError::io(path.display().to_string(), error))?;
            // Skip edit/mask payloads rather than decompressing every version to
            // draw its name in the Library.
            let header: Header =
                serde_json::from_slice(&bytes).map_err(|error| SpError::CorruptData {
                    path: path.display().to_string(),
                    reason: error.to_string(),
                })?;
            Ok(crate::VirtualCopyRecord {
                version: 1,
                name: header.virtual_copy_name.unwrap_or_else(|| id.clone()),
                id,
            })
        })
        .collect()
}

pub fn rename_virtual_copy(original: &Path, id: &str, name: &str) -> Result<(), SpError> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 120 || name.chars().any(char::is_control) {
        return Err(SpError::Catalog(
            "Version names must contain 1–120 characters, without control characters".into(),
        ));
    }
    if !virtual_copies(original)?.iter().any(|copy| copy == id) {
        return Err(SpError::Catalog("Virtual copy does not exist".into()));
    }
    let path = virtual_copy_sidecar_path(original, id);
    let mut sidecar = crate::load_sidecar_path(&path)?
        .ok_or_else(|| SpError::Catalog("Virtual copy disappeared".into()))?;
    sidecar.virtual_copy_name = Some(name.to_owned());
    save_sidecar_path(&path, &sidecar)
}

/// Remove only a version's edit sidecar via the OS trash; the photo stays intact.
pub fn trash_virtual_copy(original: &Path, id: &str) -> Result<(), SpError> {
    if !virtual_copies(original)?.iter().any(|copy| copy == id) {
        return Err(SpError::Catalog("Virtual copy does not exist".into()));
    }
    trash::delete(virtual_copy_sidecar_path(original, id))
        .map_err(|error| SpError::Catalog(format!("Could not trash version: {error}")))
}

/// Result of one settings-sync transaction; `undo` restores every previous
/// sidecar (or removes one that did not exist before the sync).
pub struct SettingsBatch {
    before: Vec<(PathBuf, Option<Sidecar>)>,
}

impl SettingsBatch {
    pub fn undo(self) -> Result<(), SpError> {
        for (source, previous) in self.before {
            match previous {
                Some(sidecar) => save_sidecar(&source, &sidecar)?,
                None => match std::fs::remove_file(sidecar_path(&source)) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(SpError::io(source.display().to_string(), error)),
                },
            }
        }
        Ok(())
    }
}

/// Copy selected edit groups to all targets as a recoverable batch.
pub fn sync_settings(
    source: &Sidecar,
    targets: &[PathBuf],
    groups: EditGroups,
) -> Result<SettingsBatch, SpError> {
    // Validate every existing document before changing any of them. A corrupt
    // later target must not leave earlier photos edited without an undo batch.
    let mut seen = HashSet::new();
    let mut prepared = Vec::with_capacity(targets.len());
    for target in targets {
        if !seen.insert(normalized_source_path(target)) {
            continue;
        }
        let previous = load_sidecar(target)?;
        prepared.push((target, previous));
    }
    let mut before = Vec::with_capacity(prepared.len());
    for (target, previous) in prepared {
        let mut next = previous.clone().unwrap_or_else(|| Sidecar {
            version: sp_core::edit::EDIT_SCHEMA_VERSION,
            ..Sidecar::default()
        });
        next.history.push(HistoryEntry {
            label: "Before settings sync".into(),
            created_s: now_s(),
            edit: next.edit.clone(),
        });
        if next.history.len() > 200 {
            next.history.drain(..next.history.len() - 200);
        }
        groups.apply(&source.edit, &mut next.edit);
        if let Err(error) = save_sidecar(target, &next) {
            SettingsBatch { before }.undo()?;
            return Err(error);
        }
        before.push((target.clone(), previous));
    }
    Ok(SettingsBatch { before })
}

pub fn load_user_presets(path: &Path) -> Result<Vec<UserPreset>, SpError> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|error| SpError::CorruptData {
            path: path.display().to_string(),
            reason: error.to_string(),
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(SpError::io(path.display().to_string(), error)),
    }
}

pub fn save_user_presets(path: &Path, presets: &[UserPreset]) -> Result<(), SpError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| SpError::io(parent.display().to_string(), error))?;
    }
    let bytes = serde_json::to_vec_pretty(presets).map_err(|error| SpError::CorruptData {
        path: path.display().to_string(),
        reason: error.to_string(),
    })?;
    crate::atomic_write(path, &bytes)
}

fn companion_paths(path: &Path) -> [(PathBuf, bool); 3] {
    [
        (path.to_path_buf(), true),
        (sidecar_path(path), false),
        (xmp::xmp_path(path), false),
    ]
}

/// Expand the library rename tokens while preserving the original extension.
/// Path separators and control characters are replaced so a pattern cannot
/// escape the chosen destination directory.
#[must_use]
pub fn renamed_file_name(
    source: &Path,
    pattern: &str,
    sequence: usize,
    captured_at: Option<&str>,
) -> String {
    let stem = source
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("image");
    let date = captured_at
        .and_then(|value| value.get(..10))
        .unwrap_or("undated");
    let expanded = pattern
        .replace("{name}", stem)
        .replace("{date}", date)
        .replace("{seq}", &format!("{sequence:04}"));
    let safe: String = expanded
        .chars()
        .map(|character| {
            if character.is_control() || matches!(character, '/' | '\\' | ':') {
                '_'
            } else {
                character
            }
        })
        .collect();
    let safe = safe.trim().trim_matches('.');
    let safe = if safe.is_empty() { "image" } else { safe };
    source
        .extension()
        .and_then(|value| value.to_str())
        .map_or_else(
            || safe.to_owned(),
            |extension| format!("{safe}.{extension}"),
        )
}

/// Recoverable batch of completed move/rename operations.
pub struct FileMoveBatch {
    moved: Vec<(PathBuf, PathBuf)>,
}

impl FileMoveBatch {
    #[must_use]
    pub fn moves(&self) -> &[(PathBuf, PathBuf)] {
        &self.moved
    }

    pub fn undo(&self) -> Result<(), SpError> {
        let mut undone: Vec<(&PathBuf, &PathBuf)> = Vec::new();
        for (source, destination) in self.moved.iter().rev() {
            if let Err(error) = move_photo(destination, source) {
                for (old, new) in undone.into_iter().rev() {
                    move_photo(old, new)?;
                }
                return Err(error);
            }
            undone.push((source, destination));
        }
        Ok(())
    }
}

/// Move or rename several photographs as one rollback-safe batch.
pub fn move_photos(
    sources: &[(PathBuf, Option<String>)],
    destination_directory: &Path,
    pattern: &str,
) -> Result<FileMoveBatch, SpError> {
    let mut batch = FileMoveBatch { moved: Vec::new() };
    for (index, (source, captured_at)) in sources.iter().enumerate() {
        let destination = destination_directory.join(renamed_file_name(
            source,
            pattern,
            index + 1,
            captured_at.as_deref(),
        ));
        if let Err(error) = move_photo(source, &destination) {
            batch.undo()?;
            return Err(error);
        }
        batch.moved.push((source.clone(), destination));
    }
    Ok(batch)
}

/// Rename several photographs in their current directories.
pub fn rename_photos(
    sources: &[(PathBuf, Option<String>)],
    pattern: &str,
) -> Result<FileMoveBatch, SpError> {
    let mut batch = FileMoveBatch { moved: Vec::new() };
    for (index, (source, captured_at)) in sources.iter().enumerate() {
        let parent = source.parent().unwrap_or_else(|| Path::new("."));
        let destination = parent.join(renamed_file_name(
            source,
            pattern,
            index + 1,
            captured_at.as_deref(),
        ));
        if destination == *source {
            continue;
        }
        if let Err(error) = move_photo(source, &destination) {
            batch.undo()?;
            return Err(error);
        }
        batch.moved.push((source.clone(), destination));
    }
    Ok(batch)
}

/// Move an original and its ShopPhoto/XMP sidecars to the operating system's
/// trash or recycle bin. Virtual-copy sidecars are included.
pub fn trash_photo(original: &Path) -> Result<(), SpError> {
    let mut paths: Vec<_> = companion_paths(original)
        .into_iter()
        .map(|(path, _)| path)
        .filter(|path| path.exists())
        .collect();
    if let (Some(parent), Some(file_name)) = (
        original.parent(),
        original.file_name().and_then(|value| value.to_str()),
    ) {
        let prefix = format!("{file_name}.");
        if let Ok(entries) = std::fs::read_dir(parent) {
            paths.extend(entries.flatten().map(|entry| entry.path()).filter(|path| {
                path.file_name()
                    .and_then(|value| value.to_str())
                    .is_some_and(|name| name.starts_with(&prefix) && name.ends_with(".spedit"))
            }));
        }
    }
    if paths.is_empty() {
        return Err(SpError::Catalog(format!(
            "source does not exist: {}",
            original.display()
        )));
    }
    trash::delete_all(paths)
        .map_err(|error| SpError::Catalog(format!("could not move to trash: {error}")))
}

/// Move/rename an original and its known sidecars. Missing companions are
/// ignored; a failed move rolls back companions already moved.
pub fn move_photo(source: &Path, destination: &Path) -> Result<(), SpError> {
    if source == destination {
        return Ok(());
    }
    if destination.exists() {
        return Err(SpError::Catalog(format!(
            "destination already exists: {}",
            destination.display()
        )));
    }
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| SpError::io(parent.display().to_string(), e))?;
    }
    let mut pairs: Vec<_> = companion_paths(source)
        .into_iter()
        .zip(companion_paths(destination))
        .map(|((from, required), (to, _))| (from, to, required))
        .collect();
    for copy in virtual_copies(source)? {
        pairs.push((
            virtual_copy_sidecar_path(source, &copy),
            virtual_copy_sidecar_path(destination, &copy),
            true,
        ));
    }
    // Check every companion before moving the original. A renamed image
    // must never overwrite another image's edits or foreign XMP.
    for (from, to, required) in &pairs {
        if !from
            .try_exists()
            .map_err(|e| SpError::io(from.display().to_string(), e))?
        {
            if *required {
                return Err(SpError::Catalog(format!(
                    "source does not exist: {}",
                    from.display()
                )));
            }
        } else if to
            .try_exists()
            .map_err(|e| SpError::io(to.display().to_string(), e))?
        {
            return Err(SpError::Catalog(format!(
                "destination already exists: {}",
                to.display()
            )));
        }
    }
    let mut moved: Vec<(PathBuf, PathBuf)> = Vec::new();
    for (from, to, required) in pairs {
        if !from.exists() {
            if required {
                return Err(SpError::Catalog(format!(
                    "source does not exist: {}",
                    source.display()
                )));
            }
            continue;
        }
        if let Err(error) = move_without_replace(&from, &to) {
            for (old, new) in moved.into_iter().rev() {
                if let Err(rollback) = move_without_replace(&new, &old) {
                    return Err(SpError::Catalog(format!(
                        "{error}; rollback of {} failed: {rollback}",
                        new.display()
                    )));
                }
            }
            return Err(error);
        }
        moved.push((from, to));
    }
    Ok(())
}

fn move_without_replace(source: &Path, destination: &Path) -> Result<(), SpError> {
    // A hard link is an atomic no-replace operation, unlike rename on Unix.
    // Cross-volume moves and filesystems without hard links use a verified copy.
    if let Err(error) = std::fs::hard_link(source, destination) {
        if error.kind() == std::io::ErrorKind::AlreadyExists {
            return Err(SpError::io(destination.display().to_string(), error));
        }
        copy_verified(source, destination)?;
    }
    if let Err(error) = std::fs::remove_file(source) {
        let _ = std::fs::remove_file(destination);
        return Err(SpError::io(source.display().to_string(), error));
    }
    Ok(())
}

/// Content hash used for duplicate detection and card-import de-duplication.
pub fn content_hash(path: &Path) -> Result<String, SpError> {
    let mut file =
        std::fs::File::open(path).map_err(|e| SpError::io(path.display().to_string(), e))?;
    let mut hash = blake3::Hasher::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|e| SpError::io(path.display().to_string(), e))?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(hash.finalize().to_hex().to_string())
}

/// Copy without overwriting, flush to disk, then verify destination bytes.
/// A failed or changed-source copy is removed; existing destinations are never touched.
pub fn copy_verified(source: &Path, destination: &Path) -> Result<String, SpError> {
    let mut input =
        std::fs::File::open(source).map_err(|e| SpError::io(source.display().to_string(), e))?;
    let before = input
        .metadata()
        .map_err(|e| SpError::io(source.display().to_string(), e))?;
    let mut output = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .map_err(|e| SpError::io(destination.display().to_string(), e))?;
    let result = (|| {
        let mut hash = blake3::Hasher::new();
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let count = input
                .read(&mut buffer)
                .map_err(|e| SpError::io(source.display().to_string(), e))?;
            if count == 0 {
                break;
            }
            hash.update(&buffer[..count]);
            output
                .write_all(&buffer[..count])
                .map_err(|e| SpError::io(destination.display().to_string(), e))?;
        }
        output
            .sync_all()
            .map_err(|e| SpError::io(destination.display().to_string(), e))?;
        let after = input
            .metadata()
            .map_err(|e| SpError::io(source.display().to_string(), e))?;
        let digest = hash.finalize().to_hex().to_string();
        if before.len() != after.len()
            || before.modified().ok() != after.modified().ok()
            || content_hash(destination)? != digest
        {
            return Err(SpError::Catalog(format!(
                "Copy verification failed for {}",
                destination.display()
            )));
        }
        Ok(digest)
    })();
    drop(output);
    if result.is_err() {
        let _ = std::fs::remove_file(destination);
    }
    result
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum AutoStackMode {
    #[default]
    CaptureTime,
    RawJpeg,
}

fn capture_seconds(value: &str) -> Option<(i64, String)> {
    let head = value.get(..19)?;
    let bytes = head.as_bytes();
    if !bytes.iter().enumerate().all(|(i, c)| {
        if [4, 7, 10, 13, 16].contains(&i) {
            matches!(*c, b'-' | b':' | b'T' | b' ')
        } else {
            c.is_ascii_digit()
        }
    }) {
        return None;
    }
    let year = head[0..4].parse::<i64>().ok()?;
    let month = head[5..7].parse::<i64>().ok()?;
    let day = head[8..10].parse::<i64>().ok()?;
    let hour = head[11..13].parse::<i64>().ok()?;
    let minute = head[14..16].parse::<i64>().ok()?;
    let second = head[17..19].parse::<i64>().ok()?;
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let limit = match month {
        2 => {
            if leap {
                29
            } else {
                28
            }
        }
        4 | 6 | 9 | 11 => 30,
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        _ => return None,
    };
    if !(1..=limit).contains(&day) || hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    let y = year - i64::from(month <= 2);
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let days = era * 146097 + yoe * 365 + yoe / 4 - yoe / 100 + doy;
    // Do not compare wall-clock timestamps from different offset contexts.
    let suffix = &value[19..];
    let zone = suffix
        .trim_start_matches(|c: char| c == '.' || c.is_ascii_digit())
        .to_owned();
    Some((days * 86400 + hour * 3600 + minute * 60 + second, zone))
}

/// Suggest groupings only. Applying suggestions is a separate explicit UI action.
/// Capture groups require matching directory, camera, lens and time-zone suffix;
/// each group's total span is bounded, preventing chains across a long session.
pub fn propose_stacks(
    photos: &[crate::CatalogPhoto],
    mode: AutoStackMode,
    max_span_s: u32,
) -> Vec<Vec<PathBuf>> {
    let mut groups = Vec::new();
    match mode {
        AutoStackMode::RawJpeg => {
            let mut names =
                std::collections::BTreeMap::<(PathBuf, String), Vec<&crate::CatalogPhoto>>::new();
            for photo in photos.iter().filter(|photo| photo.stack_id.is_none()) {
                let extension = photo
                    .path
                    .extension()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_ascii_lowercase();
                if sp_core::raw::SUPPORTED_RAW_EXTENSIONS.contains(&extension.as_str())
                    || matches!(extension.as_str(), "jpg" | "jpeg")
                {
                    names
                        .entry((
                            photo.path.parent().unwrap_or(Path::new("")).to_owned(),
                            photo
                                .path
                                .file_stem()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .to_lowercase(),
                        ))
                        .or_default()
                        .push(photo);
                }
            }
            for (_, members) in names {
                let raws = members
                    .iter()
                    .filter(|photo| {
                        photo.path.extension().is_some_and(|ext| {
                            sp_core::raw::SUPPORTED_RAW_EXTENSIONS
                                .contains(&ext.to_string_lossy().to_ascii_lowercase().as_str())
                        })
                    })
                    .count();
                if raws == 1 && members.len() > 1 {
                    groups.push(
                        members
                            .into_iter()
                            .map(|photo| photo.path.clone())
                            .collect(),
                    );
                }
            }
        }
        AutoStackMode::CaptureTime => {
            let mut cameras = std::collections::BTreeMap::<
                (PathBuf, String, String, String),
                Vec<(i64, PathBuf)>,
            >::new();
            for photo in photos.iter().filter(|photo| photo.stack_id.is_none()) {
                if let Some((time, zone)) = photo
                    .captured_at
                    .as_ref()
                    .and_then(|date| capture_seconds(date))
                {
                    cameras
                        .entry((
                            photo.path.parent().unwrap_or(Path::new("")).to_owned(),
                            photo.camera_model.clone(),
                            photo.lens_model.clone(),
                            zone,
                        ))
                        .or_default()
                        .push((time, photo.path.clone()));
                }
            }
            for (_, mut photos) in cameras {
                photos.sort();
                let mut current = Vec::new();
                let mut start = 0;
                for (time, path) in photos {
                    if !current.is_empty() && time - start > i64::from(max_span_s.clamp(1, 60)) {
                        if current.len() > 1 {
                            groups.push(std::mem::take(&mut current));
                        } else {
                            current.clear();
                        }
                    }
                    if current.is_empty() {
                        start = time;
                    }
                    current.push(path);
                }
                if current.len() > 1 {
                    groups.push(current);
                }
            }
        }
    }
    groups
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ImportFolderLayout {
    #[default]
    Flat,
    CaptureDate,
}

#[derive(Debug, Clone, Default)]
pub struct ImportOptions {
    pub destination: PathBuf,
    pub backup: Option<PathBuf>,
    pub filename_pattern: String,
    pub skip_duplicates: bool,
    pub preset: Option<UserPreset>,
    /// None imports every supported image; Some imports exactly this subset.
    pub selection: Option<HashSet<PathBuf>>,
    pub captured_at: HashMap<PathBuf, String>,
    pub folder_layout: ImportFolderLayout,
    /// Nonempty template fields replace corresponding source metadata fields.
    pub metadata: crate::DescriptiveMetadata,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportReport {
    pub copied: Vec<PathBuf>,
    pub skipped_duplicates: usize,
    pub resumed: usize,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MetadataTemplate {
    pub version: u32,
    pub name: String,
    pub metadata: crate::DescriptiveMetadata,
}

pub fn load_metadata_template(path: &Path) -> Result<MetadataTemplate, SpError> {
    let bytes = std::fs::read(path).map_err(|e| SpError::io(path.display().to_string(), e))?;
    let template: MetadataTemplate =
        serde_json::from_slice(&bytes).map_err(|e| SpError::CorruptData {
            path: path.display().to_string(),
            reason: e.to_string(),
        })?;
    if template.version != 1 {
        return Err(SpError::Catalog(format!(
            "Unsupported metadata template version {}",
            template.version
        )));
    }
    Ok(template)
}

pub fn save_metadata_template(
    path: &Path,
    metadata: &crate::DescriptiveMetadata,
) -> Result<(), SpError> {
    let template = MetadataTemplate {
        version: 1,
        name: path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
        metadata: metadata.clone(),
    };
    let bytes =
        serde_json::to_vec_pretty(&template).map_err(|e| SpError::Catalog(e.to_string()))?;
    crate::atomic_write(path, &bytes)
}

/// Plausible removable-media roots for the card-import source picker.
#[must_use]
pub fn detected_import_sources() -> Vec<PathBuf> {
    #[cfg(target_os = "macos")]
    let roots = vec![PathBuf::from("/Volumes")];
    #[cfg(target_os = "linux")]
    let roots = vec![
        PathBuf::from("/media"),
        PathBuf::from("/run/media"),
        PathBuf::from("/mnt"),
    ];
    #[cfg(target_os = "windows")]
    let roots = (b'D'..=b'Z')
        .map(|letter| PathBuf::from(format!("{}:\\", letter as char)))
        .collect::<Vec<_>>();
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    let roots: Vec<PathBuf> = Vec::new();
    let mut sources = Vec::new();
    for root in roots {
        if root.is_dir() {
            #[cfg(target_os = "windows")]
            sources.push(root.clone());
            #[cfg(not(target_os = "windows"))]
            if let Ok(entries) = std::fs::read_dir(&root) {
                sources.extend(entries.flatten().map(|entry| entry.path()).filter(|path| {
                    removable_media_candidate(path)
                        && walkdir::WalkDir::new(path)
                            .max_depth(3)
                            .into_iter()
                            .filter_map(Result::ok)
                            .any(|entry| {
                                entry
                                    .path()
                                    .extension()
                                    .and_then(|value| value.to_str())
                                    .is_some_and(|extension| {
                                        sp_core::raw::SUPPORTED_RAW_EXTENSIONS
                                            .contains(&extension.to_ascii_lowercase().as_str())
                                    })
                            })
                }));
            }
        }
    }
    sources.sort();
    sources.dedup();
    sources
}

/// Whether a mounted volume could be a camera card or local drive worth
/// scanning. The `/Volumes/Macintosh HD` link to the startup disk, the
/// startup disk itself and network shares are skipped: walking them is slow
/// (a few SMB shares took over 30 s) and they are never import cards.
#[cfg(not(target_os = "windows"))]
fn removable_media_candidate(path: &Path) -> bool {
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return false;
    };
    if !metadata.is_dir() {
        return false;
    }
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        use std::os::unix::fs::MetadataExt;
        if std::fs::metadata("/").is_ok_and(|root| root.dev() == metadata.dev()) {
            return false;
        }
        if network_mount(path) {
            return false;
        }
    }
    true
}

#[cfg(target_os = "macos")]
fn network_mount(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(path) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
        return true;
    };
    let mut stats = std::mem::MaybeUninit::<libc::statfs>::uninit();
    // SAFETY: valid NUL-terminated path and a properly sized output buffer.
    if unsafe { libc::statfs(path.as_ptr(), stats.as_mut_ptr()) } != 0 {
        return true;
    }
    // SAFETY: statfs succeeded and initialised the buffer.
    let stats = unsafe { stats.assume_init() };
    stats.f_flags & libc::MNT_LOCAL as u32 == 0
}

#[cfg(target_os = "linux")]
fn network_mount(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(path) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
        return true;
    };
    let mut stats = std::mem::MaybeUninit::<libc::statfs>::uninit();
    // SAFETY: valid NUL-terminated path and a properly sized output buffer.
    if unsafe { libc::statfs(path.as_ptr(), stats.as_mut_ptr()) } != 0 {
        return true;
    }
    // SAFETY: statfs succeeded and initialised the buffer.
    let kind = unsafe { stats.assume_init() }.f_type as i64;
    // NFS, SMB, CIFS, SMB2, AFS, Coda, NCP and FUSE (sshfs and friends).
    [
        0x6969,
        0x517b,
        0xff53_4d42,
        0xfe53_4d42,
        0x5346_414f,
        0x7375_7245,
        0x564c,
        0x6573_5546,
    ]
    .contains(&kind)
}

/// Enumerate supported originals in stable filename order without following links.
pub fn import_candidates(source: &Path) -> Result<Vec<PathBuf>, SpError> {
    let mut files = Vec::new();
    for entry in walkdir::WalkDir::new(source)
        .follow_links(false)
        .into_iter()
    {
        let entry = entry
            .map_err(|error| SpError::Catalog(format!("Cannot list import source: {error}")))?;
        let path = entry.path();
        if entry.file_type().is_file()
            && path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
                let extension = e.to_ascii_lowercase();
                sp_core::raw::SUPPORTED_RAW_EXTENSIONS.contains(&extension.as_str())
                    || sp_core::source::SUPPORTED_RASTER_EXTENSIONS.contains(&extension.as_str())
            })
        {
            files.push(entry.into_path());
        }
    }
    files.sort();
    Ok(files)
}

fn import_subfolder(layout: ImportFolderLayout, date: Option<&str>) -> PathBuf {
    if layout == ImportFolderLayout::Flat {
        return PathBuf::new();
    }
    let date = date.and_then(|value| value.get(..10));
    if let Some(date) = date {
        let bytes = date.as_bytes();
        if bytes.iter().enumerate().all(|(index, byte)| {
            if index == 4 || index == 7 {
                matches!(*byte, b'-' | b':')
            } else {
                byte.is_ascii_digit()
            }
        }) && matches!(date[5..7].parse::<u32>(), Ok(1..=12))
            && matches!(date[8..10].parse::<u32>(), Ok(1..=31))
        {
            return PathBuf::from(&date[..4]).join(date.replace(':', "-"));
        }
    }
    PathBuf::from("Undated")
}

fn apply_import_metadata(destination: &Path, options: &ImportOptions) -> Result<(), SpError> {
    if options.preset.is_some() || options.metadata != crate::DescriptiveMetadata::default() {
        let mut sidecar = match load_sidecar(destination)? {
            Some(sidecar) => sidecar,
            None => {
                let mut sidecar = Sidecar::default();
                if let Some(document) = xmp::load_with_metadata(destination)? {
                    sidecar.metadata = document.metadata;
                    if let Some(annotations) = document.annotations {
                        sidecar.annotations = annotations;
                    }
                }
                sidecar
            }
        };
        if let Some(preset) = &options.preset {
            preset.apply(&mut sidecar.edit);
        }
        let template = &options.metadata;
        for (value, target) in [
            (&template.creator, &mut sidecar.metadata.creator),
            (&template.copyright, &mut sidecar.metadata.copyright),
            (&template.rights, &mut sidecar.metadata.rights),
            (&template.contact, &mut sidecar.metadata.contact),
            (&template.caption, &mut sidecar.metadata.caption),
        ] {
            if !value.trim().is_empty() {
                target.clone_from(value);
            }
        }
        for keyword in &template.keywords {
            if !sidecar.metadata.keywords.contains(keyword) {
                sidecar.metadata.keywords.push(keyword.clone());
            }
        }
        save_sidecar(destination, &sidecar)?;
    }
    Ok(())
}

/// Copy selected originals and sidecars with checksum verification and optional backup.
/// The callback receives `(completed,total)` and returns false to cancel.
pub fn import_card(
    source: &Path,
    options: &ImportOptions,
    mut progress: impl FnMut(usize, usize) -> bool,
) -> Result<ImportReport, SpError> {
    if options.backup.as_ref().is_some_and(|backup| {
        normalized_source_path(backup) == normalized_source_path(&options.destination)
    }) {
        return Err(SpError::Catalog(
            "Choose a backup folder different from the import destination".into(),
        ));
    }
    let files: Vec<_> = import_candidates(source)?
        .into_iter()
        .filter(|path| {
            options
                .selection
                .as_ref()
                .is_none_or(|selected| selected.contains(path))
        })
        .collect();
    std::fs::create_dir_all(&options.destination)
        .map_err(|e| SpError::io(options.destination.display().to_string(), e))?;
    if let Some(backup) = &options.backup {
        std::fs::create_dir_all(backup)
            .map_err(|e| SpError::io(backup.display().to_string(), e))?;
    }
    let session = import_journal::Session::open(source, options)?;
    let resumed = session.resume(options.selection.as_ref())?;
    let mut report = ImportReport {
        copied: resumed
            .iter()
            .map(|(_, destination, _)| destination.clone())
            .collect(),
        resumed: resumed.len(),
        ..ImportReport::default()
    };
    let mut known = HashSet::new();
    if options.skip_duplicates {
        for entry in walkdir::WalkDir::new(&options.destination)
            .into_iter()
            .filter_entry(|entry| entry.file_name() != ".shopphoto-import")
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_file())
        {
            if let Ok(hash) = content_hash(entry.path()) {
                known.insert(hash);
            }
        }
    }
    for (index, source_file) in files.iter().enumerate() {
        if !progress(index, files.len()) {
            return Err(SpError::Catalog("card import cancelled".into()));
        }
        let hash = content_hash(source_file)?;
        if resumed.iter().any(|(path, _, digest)| {
            *path == normalized_source_path(source_file) && *digest == hash
        }) {
            continue;
        }
        if options.skip_duplicates && !known.insert(hash.clone()) {
            report.skipped_duplicates += 1;
            continue;
        }
        let captured_at = options.captured_at.get(source_file).map(String::as_str);
        let subfolder = import_subfolder(options.folder_layout, captured_at);
        let destination_folder = options.destination.join(&subfolder);
        let backup_folder = options.backup.as_ref().map(|root| root.join(&subfolder));
        std::fs::create_dir_all(&destination_folder)
            .map_err(|e| SpError::io(destination_folder.display().to_string(), e))?;
        if let Some(backup) = &backup_folder {
            std::fs::create_dir_all(backup)
                .map_err(|e| SpError::io(backup.display().to_string(), e))?;
        }
        let filename = renamed_file_name(
            source_file,
            &options.filename_pattern,
            index + 1,
            captured_at,
        );
        let name = Path::new(&filename)
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy();
        let ext = source_file
            .extension()
            .unwrap_or_default()
            .to_string_lossy();
        let mut destination = destination_folder.join(&filename);
        let copies = virtual_copies(source_file)?;
        let occupied = |candidate: &Path| {
            companion_paths(candidate)
                .iter()
                .any(|(path, _)| path.exists())
                || copies
                    .iter()
                    .any(|id| virtual_copy_sidecar_path(candidate, id).exists())
        };
        let mut collision = 2u32;
        while occupied(&destination)
            || backup_folder.as_ref().is_some_and(|backup| {
                occupied(&backup.join(destination.file_name().unwrap_or_default()))
            })
        {
            destination = destination_folder.join(format!("{name}-{collision}.{ext}"));
            collision += 1;
        }
        let (staged, manifest) = session.stage(
            source_file,
            &destination,
            backup_folder.as_deref(),
            &hash,
            options,
        )?;
        session.commit(&staged, &manifest)?;
        report.copied.push(destination);
    }
    let _ = progress(files.len(), files.len());
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn automatic_groups_are_bounded_and_raw_jpeg_pairs_require_one_raw() {
        let dir = tempfile::tempdir().unwrap();
        for name in [
            "one.dng",
            "one.JPG",
            "two.dng",
            "three.dng",
            "ambiguous.dng",
            "ambiguous.nef",
            "ambiguous.jpg",
        ] {
            std::fs::write(dir.path().join(name), name).unwrap();
        }
        let mut db = crate::CatalogDb::open_memory().unwrap();
        db.scan_folder(dir.path(), |_| {}).unwrap();
        let mut photos = db
            .photos(&crate::LibraryFilter::default(), crate::LibrarySort::Name)
            .unwrap();
        let pairs = propose_stacks(&photos, AutoStackMode::RawJpeg, 2);
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].len(), 2);
        for photo in &mut photos {
            photo.camera_model = "Camera A".into();
            photo.lens_model = "Lens A".into();
            photo.captured_at = match photo.path.file_stem().unwrap().to_str().unwrap() {
                "one" => Some("2026-09-29T23:59:59+02:00".into()),
                "two" => Some("2026-09-30T00:00:01+02:00".into()),
                "three" => Some("2026-09-30T00:00:03+02:00".into()),
                _ => None,
            };
        }
        let groups = propose_stacks(&photos, AutoStackMode::CaptureTime, 2);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].len(), 3);
        assert!(capture_seconds("2026-02-30T12:00:00").is_none());
        assert!(capture_seconds("../unsafe").is_none());
        let mut other = photos.clone();
        for photo in &mut other {
            photo.stack_id = Some(1);
        }
        assert!(propose_stacks(&other, AutoStackMode::RawJpeg, 2).is_empty());
    }

    #[test]
    fn named_versions_keep_identity_and_edits_after_rename_and_move() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("frame.dng");
        std::fs::write(&source, b"raw").unwrap();
        let mut sidecar = Sidecar::default();
        sidecar.edit.tone.exposure_ev = 1.25;
        let id = create_virtual_copy(&source, &sidecar).unwrap();
        rename_virtual_copy(&source, &id, "Warm portrait").unwrap();
        assert!(rename_virtual_copy(&source, "../unknown", "Bad").is_err());
        assert!(rename_virtual_copy(&source, &id, "\n").is_err());
        let destination = dir.path().join("renamed.dng");
        move_photo(&source, &destination).unwrap();
        let records = virtual_copy_records(&destination).unwrap();
        assert_eq!(records[0].id, id);
        assert_eq!(records[0].name, "Warm portrait");
        let saved = crate::load_sidecar_path(&virtual_copy_sidecar_path(&destination, &id))
            .unwrap()
            .unwrap();
        assert_eq!(saved.edit.tone.exposure_ev, 1.25);
        assert_eq!(saved.virtual_copy_name.as_deref(), Some("Warm portrait"));
    }

    #[test]
    fn import_subset_dates_templates_and_sidecars_preserve_source_content() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("card");
        let destination = dir.path().join("photos");
        let backup = dir.path().join("backup");
        std::fs::create_dir(&source).unwrap();
        let selected = source.join("selected.png");
        std::fs::write(&selected, b"selected pixels").unwrap();
        std::fs::write(source.join("excluded.dng"), b"excluded pixels").unwrap();
        let mut original = Sidecar::default();
        original.metadata.caption = "Original caption".into();
        original.metadata.keywords = vec!["travel".into()];
        original.edit.tone.exposure_ev = 0.75;
        save_sidecar(&selected, &original).unwrap();
        let version = create_virtual_copy(&selected, &original).unwrap();
        xmp::save_with_metadata(&selected, original.annotations, &original.metadata).unwrap();
        let original_xmp = std::fs::read(xmp::xmp_path(&selected)).unwrap();
        let template = crate::DescriptiveMetadata {
            creator: "Test photographer".into(),
            keywords: vec!["travel".into(), "portfolio".into()],
            ..Default::default()
        };
        let template_path = dir.path().join("Studio.json");
        save_metadata_template(&template_path, &template).unwrap();
        let template = load_metadata_template(&template_path).unwrap();
        assert_eq!(template.name, "Studio");
        let options = ImportOptions {
            destination: destination.clone(),
            backup: Some(backup.clone()),
            filename_pattern: "{date}-{name}".into(),
            selection: Some(HashSet::from([selected.clone()])),
            captured_at: HashMap::from([(selected.clone(), "2026:09:29 12:15:00".into())]),
            folder_layout: ImportFolderLayout::CaptureDate,
            metadata: template.metadata,
            ..Default::default()
        };
        let report = import_card(&source, &options, |_, _| true).unwrap();
        assert_eq!(report.copied.len(), 1);
        let imported = &report.copied[0];
        assert_eq!(
            imported.parent().unwrap(),
            destination.join("2026/2026-09-29")
        );
        assert!(virtual_copy_sidecar_path(imported, &version).exists());
        assert_eq!(
            std::fs::read(xmp::xmp_path(imported)).unwrap(),
            original_xmp
        );
        let edited = load_sidecar(imported).unwrap().unwrap();
        assert_eq!(edited.metadata.caption, "Original caption");
        assert_eq!(edited.metadata.creator, "Test photographer");
        assert_eq!(edited.metadata.keywords, ["travel", "portfolio"]);
        assert_eq!(edited.edit.tone.exposure_ev, 0.75);
        assert_eq!(load_sidecar(&selected).unwrap().unwrap(), original);
        let mirrored = backup.join(imported.strip_prefix(&destination).unwrap());
        assert_eq!(std::fs::read(&mirrored).unwrap(), b"selected pixels");
        assert_eq!(load_sidecar(&mirrored).unwrap().unwrap(), edited);
        assert!(virtual_copy_sidecar_path(&mirrored, &version).exists());
        assert_eq!(
            import_subfolder(ImportFolderLayout::CaptureDate, Some("../../escape")),
            PathBuf::from("Undated")
        );
        assert!(import_card(
            &source,
            &ImportOptions {
                destination: destination.clone(),
                backup: Some(destination),
                ..Default::default()
            },
            |_, _| true
        )
        .is_err());
    }

    #[test]
    fn moving_photo_moves_virtual_copies_and_never_overwrites_a_companion() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("original.dng");
        let destination = dir.path().join("renamed.dng");
        std::fs::write(&source, b"raw").unwrap();
        let copy = create_virtual_copy(&source, &Sidecar::default()).unwrap();
        let copy_source = virtual_copy_sidecar_path(&source, &copy);
        let copy_destination = virtual_copy_sidecar_path(&destination, &copy);
        std::fs::write(&copy_destination, b"existing edit").unwrap();
        assert!(move_photo(&source, &destination).is_err());
        assert!(source.exists());
        assert!(copy_source.exists());
        assert_eq!(std::fs::read(&copy_destination).unwrap(), b"existing edit");
        std::fs::remove_file(&copy_destination).unwrap();
        move_photo(&source, &destination).unwrap();
        assert!(!source.exists());
        assert!(!copy_source.exists());
        assert!(copy_destination.exists());
        move_photo(&destination, &source).unwrap();
        assert!(copy_source.exists());
    }

    #[test]
    fn verified_copy_preserves_destination_on_collision_and_matches_source() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("large.raw");
        let destination = dir.path().join("copy.raw");
        let bytes: Vec<u8> = (0..160_001).map(|v| (v % 251) as u8).collect();
        std::fs::write(&source, &bytes).unwrap();
        assert_eq!(
            copy_verified(&source, &destination).unwrap(),
            blake3::hash(&bytes).to_hex().to_string()
        );
        std::fs::write(&source, b"replacement").unwrap();
        assert!(copy_verified(&source, &destination).is_err());
        assert_eq!(std::fs::read(&destination).unwrap(), bytes);
    }

    #[test]
    fn import_avoids_backup_collisions_and_sanitizes_names() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("card");
        let destination = dir.path().join("photos");
        let backup = dir.path().join("backup");
        std::fs::create_dir(&source).unwrap();
        std::fs::create_dir(&backup).unwrap();
        std::fs::write(source.join("one.dng"), b"new raw").unwrap();
        std::fs::write(backup.join("one.dng"), b"old backup").unwrap();
        let mut options = ImportOptions {
            destination,
            backup: Some(backup.clone()),
            filename_pattern: "{name}".into(),
            skip_duplicates: false,
            preset: None,
            ..ImportOptions::default()
        };
        let report = import_card(&source, &options, |_, _| true).unwrap();
        assert_eq!(report.copied[0].file_name().unwrap(), "one-2.dng");
        assert_eq!(
            std::fs::read(backup.join("one.dng")).unwrap(),
            b"old backup"
        );
        assert_eq!(std::fs::read(backup.join("one-2.dng")).unwrap(), b"new raw");
        options.filename_pattern = "../../{name}".into();
        let report = import_card(&source, &options, |_, _| true).unwrap();
        assert_eq!(report.copied[0].parent().unwrap(), options.destination);
    }

    #[test]
    fn settings_sync_is_undoable() {
        let dir = tempfile::tempdir().expect("dir");
        let target = dir.path().join("target.dng");
        std::fs::write(&target, b"raw").expect("raw");
        let mut original = Sidecar::default();
        original.edit.geometry.flip_horizontal = true;
        save_sidecar(&target, &original).expect("sidecar");
        let mut source = Sidecar::default();
        source.edit.tone.exposure_ev = 2.0;
        let batch = sync_settings(
            &source,
            std::slice::from_ref(&target),
            EditGroups::default(),
        )
        .expect("sync");
        let synced = load_sidecar(&target).expect("load").expect("sidecar");
        assert_eq!(synced.edit.tone.exposure_ev, 2.0);
        assert!(synced.edit.geometry.flip_horizontal);
        batch.undo().expect("undo");
        assert_eq!(load_sidecar(&target).unwrap().unwrap(), original);
    }

    #[test]
    fn settings_sync_preflights_later_corrupt_targets() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("first.dng");
        let corrupt = dir.path().join("corrupt.dng");
        let mut original = Sidecar::default();
        original.edit.tone.exposure_ev = 0.5;
        save_sidecar(&first, &original).unwrap();
        let original_bytes = std::fs::read(sidecar_path(&first)).unwrap();
        std::fs::write(sidecar_path(&corrupt), b"broken json").unwrap();
        let mut source = Sidecar::default();
        source.edit.tone.exposure_ev = 2.0;

        assert!(sync_settings(
            &source,
            &[first.clone(), corrupt.clone()],
            EditGroups::default()
        )
        .is_err());
        assert_eq!(std::fs::read(sidecar_path(&first)).unwrap(), original_bytes);
        assert_eq!(
            std::fs::read(sidecar_path(&corrupt)).unwrap(),
            b"broken json"
        );
    }

    #[test]
    fn settings_sync_duplicate_paths_are_applied_once_and_undo_to_original() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target.dng");
        let mut source = Sidecar::default();
        source.edit.tone.exposure_ev = 2.0;
        let batch = sync_settings(
            &source,
            &[target.clone(), dir.path().join("./target.dng")],
            EditGroups::default(),
        )
        .unwrap();
        assert_eq!(load_sidecar(&target).unwrap().unwrap().history.len(), 1);
        batch.undo().unwrap();
        assert!(load_sidecar(&target).unwrap().is_none());
    }

    #[test]
    fn settings_sync_rolls_back_earlier_targets_on_publication_failure() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("first.dng");
        let second = dir.path().join("second.dng");
        let missing_parent = dir.path().join("missing/third.dng");
        let mut original = Sidecar::default();
        original.edit.tone.exposure_ev = 0.5;
        save_sidecar(&first, &original).unwrap();
        let mut source = Sidecar::default();
        source.edit.tone.exposure_ev = 2.0;
        assert!(sync_settings(
            &source,
            &[first.clone(), second.clone(), missing_parent],
            EditGroups::default(),
        )
        .is_err());
        assert_eq!(load_sidecar(&first).unwrap().unwrap(), original);
        assert!(load_sidecar(&second).unwrap().is_none());
    }

    #[test]
    fn import_copies_backup_and_skips_duplicate_content() {
        let dir = tempfile::tempdir().expect("dir");
        let card = dir.path().join("card");
        let destination = dir.path().join("photos");
        let backup = dir.path().join("backup");
        std::fs::create_dir_all(&card).unwrap();
        std::fs::write(card.join("one.dng"), b"same").unwrap();
        std::fs::write(card.join("two.dng"), b"same").unwrap();
        let report = import_card(
            &card,
            &ImportOptions {
                destination: destination.clone(),
                backup: Some(backup.clone()),
                filename_pattern: "{name}-{seq}".into(),
                skip_duplicates: true,
                preset: None,
                ..ImportOptions::default()
            },
            |_, _| true,
        )
        .expect("import");
        assert_eq!(report.copied.len(), 1);
        assert_eq!(report.skipped_duplicates, 1);
        assert!(backup.join(report.copied[0].file_name().unwrap()).exists());
    }

    #[test]
    fn import_applies_a_selected_user_preset_to_new_sidecars() {
        let dir = tempfile::tempdir().unwrap();
        let card = dir.path().join("card");
        let destination = dir.path().join("photos");
        std::fs::create_dir_all(&card).unwrap();
        std::fs::write(card.join("one.dng"), b"raw").unwrap();
        let mut preset_edit = sp_core::EditState::default();
        preset_edit.tone.exposure_ev = 1.25;
        let preset = UserPreset::from_edit("Bright", EditGroups::default(), &preset_edit);
        let report = import_card(
            &card,
            &ImportOptions {
                destination,
                backup: None,
                filename_pattern: "{name}".into(),
                skip_duplicates: false,
                preset: Some(preset),
                ..ImportOptions::default()
            },
            |_, _| true,
        )
        .unwrap();
        let sidecar = load_sidecar(&report.copied[0]).unwrap().unwrap();
        assert_eq!(sidecar.edit.tone.exposure_ev, 1.25);
    }

    #[test]
    fn token_rename_moves_companions_and_is_undoable() {
        let dir = tempfile::tempdir().unwrap();
        let source_dir = dir.path().join("source");
        let destination_dir = dir.path().join("destination");
        std::fs::create_dir_all(&source_dir).unwrap();
        let source = source_dir.join("IMG_001.3FR");
        std::fs::write(&source, b"raw").unwrap();
        save_sidecar(&source, &Sidecar::default()).unwrap();
        let batch = move_photos(
            &[(source.clone(), Some("2026-09-03T12:00:00".into()))],
            &destination_dir,
            "{date}-{name}-{seq}",
        )
        .unwrap();
        let moved = destination_dir.join("2026-09-03-IMG_001-0001.3FR");
        assert!(moved.exists());
        assert!(sidecar_path(&moved).exists());
        batch.undo().unwrap();
        assert!(source.exists());
        assert!(sidecar_path(&source).exists());
    }

    #[test]
    fn rename_tokens_cannot_escape_destination() {
        let name = renamed_file_name(Path::new("IMG_001.dng"), "../{name}:{seq}", 7, None);
        assert_eq!(name, "_IMG_001_0007.dng");
        assert!(!name.contains('/'));
    }

    #[test]
    fn virtual_copies_are_unique_and_discoverable() {
        let dir = tempfile::tempdir().unwrap();
        let original = dir.path().join("frame.3FR");
        std::fs::write(&original, b"raw").unwrap();
        let source = Sidecar {
            version: sp_core::edit::EDIT_SCHEMA_VERSION,
            ..Sidecar::default()
        };
        let first = create_virtual_copy(&original, &source).unwrap();
        let second = create_virtual_copy(&original, &source).unwrap();
        assert_ne!(first, second);
        assert_eq!(virtual_copies(&original).unwrap(), vec![first, second]);
    }
}
