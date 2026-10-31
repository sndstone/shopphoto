//! Portable, verified photo packages. Every referenced file is relative to
//! the package; opening a moved package rebuilds local catalog relationships.

use crate::{workflow, CatalogDb, LibraryFilter, LibrarySort};
use serde::{Deserialize, Serialize};
use sp_core::SpError;
use std::io::Read;
use std::path::{Path, PathBuf};

#[derive(Debug, Serialize, Deserialize)]
pub struct ProjectManifest {
    pub version: u32,
    pub name: String,
    pub files: Vec<ProjectFile>,
    pub photos: Vec<ProjectPhoto>,
    pub collections: Vec<ProjectCollection>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ProjectFile {
    pub version: u32,
    pub path: PathBuf,
    pub hash: String,
    #[serde(default)]
    pub editable: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ProjectPhoto {
    pub version: u32,
    pub path: PathBuf,
    pub stack: Option<i64>,
    pub stack_cover: bool,
    pub companion: Option<usize>,
    pub derived_sources: Vec<usize>,
    pub operation: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ProjectCollection {
    pub version: u32,
    pub id: i64,
    pub name: String,
    pub parent: Option<i64>,
    pub members: Vec<usize>,
}

/// `sources` pairs logical catalog paths with the authoritative local original
/// and sidecar location (which may be a pinned offline copy).
pub fn create(
    db: &CatalogDb,
    sources: &[(PathBuf, PathBuf)],
    destination: &Path,
    mut progress: impl FnMut(usize, usize),
) -> Result<PathBuf, SpError> {
    if sources.is_empty() {
        return Err(SpError::Catalog("Select photos to package first".into()));
    }
    std::fs::create_dir(destination)
        .map_err(|e| SpError::io(destination.display().to_string(), e))?;
    let result = (|| {
        let catalog = db.photos(&LibraryFilter::default(), LibrarySort::Name)?;
        let lookup = |path: &Path| {
            sources.iter().position(|(logical, _)| {
                workflow::normalized_source_path(logical) == workflow::normalized_source_path(path)
            })
        };
        let mut manifest = ProjectManifest {
            version: 1,
            name: destination
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            files: Vec::new(),
            photos: Vec::new(),
            collections: Vec::new(),
        };
        for (index, (logical, read_from)) in sources.iter().enumerate() {
            progress(index, sources.len());
            let relative = PathBuf::from("originals")
                .join(format!("{:04}", index + 1))
                .join(logical.file_name().unwrap_or_default());
            let original = destination.join(&relative);
            std::fs::create_dir_all(original.parent().unwrap_or(destination))
                .map_err(|e| SpError::io(original.display().to_string(), e))?;
            let mut pairs = vec![
                (read_from.clone(), original.clone()),
                (
                    crate::sidecar_path(read_from),
                    crate::sidecar_path(&original),
                ),
                (
                    crate::xmp::xmp_path(read_from),
                    crate::xmp::xmp_path(&original),
                ),
            ];
            for version in workflow::virtual_copies(read_from)? {
                pairs.push((
                    crate::virtual_copy_sidecar_path(read_from, &version),
                    crate::virtual_copy_sidecar_path(&original, &version),
                ));
            }
            for (from, to) in pairs {
                if from == *read_from || from.exists() {
                    let hash = workflow::copy_verified(&from, &to)?;
                    manifest.files.push(ProjectFile {
                        version: 1,
                        path: to
                            .strip_prefix(destination)
                            .map_err(|e| SpError::Catalog(e.to_string()))?
                            .to_owned(),
                        hash,
                        editable: to != original,
                    });
                }
            }
            let photo = catalog.iter().find(|photo| {
                workflow::normalized_source_path(&photo.path)
                    == workflow::normalized_source_path(logical)
            });
            manifest.photos.push(ProjectPhoto {
                version: 1,
                path: relative,
                stack: photo.and_then(|p| p.stack_id),
                stack_cover: photo.is_some_and(|p| p.stack_cover),
                companion: photo
                    .and_then(|p| p.companion_of.as_ref())
                    .and_then(|source| lookup(source)),
                derived_sources: photo.map_or_else(Vec::new, |p| {
                    p.derived_sources
                        .iter()
                        .filter_map(|source| lookup(source))
                        .collect()
                }),
                operation: photo.and_then(|p| p.derived_operation.clone()),
            });
        }
        for collection in db.collections()? {
            let members = catalog
                .iter()
                .filter(|photo| photo.collection_ids.contains(&collection.id))
                .filter_map(|photo| lookup(&photo.path))
                .collect();
            manifest.collections.push(ProjectCollection {
                version: 1,
                id: collection.id,
                name: collection.name,
                parent: collection.parent_id,
                members,
            });
        }
        let mut needed: std::collections::HashSet<_> = manifest
            .collections
            .iter()
            .filter(|collection| !collection.members.is_empty())
            .map(|collection| collection.id)
            .collect();
        loop {
            let before = needed.len();
            for collection in &manifest.collections {
                if needed.contains(&collection.id) {
                    if let Some(parent) = collection.parent {
                        needed.insert(parent);
                    }
                }
            }
            if before == needed.len() {
                break;
            }
        }
        manifest
            .collections
            .retain(|collection| needed.contains(&collection.id));
        let path = destination.join("project.spproject");
        std::fs::write(
            &path,
            serde_json::to_vec_pretty(&manifest).map_err(|e| SpError::Catalog(e.to_string()))?,
        )
        .map_err(|e| SpError::io(path.display().to_string(), e))?;
        progress(sources.len(), sources.len());
        Ok(path)
    })();
    // Only this operation created the directory; failed packages can be
    // removed without touching pre-existing files or originals.
    if result.is_err() {
        let _ = std::fs::remove_dir_all(destination);
    }
    result
}

fn inside(root: &Path, relative: &Path) -> Result<PathBuf, SpError> {
    if relative.as_os_str().is_empty()
        || relative
            .components()
            .any(|part| !matches!(part, std::path::Component::Normal(_)))
    {
        return Err(SpError::Catalog(
            "Project contains an invalid relative path".into(),
        ));
    }
    let path = root
        .join(relative)
        .canonicalize()
        .map_err(|e| SpError::io(relative.display().to_string(), e))?;
    if !path.starts_with(root) {
        return Err(SpError::Catalog(
            "Project reference escapes its folder".into(),
        ));
    }
    Ok(path)
}

/// Verify every file before importing package rows into the current catalog.
/// Originals remain inside the chosen package folder; no extra copy is made.
pub fn open(db: &mut CatalogDb, manifest_path: &Path) -> Result<Vec<PathBuf>, SpError> {
    let mut bytes = Vec::new();
    std::fs::File::open(manifest_path)
        .map_err(|e| SpError::io(manifest_path.display().to_string(), e))?
        .take(16 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| SpError::io(manifest_path.display().to_string(), e))?;
    if bytes.len() > 16 * 1024 * 1024 {
        return Err(SpError::Catalog("Project manifest exceeds 16 MiB".into()));
    }
    let manifest: ProjectManifest =
        serde_json::from_slice(&bytes).map_err(|e| SpError::CorruptData {
            path: manifest_path.display().to_string(),
            reason: e.to_string(),
        })?;
    if manifest.version != 1
        || manifest.files.iter().any(|file| file.version != 1)
        || manifest.photos.iter().any(|photo| photo.version != 1)
        || manifest
            .collections
            .iter()
            .any(|collection| collection.version != 1)
    {
        return Err(SpError::Catalog(
            "Unsupported portable project version".into(),
        ));
    }
    if manifest.name.trim().is_empty()
        || manifest.name.chars().count() > 160
        || manifest.photos.is_empty()
    {
        return Err(SpError::Catalog(
            "Project needs a valid name and at least one photograph".into(),
        ));
    }
    let collection_ids: std::collections::HashSet<_> = manifest
        .collections
        .iter()
        .map(|collection| collection.id)
        .collect();
    if collection_ids.len() != manifest.collections.len() {
        return Err(SpError::Catalog(
            "Duplicate project collection identifiers".into(),
        ));
    }
    for collection in &manifest.collections {
        let mut seen = std::collections::HashSet::from([collection.id]);
        let mut parent = collection.parent;
        while let Some(id) = parent {
            if !seen.insert(id) {
                return Err(SpError::Catalog(
                    "Project collection hierarchy contains a cycle".into(),
                ));
            }
            let ancestor = manifest
                .collections
                .iter()
                .find(|collection| collection.id == id)
                .ok_or_else(|| {
                    SpError::Catalog("Project collection has a missing parent".into())
                })?;
            parent = ancestor.parent;
        }
    }
    let root = manifest_path
        .parent()
        .unwrap_or(Path::new("."))
        .canonicalize()
        .map_err(|e| SpError::io(manifest_path.display().to_string(), e))?;
    let mut verified = std::collections::HashSet::new();
    for file in &manifest.files {
        let path = inside(&root, &file.path)?;
        if !file.editable {
            if workflow::content_hash(&path)? != file.hash {
                return Err(SpError::Catalog(format!(
                    "Project checksum mismatch: {}",
                    file.path.display()
                )));
            }
            verified.insert(path);
        }
    }
    let paths = manifest
        .photos
        .iter()
        .map(|photo| inside(&root, &photo.path))
        .collect::<Result<Vec<_>, _>>()?;
    if paths.iter().collect::<std::collections::HashSet<_>>().len() != paths.len()
        || paths.iter().any(|path| !verified.contains(path))
        || manifest.photos.iter().any(|photo| {
            photo.companion.is_some_and(|index| index >= paths.len())
                || photo
                    .derived_sources
                    .iter()
                    .any(|index| *index >= paths.len())
        })
        || manifest
            .collections
            .iter()
            .any(|collection| collection.members.iter().any(|index| *index >= paths.len()))
    {
        return Err(SpError::Catalog(
            "Project contains an invalid photo reference".into(),
        ));
    }
    for (index, photo) in manifest.photos.iter().enumerate() {
        if photo.companion == Some(index)
            || photo.derived_sources.contains(&index)
            || photo
                .operation
                .as_ref()
                .is_some_and(|name| name.trim().is_empty() || name.chars().count() > 120)
        {
            return Err(SpError::Catalog(
                "Project contains invalid derivative provenance".into(),
            ));
        }
    }
    db.scan_folder(&root.join("originals"), |_| {})?;
    let package = db.create_collection(&manifest.name)?;
    for path in &paths {
        db.add_to_collection(package, path)?;
    }
    let mut collections = std::collections::HashMap::new();
    let mut pending: Vec<_> = manifest.collections.iter().collect();
    while !pending.is_empty() {
        let before = pending.len();
        let mut next = Vec::new();
        for collection in pending {
            let parent = match collection.parent {
                None => Some(package),
                Some(id) => collections.get(&id).copied(),
            };
            let Some(parent) = parent else {
                next.push(collection);
                continue;
            };
            let name = format!(
                "{} / {}: {}",
                manifest.name.chars().take(70).collect::<String>(),
                collection.id,
                collection.name
            )
            .chars()
            .take(160)
            .collect::<String>();
            let id = db.create_collection_in(&name, Some(parent))?;
            collections.insert(collection.id, id);
            for member in &collection.members {
                db.add_to_collection(id, &paths[*member])?;
            }
        }
        if next.len() == before {
            return Err(SpError::Catalog(
                "Project collection hierarchy contains a cycle or missing parent".into(),
            ));
        }
        pending = next;
    }
    let current = db.photos(&LibraryFilter::default(), LibrarySort::Name)?;
    let mut stacks = std::collections::BTreeMap::<i64, Vec<usize>>::new();
    for (index, photo) in manifest.photos.iter().enumerate() {
        if let Some(source) = photo.companion {
            db.add_companion(&paths[source], &paths[index])?;
        }
        if !photo.derived_sources.is_empty() {
            db.link_derived_sources(
                &paths[index],
                &photo
                    .derived_sources
                    .iter()
                    .map(|source| paths[*source].clone())
                    .collect::<Vec<_>>(),
                photo.operation.as_deref().unwrap_or("Merge"),
            )?;
        }
        if let Some(stack) = photo.stack {
            stacks.entry(stack).or_default().push(index);
        }
    }
    for (_, mut members) in stacks {
        if members.len() < 2
            || members.iter().any(|index| {
                current
                    .iter()
                    .any(|photo| photo.path == paths[*index] && photo.stack_id.is_some())
            })
        {
            continue;
        }
        members.sort_by_key(|index| !manifest.photos[*index].stack_cover);
        db.stack_photos(
            &members
                .iter()
                .map(|index| paths[*index].clone())
                .collect::<Vec<_>>(),
        )?;
    }
    Ok(paths)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn moved_project_restores_edits_versions_stacks_and_refuses_tampering() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("photo.dng");
        let other = dir.path().join("other.dng");
        std::fs::write(&source, b"sensor").unwrap();
        std::fs::write(&other, b"other").unwrap();
        let version = workflow::create_virtual_copy(&source, &crate::Sidecar::default()).unwrap();
        workflow::rename_virtual_copy(&source, &version, "Soft").unwrap();
        let db = CatalogDb::open_memory().unwrap();
        db.stack_photos(&[source.clone(), other.clone()]).unwrap();
        let parent = db.create_collection("Portraits").unwrap();
        let child = db.create_collection_in("Studio", Some(parent)).unwrap();
        db.add_to_collection(child, &source).unwrap();
        let package = dir.path().join("Travel");
        create(
            &db,
            &[
                (source.clone(), source.clone()),
                (other.clone(), other.clone()),
            ],
            &package,
            |_, _| {},
        )
        .unwrap();
        let moved = dir.path().join("Moved");
        std::fs::rename(&package, &moved).unwrap();
        let mut restored = CatalogDb::open_memory().unwrap();
        let photos = open(&mut restored, &moved.join("project.spproject")).unwrap();
        assert_eq!(photos.len(), 2);
        assert_eq!(
            workflow::virtual_copy_records(&photos[0]).unwrap()[0].name,
            "Soft"
        );
        assert_eq!(
            restored
                .photos(&LibraryFilter::default(), LibrarySort::Name)
                .unwrap()[0]
                .stack_size,
            2
        );
        assert_eq!(restored.collections().unwrap().len(), 3);
        std::fs::write(&photos[0], b"tampered").unwrap();
        assert!(open(&mut restored, &moved.join("project.spproject")).is_err());
        assert_eq!(std::fs::read(&source).unwrap(), b"sensor");
    }
}
