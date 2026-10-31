//! SQLite catalog. Sidecars remain the source of truth; every mirrored
//! annotation can be rebuilt from folders plus `.spedit` files.

use crate::{load_sidecar, Annotations, Flag};
use rusqlite::{params, Connection, OptionalExtension};
use rusqlite_migration::{Migrations, M};
use sp_core::{ImageMeta, SpError};
use std::path::{Path, PathBuf};

fn scan_file_hash(
    path: &Path,
    network_source: bool,
    modified_s: i64,
    previous: Option<&(i64, String)>,
) -> String {
    if let Some((old_modified_s, hash)) = previous {
        if *old_modified_s == modified_s && !hash.is_empty() {
            return hash.clone();
        }
    }
    // Reading every byte of every RAW merely to populate duplicate hints is
    // ruinous over SMB. Card import still hashes before copying; catalog scans
    // leave new/changed network hashes empty and prioritize usable browsing.
    if network_source {
        String::new()
    } else {
        crate::workflow::content_hash(path).unwrap_or_default()
    }
}

fn migrations() -> Migrations<'static> {
    let migrations = vec![
        M::up(
            "CREATE TABLE folders (
        id INTEGER PRIMARY KEY,
        path TEXT NOT NULL UNIQUE,
        added_at INTEGER NOT NULL DEFAULT (unixepoch())
     );
     CREATE TABLE photos (
        id INTEGER PRIMARY KEY,
        path TEXT NOT NULL UNIQUE,
        folder_id INTEGER NOT NULL REFERENCES folders(id) ON DELETE CASCADE,
        file_hash TEXT NOT NULL DEFAULT '',
        file_type TEXT NOT NULL,
        modified_s INTEGER NOT NULL DEFAULT 0,
        sidecar_mtime_s INTEGER NOT NULL DEFAULT 0,
        captured_at TEXT,
        camera_model TEXT NOT NULL DEFAULT '',
        rating INTEGER NOT NULL DEFAULT 0,
        flag INTEGER NOT NULL DEFAULT 0,
        edited INTEGER NOT NULL DEFAULT 0
     );
     CREATE INDEX photos_folder ON photos(folder_id);
     CREATE INDEX photos_rating ON photos(rating);
     CREATE TABLE collections (
        id INTEGER PRIMARY KEY,
        name TEXT NOT NULL UNIQUE,
        created_at INTEGER NOT NULL DEFAULT (unixepoch())
     );
     CREATE TABLE collection_photos (
        collection_id INTEGER NOT NULL REFERENCES collections(id) ON DELETE CASCADE,
        photo_id INTEGER NOT NULL REFERENCES photos(id) ON DELETE CASCADE,
        PRIMARY KEY(collection_id, photo_id)
     );
     CREATE TABLE export_presets (
        name TEXT PRIMARY KEY,
        settings_json TEXT NOT NULL
     );",
        ),
        M::up("ALTER TABLE photos ADD COLUMN scan_generation INTEGER NOT NULL DEFAULT 0;"),
        M::up(
            "ALTER TABLE photos ADD COLUMN lens_model TEXT NOT NULL DEFAULT '';
             ALTER TABLE photos ADD COLUMN iso INTEGER;
             ALTER TABLE photos ADD COLUMN caption TEXT NOT NULL DEFAULT '';
             ALTER TABLE photos ADD COLUMN keywords TEXT NOT NULL DEFAULT '';
             ALTER TABLE photos ADD COLUMN color_label INTEGER NOT NULL DEFAULT 0;
             CREATE INDEX photos_camera ON photos(camera_model);
             CREATE INDEX photos_lens ON photos(lens_model);
             CREATE INDEX photos_iso ON photos(iso);
             CREATE INDEX photos_hash ON photos(file_hash);
             CREATE TABLE smart_collections (
               id INTEGER PRIMARY KEY,
               name TEXT NOT NULL UNIQUE,
               filter_json TEXT NOT NULL,
               created_at INTEGER NOT NULL DEFAULT (unixepoch())
             );",
        ),
        // Full-text index over the searchable columns. An external-content
        // table reads rows back from `photos`, so it costs one index rather
        // than a copy; the triggers keep it current for every write path.
        M::up(
            "CREATE VIRTUAL TABLE photos_fts USING fts5(
                path, caption, keywords, camera_model, lens_model,
                content='photos', content_rowid='id',
                tokenize='unicode61 remove_diacritics 2'
             );
             CREATE TRIGGER photos_fts_insert AFTER INSERT ON photos BEGIN
                INSERT INTO photos_fts(rowid, path, caption, keywords, camera_model, lens_model)
                VALUES (new.id, new.path, new.caption, new.keywords, new.camera_model, new.lens_model);
             END;
             CREATE TRIGGER photos_fts_delete AFTER DELETE ON photos BEGIN
                INSERT INTO photos_fts(photos_fts, rowid, path, caption, keywords, camera_model, lens_model)
                VALUES ('delete', old.id, old.path, old.caption, old.keywords, old.camera_model, old.lens_model);
             END;
             CREATE TRIGGER photos_fts_update AFTER UPDATE ON photos BEGIN
                INSERT INTO photos_fts(photos_fts, rowid, path, caption, keywords, camera_model, lens_model)
                VALUES ('delete', old.id, old.path, old.caption, old.keywords, old.camera_model, old.lens_model);
                INSERT INTO photos_fts(rowid, path, caption, keywords, camera_model, lens_model)
                VALUES (new.id, new.path, new.caption, new.keywords, new.camera_model, new.lens_model);
             END;
             INSERT INTO photos_fts(photos_fts) VALUES ('rebuild');",
        ),
        M::up(
            "CREATE TABLE photo_companions (
                source_photo_id INTEGER NOT NULL REFERENCES photos(id) ON DELETE CASCADE,
                companion_photo_id INTEGER NOT NULL UNIQUE REFERENCES photos(id) ON DELETE CASCADE,
                created_at INTEGER NOT NULL DEFAULT (unixepoch()),
                PRIMARY KEY(source_photo_id, companion_photo_id)
             );
             CREATE INDEX photo_companions_source ON photo_companions(source_photo_id);",
        ),
        M::up("ALTER TABLE photos ADD COLUMN virtual_copies_json TEXT NOT NULL DEFAULT '[]';"),
        M::up("CREATE TABLE photo_derivations (
            derivative_photo_id INTEGER NOT NULL REFERENCES photos(id) ON DELETE CASCADE,
            source_photo_id INTEGER NOT NULL REFERENCES photos(id) ON DELETE CASCADE,
            source_order INTEGER NOT NULL,
            operation TEXT NOT NULL,
            PRIMARY KEY(derivative_photo_id, source_photo_id),
            CHECK(derivative_photo_id <> source_photo_id)
        );
        CREATE INDEX photo_derivations_source ON photo_derivations(source_photo_id);"),
        M::up("CREATE TABLE photo_stacks (
            id INTEGER PRIMARY KEY,
            cover_photo_id INTEGER NOT NULL REFERENCES photos(id) ON DELETE CASCADE
        );
        CREATE TABLE photo_stack_members (
            stack_id INTEGER NOT NULL REFERENCES photo_stacks(id) ON DELETE CASCADE,
            photo_id INTEGER PRIMARY KEY REFERENCES photos(id) ON DELETE CASCADE
        );
        CREATE INDEX photo_stack_members_stack ON photo_stack_members(stack_id);"),
        M::up("ALTER TABLE collections ADD COLUMN parent_id INTEGER REFERENCES collections(id) ON DELETE SET NULL;"),
    ];
    Migrations::from_slice(Box::leak(migrations.into_boxed_slice()))
}

/// One folder registered with the catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderRecord {
    pub id: i64,
    pub path: PathBuf,
}

/// Lightweight row consumed by the virtualized grid and filmstrip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogPhoto {
    pub id: i64,
    pub path: PathBuf,
    pub file_type: String,
    pub captured_at: Option<String>,
    pub camera_model: String,
    pub lens_model: String,
    pub iso: Option<u32>,
    pub caption: String,
    pub keywords: Vec<String>,
    pub color_label: crate::ColorLabel,
    pub duplicate: bool,
    pub annotations: Annotations,
    pub edited: bool,
    /// The RAW this externally edited derivative belongs to.
    pub companion_of: Option<PathBuf>,
    pub virtual_copies: Vec<crate::VirtualCopyRecord>,
    pub derived_sources: Vec<PathBuf>,
    pub derived_operation: Option<String>,
    pub stack_id: Option<i64>,
    pub stack_size: usize,
    pub stack_cover: bool,
    pub collection_ids: Vec<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Collection {
    pub id: i64,
    pub name: String,
    pub parent_id: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SmartCollection {
    pub id: i64,
    pub name: String,
    pub filter: LibraryFilter,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum LibrarySort {
    #[default]
    CaptureTime,
    Name,
    Rating,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LibraryFilter {
    pub minimum_rating: u8,
    pub flag: Option<Flag>,
    pub file_type: Option<String>,
    pub text: String,
    pub camera_model: Option<String>,
    pub lens_model: Option<String>,
    pub iso_min: Option<u32>,
    pub iso_max: Option<u32>,
    pub date_from: Option<String>,
    pub date_to: Option<String>,
    pub color_label: Option<crate::ColorLabel>,
    #[serde(default)]
    pub keyword: Option<String>,
}

/// Connection-owning catalog handle. Create it on the catalog worker thread;
/// rusqlite connections are intentionally not shared across UI/GPU threads.
pub struct CatalogDb {
    connection: Connection,
}

impl CatalogDb {
    pub fn open(path: &Path) -> Result<Self, SpError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|source| SpError::io(parent.display().to_string(), source))?;
        }
        let mut connection = Connection::open(path).map_err(|e| SpError::Catalog(e.to_string()))?;
        connection
            .execute_batch("PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL;")
            .map_err(|e| SpError::Catalog(e.to_string()))?;
        migrations()
            .to_latest(&mut connection)
            .map_err(|e| SpError::Catalog(e.to_string()))?;
        Ok(Self { connection })
    }

    pub fn open_memory() -> Result<Self, SpError> {
        let mut connection =
            Connection::open_in_memory().map_err(|e| SpError::Catalog(e.to_string()))?;
        connection
            .execute_batch("PRAGMA foreign_keys=ON;")
            .map_err(|e| SpError::Catalog(e.to_string()))?;
        migrations()
            .to_latest(&mut connection)
            .map_err(|e| SpError::Catalog(e.to_string()))?;
        Ok(Self { connection })
    }

    pub fn add_folder(&self, path: &Path) -> Result<i64, SpError> {
        let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        self.connection
            .execute(
                "INSERT INTO folders(path) VALUES(?1) ON CONFLICT(path) DO NOTHING",
                [canonical.to_string_lossy().as_ref()],
            )
            .map_err(|e| SpError::Catalog(e.to_string()))?;
        self.connection
            .query_row(
                "SELECT id FROM folders WHERE path=?1",
                [canonical.to_string_lossy().as_ref()],
                |row| row.get(0),
            )
            .map_err(|e| SpError::Catalog(e.to_string()))
    }

    pub fn folders(&self) -> Result<Vec<FolderRecord>, SpError> {
        let mut statement = self
            .connection
            .prepare("SELECT id,path FROM folders ORDER BY path COLLATE NOCASE")
            .map_err(|e| SpError::Catalog(e.to_string()))?;
        let rows = statement
            .query_map([], |row| {
                Ok(FolderRecord {
                    id: row.get(0)?,
                    path: PathBuf::from(row.get::<_, String>(1)?),
                })
            })
            .map_err(|e| SpError::Catalog(e.to_string()))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| SpError::Catalog(e.to_string()))
    }

    /// Stop cataloguing a folder and remove its indexed photos/collection
    /// memberships. This only mutates the rebuildable SQLite catalog;
    /// originals and sidecars on disk are never touched.
    pub fn remove_folder(&self, folder_id: i64) -> Result<bool, SpError> {
        let removed = self
            .connection
            .execute("DELETE FROM folders WHERE id=?1", [folder_id])
            .map_err(|e| SpError::Catalog(e.to_string()))?;
        Ok(removed != 0)
    }

    /// Recursively scan supported RAWs. Sidecars are read during every scan
    /// and overwrite mirrored rating/flag data.
    pub fn scan_folder(
        &mut self,
        path: &Path,
        mut progress: impl FnMut(usize),
    ) -> Result<usize, SpError> {
        // Opening the directory before changing the database makes a missing
        // or disconnected share a non-destructive scan failure. In
        // particular, it must never look like an empty folder and delete all
        // of its cached catalog rows.
        std::fs::read_dir(path)
            .map_err(|source| SpError::io(path.display().to_string(), source))?;
        let network_source = crate::source_cache::is_network_path(path);
        let folder_id = self.add_folder(path)?;
        let mut traversal_error = None;
        let mut paths = Vec::new();
        for entry in walkdir::WalkDir::new(path)
            .follow_links(false)
            .into_iter()
            .filter_entry(|entry| entry.file_name() != ".shopphoto-import")
        {
            match entry {
                Ok(entry) if entry.file_type().is_file() && is_catalog_media(entry.path()) => {
                    paths.push(entry.into_path());
                }
                Ok(_) => {}
                Err(error) => {
                    tracing::warn!(folder = %path.display(), "catalog traversal skipped an unreadable entry: {error}");
                    traversal_error.get_or_insert_with(|| error.to_string());
                }
            }
        }
        let transaction = self
            .connection
            .transaction()
            .map_err(|e| SpError::Catalog(e.to_string()))?;
        let scan_generation = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
            .min(i64::MAX as u128) as i64;
        for (index, photo) in paths.iter().enumerate() {
            let metadata = std::fs::metadata(photo)
                .map_err(|source| SpError::io(photo.display().to_string(), source))?;
            let modified_s = metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |duration| duration.as_secs() as i64);
            let sidecar = load_sidecar(photo).ok().flatten();
            let foreign = if sidecar.is_none() {
                crate::xmp::load_with_metadata(photo).ok().flatten()
            } else {
                None
            };
            // Photos rated in Lightroom/Bridge before they ever reached
            // ShopPhoto carry their stars in an .xmp sidecar; adopt those
            // when we have no sidecar of our own for the file.
            let annotations = sidecar.as_ref().map_or_else(
                || {
                    foreign
                        .as_ref()
                        .and_then(|document| document.annotations)
                        .unwrap_or_default()
                },
                |value| value.annotations,
            );
            let edited = sidecar
                .as_ref()
                .is_some_and(|value| value.edit != Default::default());
            let descriptive = sidecar
                .as_ref()
                .map(|value| &value.metadata)
                .or_else(|| foreign.as_ref().map(|document| &document.metadata));
            let extension = photo
                .extension()
                .and_then(|value| value.to_str())
                .unwrap_or_default()
                .to_ascii_lowercase();
            let previous = transaction
                .query_row(
                    "SELECT modified_s,file_hash FROM photos WHERE path=?1",
                    [photo.to_string_lossy().as_ref()],
                    |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
                )
                .optional()
                .map_err(|error| SpError::Catalog(error.to_string()))?;
            let file_hash = scan_file_hash(photo, network_source, modified_s, previous.as_ref());
            transaction
                .execute(
                    "INSERT INTO photos(path,folder_id,file_type,modified_s,rating,flag,edited,scan_generation,caption,keywords,color_label,file_hash)
                     VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)
                     ON CONFLICT(path) DO UPDATE SET
                       folder_id=excluded.folder_id,file_type=excluded.file_type,
                       modified_s=excluded.modified_s,rating=excluded.rating,
                       flag=excluded.flag,edited=excluded.edited,caption=excluded.caption,
                       keywords=excluded.keywords,color_label=excluded.color_label,file_hash=excluded.file_hash,
                       scan_generation=excluded.scan_generation",
                    params![
                        photo.to_string_lossy().as_ref(),
                        folder_id,
                        extension,
                        modified_s,
                        annotations.rating.min(5),
                        flag_to_i64(annotations.flag),
                        i64::from(edited),
                        scan_generation,
                        descriptive.map_or("", |value| value.caption.as_str()),
                        descriptive.map_or_else(String::new, |value| value.keywords.join("\u{1f}")),
                        descriptive.map_or(0, |value| color_label_to_i64(value.color_label)),
                        file_hash,
                    ],
                )
                .map_err(|e| SpError::Catalog(e.to_string()))?;
            if let Ok(copies) = crate::workflow::virtual_copy_records(photo) {
                let json =
                    serde_json::to_string(&copies).map_err(|e| SpError::Catalog(e.to_string()))?;
                transaction
                    .execute(
                        "UPDATE photos SET virtual_copies_json=?2 WHERE path=?1",
                        params![photo.to_string_lossy().as_ref(), json],
                    )
                    .map_err(|e| SpError::Catalog(e.to_string()))?;
            }
            progress(index + 1);
        }
        if traversal_error.is_none() {
            transaction
                .execute(
                    "DELETE FROM photos WHERE folder_id=?1 AND scan_generation<>?2",
                    params![folder_id, scan_generation],
                )
                .map_err(|e| SpError::Catalog(e.to_string()))?;
        }
        transaction
            .commit()
            .map_err(|e| SpError::Catalog(e.to_string()))?;
        if let Some(error) = traversal_error {
            return Err(SpError::Catalog(format!(
                "{} could not be scanned completely; existing entries were kept: {error}",
                path.display()
            )));
        }
        Ok(paths.len())
    }

    pub fn photos(
        &self,
        filter: &LibraryFilter,
        sort: LibrarySort,
    ) -> Result<Vec<CatalogPhoto>, SpError> {
        let order = match sort {
            LibrarySort::CaptureTime => "COALESCE(captured_at,''), path",
            LibrarySort::Name => "path COLLATE NOCASE",
            LibrarySort::Rating => "rating DESC, path COLLATE NOCASE",
        };
        let sql = format!(
            "SELECT id,path,file_type,captured_at,camera_model,lens_model,iso,caption,keywords,color_label,
                    rating,flag,edited,
                    EXISTS(SELECT 1 FROM photos other WHERE other.file_hash=photos.file_hash AND other.id<>photos.id AND photos.file_hash<>''),
                    (SELECT source.path FROM photo_companions relation
                       JOIN photos source ON source.id=relation.source_photo_id
                      WHERE relation.companion_photo_id=photos.id), virtual_copies_json,
                    (SELECT json_group_array(path) FROM
                        (SELECT source.path FROM photo_derivations relation
                         JOIN photos source ON source.id=relation.source_photo_id
                         WHERE relation.derivative_photo_id=photos.id ORDER BY relation.source_order)),
                    (SELECT operation FROM photo_derivations WHERE derivative_photo_id=photos.id LIMIT 1),
                    (SELECT stack_id FROM photo_stack_members WHERE photo_id=photos.id),
                    (SELECT count(*) FROM photo_stack_members WHERE stack_id=(SELECT stack_id FROM photo_stack_members WHERE photo_id=photos.id)),
                    EXISTS(SELECT 1 FROM photo_stacks WHERE cover_photo_id=photos.id),
                    (SELECT json_group_array(collection_id) FROM collection_photos WHERE photo_id=photos.id)
             FROM photos WHERE rating>=?1
             AND (?2 IS NULL OR flag=?2)
             AND (?3 IS NULL OR file_type=?3)
             AND (?12='' OR photos.id IN (SELECT rowid FROM photos_fts WHERE photos_fts MATCH ?12))
             AND (?5 IS NULL OR camera_model=?5)
             AND (?6 IS NULL OR lens_model=?6)
             AND (?7 IS NULL OR iso>=?7)
             AND (?8 IS NULL OR iso<=?8)
             AND (?9 IS NULL OR captured_at>=?9)
             AND (?10 IS NULL OR captured_at<=?10)
             AND (?11 IS NULL OR color_label=?11)
             ORDER BY {order}"
        );
        let flag = filter.flag.map(flag_to_i64);
        let mut statement = self
            .connection
            .prepare(&sql)
            .map_err(|e| SpError::Catalog(e.to_string()))?;
        let rows = statement
            .query_map(
                params![
                    filter.minimum_rating.min(5),
                    flag,
                    filter.file_type,
                    filter.text,
                    filter.camera_model,
                    filter.lens_model,
                    filter.iso_min,
                    filter.iso_max,
                    filter.date_from,
                    filter.date_to,
                    filter.color_label.map(color_label_to_i64),
                    if filter.text.contains(':')
                        || filter.text.contains('"')
                        || filter
                            .text
                            .split_whitespace()
                            .any(|token| token.eq_ignore_ascii_case("OR") || token.starts_with('-'))
                    {
                        String::new()
                    } else {
                        fts_query(&filter.text)
                    }
                ],
                |row| {
                    Ok(CatalogPhoto {
                        id: row.get(0)?,
                        path: PathBuf::from(row.get::<_, String>(1)?),
                        file_type: row.get(2)?,
                        captured_at: row.get(3)?,
                        camera_model: row.get(4)?,
                        lens_model: row.get(5)?,
                        iso: row.get(6)?,
                        caption: row.get(7)?,
                        keywords: row
                            .get::<_, String>(8)?
                            .split('\u{1f}')
                            .filter(|v| !v.is_empty())
                            .map(str::to_owned)
                            .collect(),
                        color_label: i64_to_color_label(row.get(9)?),
                        annotations: Annotations {
                            rating: row.get(10)?,
                            flag: i64_to_flag(row.get(11)?),
                        },
                        edited: row.get::<_, i64>(12)? != 0,
                        duplicate: row.get::<_, i64>(13)? != 0,
                        companion_of: row.get::<_, Option<String>>(14)?.map(PathBuf::from),
                        virtual_copies: serde_json::from_str(&row.get::<_, String>(15)?)
                            .unwrap_or_default(),
                        derived_sources: serde_json::from_str(&row.get::<_, String>(16)?)
                            .unwrap_or_default(),
                        derived_operation: row.get(17)?,
                        stack_id: row.get(18)?,
                        stack_size: row.get(19)?,
                        stack_cover: row.get(20)?,
                        collection_ids: serde_json::from_str(&row.get::<_, String>(21)?)
                            .unwrap_or_default(),
                    })
                },
            )
            .map_err(|e| SpError::Catalog(e.to_string()))?;
        let mut photos = rows
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| SpError::Catalog(e.to_string()))?;
        photos.retain(|photo| {
            crate::search::matches(photo, &filter.text)
                && filter.keyword.as_ref().is_none_or(|keyword| {
                    photo
                        .keywords
                        .iter()
                        .any(|value| crate::search::keyword_matches(value, keyword))
                })
        });
        Ok(photos)
    }

    pub fn set_annotations(&self, path: &Path, value: Annotations) -> Result<(), SpError> {
        self.connection
            .execute(
                "UPDATE photos SET rating=?2,flag=?3 WHERE path=?1",
                params![
                    path.to_string_lossy().as_ref(),
                    value.rating.min(5),
                    flag_to_i64(value.flag)
                ],
            )
            .map_err(|e| SpError::Catalog(e.to_string()))?;
        Ok(())
    }

    /// Update paths after a successful filesystem move without replacing photo
    /// rows. Stable row ids preserve collections and RAW/derivative relations.
    /// Call before rescanning source folders; reverse the pairs when undoing.
    pub fn apply_file_moves(&self, moves: &[(PathBuf, PathBuf)]) -> Result<(), SpError> {
        let transaction = self
            .connection
            .unchecked_transaction()
            .map_err(|e| SpError::Catalog(e.to_string()))?;
        for (source, destination) in moves {
            if source == destination {
                continue;
            }
            let parent = destination.parent().unwrap_or_else(|| Path::new("."));
            let parent = crate::workflow::normalized_source_path(parent);
            transaction
                .execute(
                    "INSERT INTO folders(path) VALUES(?1) ON CONFLICT(path) DO NOTHING",
                    [parent.to_string_lossy().as_ref()],
                )
                .map_err(|e| SpError::Catalog(e.to_string()))?;
            transaction.execute(
                "UPDATE photos SET path=?2,folder_id=(SELECT id FROM folders WHERE path=?3) WHERE path=?1 OR path=?4",
                params![source.to_string_lossy().as_ref(), destination.to_string_lossy().as_ref(), parent.to_string_lossy().as_ref(), crate::workflow::normalized_source_path(source).to_string_lossy().as_ref()],
            ).map_err(|e| SpError::Catalog(e.to_string()))?;
        }
        transaction
            .commit()
            .map_err(|e| SpError::Catalog(e.to_string()))
    }

    /// Create a transactionally consistent SQLite backup, including WAL data.
    /// Photo originals and sidecars are stored separately and are not copied.
    pub fn backup_to(&self, destination: &Path) -> Result<(), SpError> {
        if destination.exists() {
            return Err(SpError::Catalog(format!(
                "Backup destination already exists: {}",
                destination.display()
            )));
        }
        self.connection
            .execute("VACUUM INTO ?1", [destination.to_string_lossy().as_ref()])
            .map_err(|e| SpError::Catalog(e.to_string()))?;
        Self::open(destination)?.integrity_check()
    }

    pub fn integrity_check(&self) -> Result<(), SpError> {
        let mut statement = self
            .connection
            .prepare("PRAGMA integrity_check")
            .map_err(|e| SpError::Catalog(e.to_string()))?;
        let messages = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|e| SpError::Catalog(e.to_string()))?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| SpError::Catalog(e.to_string()))?;
        if messages != ["ok"] {
            return Err(SpError::Catalog(messages.join("; ")));
        }
        let foreign_keys: i64 = self
            .connection
            .query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |row| {
                row.get(0)
            })
            .map_err(|e| SpError::Catalog(e.to_string()))?;
        if foreign_keys != 0 {
            return Err(SpError::Catalog(format!(
                "{foreign_keys} broken catalog relationships"
            )));
        }
        Ok(())
    }

    /// Validate and migrate a private staging copy, preserve the current
    /// catalog, then restore through SQLite's atomic backup transaction. The
    /// database inode and WAL connections remain valid throughout; a failed
    /// SQLite copy rolls back instead of replacing files under open readers.
    pub fn restore_from(&mut self, source: &Path, rescue: &Path) -> Result<(), SpError> {
        let source_db = Self {
            connection: Connection::open_with_flags(
                source,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            )
            .map_err(|e| SpError::Catalog(e.to_string()))?,
        };
        source_db.integrity_check()?;
        source_db.connection.prepare("SELECT photos.path,folders.path FROM photos JOIN folders ON photos.folder_id=folders.id LIMIT 0").map_err(|_| SpError::Catalog("This file is not a ShopPhoto catalog backup".into()))?;
        let mut stage = Self {
            connection: Connection::open_in_memory()
                .map_err(|e| SpError::Catalog(e.to_string()))?,
        };
        fn copy_database(source: &Connection, destination: &mut Connection) -> Result<(), SpError> {
            let backup = rusqlite::backup::Backup::new(source, destination)
                .map_err(|e| SpError::Catalog(e.to_string()))?;
            match backup
                .step(-1)
                .map_err(|e| SpError::Catalog(e.to_string()))?
            {
                rusqlite::backup::StepResult::Done => Ok(()),
                _ => Err(SpError::Catalog(
                    "The catalog is busy; retry the restore when other work finishes".into(),
                )),
            }
        }
        copy_database(&source_db.connection, &mut stage.connection)?;
        migrations()
            .to_latest(&mut stage.connection)
            .map_err(|e| SpError::Catalog(e.to_string()))?;
        stage.integrity_check()?;
        self.backup_to(rescue)?;
        copy_database(&stage.connection, &mut self.connection)?;
        self.integrity_check()
    }

    /// Relink a moved folder after checking every catalogued file is present.
    /// Originals are untouched and stable photo ids retain all relationships.
    pub fn relink_folder(&self, folder_id: i64, destination: &Path) -> Result<usize, SpError> {
        let destination = crate::workflow::normalized_source_path(destination);
        let old: String = self
            .connection
            .query_row("SELECT path FROM folders WHERE id=?1", [folder_id], |row| {
                row.get(0)
            })
            .map_err(|e| SpError::Catalog(e.to_string()))?;
        let old = crate::workflow::normalized_source_path(Path::new(&old));
        let mut statement = self
            .connection
            .prepare("SELECT id,path,file_hash FROM photos WHERE folder_id=?1")
            .map_err(|e| SpError::Catalog(e.to_string()))?;
        let photos = statement
            .query_map([folder_id], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(|e| SpError::Catalog(e.to_string()))?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| SpError::Catalog(e.to_string()))?;
        drop(statement);
        let mut moves = Vec::new();
        for (id, path, expected_hash) in photos {
            let normalized = crate::workflow::normalized_source_path(Path::new(&path));
            let relative = normalized
                .strip_prefix(&old)
                .map_err(|e| SpError::Catalog(e.to_string()))?;
            let target = destination.join(relative);
            if !target.is_file() {
                return Err(SpError::Catalog(format!(
                    "Relink target is missing: {}",
                    target.display()
                )));
            }
            if !expected_hash.is_empty() && crate::workflow::content_hash(&target)? != expected_hash
            {
                return Err(SpError::Catalog(format!(
                    "Relink target has different content: {}",
                    target.display()
                )));
            }
            moves.push((id, target));
        }
        if !destination.is_dir() {
            return Err(SpError::Catalog(
                "Relink destination must be a folder".into(),
            ));
        }
        let transaction = self
            .connection
            .unchecked_transaction()
            .map_err(|e| SpError::Catalog(e.to_string()))?;
        transaction
            .execute(
                "UPDATE folders SET path=?2 WHERE id=?1",
                params![folder_id, destination.to_string_lossy().as_ref()],
            )
            .map_err(|e| SpError::Catalog(e.to_string()))?;
        for (id, path) in &moves {
            transaction
                .execute(
                    "UPDATE photos SET path=?2 WHERE id=?1",
                    params![id, path.to_string_lossy().as_ref()],
                )
                .map_err(|e| SpError::Catalog(e.to_string()))?;
        }
        transaction
            .commit()
            .map_err(|e| SpError::Catalog(e.to_string()))?;
        Ok(moves.len())
    }

    pub fn set_virtual_copies(
        &self,
        original: &Path,
        copies: &[crate::VirtualCopyRecord],
    ) -> Result<(), SpError> {
        let json = serde_json::to_string(copies).map_err(|e| SpError::Catalog(e.to_string()))?;
        self.connection
            .execute(
                "UPDATE photos SET virtual_copies_json=?2 WHERE path=?1",
                params![original.to_string_lossy().as_ref(), json],
            )
            .map_err(|e| SpError::Catalog(e.to_string()))?;
        Ok(())
    }

    pub fn set_descriptive_metadata(
        &self,
        path: &Path,
        value: &crate::DescriptiveMetadata,
    ) -> Result<(), SpError> {
        self.connection
            .execute(
                "UPDATE photos SET caption=?2,keywords=?3,color_label=?4 WHERE path=?1",
                params![
                    path.to_string_lossy().as_ref(),
                    value.caption,
                    value.keywords.join("\u{1f}"),
                    color_label_to_i64(value.color_label)
                ],
            )
            .map_err(|e| SpError::Catalog(e.to_string()))?;
        Ok(())
    }

    /// Register a rendered file as an externally edited derivative of a RAW.
    /// Both rows are ensured first so the workflow also works for a RAW that
    /// was opened directly rather than through a catalog folder.
    pub fn add_companion(&self, source: &Path, companion: &Path) -> Result<(), SpError> {
        let source_id = self.ensure_media_row(source)?;
        let companion_id = self.ensure_media_row(companion)?;
        self.connection
            .execute(
                "INSERT INTO photo_companions(source_photo_id,companion_photo_id)
                 VALUES(?1,?2)
                 ON CONFLICT(companion_photo_id) DO UPDATE SET source_photo_id=excluded.source_photo_id",
                params![source_id, companion_id],
            )
            .map_err(|error| SpError::Catalog(error.to_string()))?;
        Ok(())
    }

    /// Keep ordered, many-source provenance for a merged derivative. Photo IDs
    /// make these links survive rename and relink operations.
    pub fn link_derived_sources(
        &self,
        derivative: &Path,
        sources: &[PathBuf],
        operation: &str,
    ) -> Result<(), SpError> {
        if sources.is_empty() || operation.trim().is_empty() || operation.chars().count() > 120 {
            return Err(SpError::Catalog(
                "A derivative needs sources and an operation name of 1–120 characters".into(),
            ));
        }
        let derivative_id = self.ensure_media_row(derivative)?;
        let source_ids = sources
            .iter()
            .map(|path| self.ensure_media_row(path))
            .collect::<Result<Vec<_>, _>>()?;
        if source_ids.contains(&derivative_id) {
            return Err(SpError::Catalog("A photo cannot derive from itself".into()));
        }
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(|e| SpError::Catalog(e.to_string()))?;
        tx.execute(
            "DELETE FROM photo_derivations WHERE derivative_photo_id=?1",
            [derivative_id],
        )
        .map_err(|e| SpError::Catalog(e.to_string()))?;
        for (order, source_id) in source_ids.iter().enumerate() {
            tx.execute("INSERT OR IGNORE INTO photo_derivations(derivative_photo_id,source_photo_id,source_order,operation) VALUES(?1,?2,?3,?4)", params![derivative_id,source_id,order as i64,operation.trim()]).map_err(|e| SpError::Catalog(e.to_string()))?;
        }
        tx.commit().map_err(|e| SpError::Catalog(e.to_string()))
    }

    /// Group at least two unstacked photos without moving any files. The
    /// first photo becomes the cover; membership follows stable catalog IDs.
    pub fn stack_photos(&self, photos: &[PathBuf]) -> Result<i64, SpError> {
        let mut ids = photos
            .iter()
            .map(|path| self.ensure_media_row(path))
            .collect::<Result<Vec<_>, _>>()?;
        let cover = ids
            .first()
            .copied()
            .ok_or_else(|| SpError::Catalog("Select at least two photos to stack".into()))?;
        ids.sort_unstable();
        ids.dedup();
        if ids.len() < 2 {
            return Err(SpError::Catalog(
                "Select at least two different photos to stack".into(),
            ));
        }
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(|e| SpError::Catalog(e.to_string()))?;
        for id in &ids {
            let occupied: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM photo_stack_members WHERE photo_id=?1)",
                    [id],
                    |row| row.get(0),
                )
                .map_err(|e| SpError::Catalog(e.to_string()))?;
            if occupied {
                return Err(SpError::Catalog(
                    "Unstack existing groups before combining them into a new stack".into(),
                ));
            }
        }
        tx.execute(
            "INSERT INTO photo_stacks(cover_photo_id) VALUES(?1)",
            [cover],
        )
        .map_err(|e| SpError::Catalog(e.to_string()))?;
        let stack = tx.last_insert_rowid();
        for id in ids {
            tx.execute(
                "INSERT INTO photo_stack_members(stack_id,photo_id) VALUES(?1,?2)",
                params![stack, id],
            )
            .map_err(|e| SpError::Catalog(e.to_string()))?;
        }
        tx.commit().map_err(|e| SpError::Catalog(e.to_string()))?;
        Ok(stack)
    }

    pub fn dissolve_stack(&self, stack: i64) -> Result<(), SpError> {
        self.connection
            .execute("DELETE FROM photo_stacks WHERE id=?1", [stack])
            .map_err(|e| SpError::Catalog(e.to_string()))?;
        Ok(())
    }

    pub fn set_stack_cover(&self, stack: i64, photo: &Path) -> Result<(), SpError> {
        let id = self.ensure_media_row(photo)?;
        let changed = self.connection.execute("UPDATE photo_stacks SET cover_photo_id=?2 WHERE id=?1 AND EXISTS(SELECT 1 FROM photo_stack_members WHERE stack_id=?1 AND photo_id=?2)",params![stack,id]).map_err(|e|SpError::Catalog(e.to_string()))?;
        if changed == 0 {
            return Err(SpError::Catalog("This photo is not in that stack".into()));
        }
        Ok(())
    }

    #[cfg(test)]
    fn companion_source(&self, companion: &Path) -> Result<Option<PathBuf>, SpError> {
        self.connection
            .query_row(
                "SELECT source.path FROM photo_companions relation
                 JOIN photos companion ON companion.id=relation.companion_photo_id
                 JOIN photos source ON source.id=relation.source_photo_id
                 WHERE companion.path=?1",
                [companion.to_string_lossy().as_ref()],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map(|value| value.map(PathBuf::from))
            .map_err(|error| SpError::Catalog(error.to_string()))
    }

    fn ensure_media_row(&self, path: &Path) -> Result<i64, SpError> {
        let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        // Prefer the exact spelling already used by a folder scan. macOS can
        // resolve `/var` to `/private/var`; blindly canonicalizing here would
        // create a second row for the same file and attach the relation to the
        // duplicate instead of the grid's photo.
        let existing_path = self
            .connection
            .query_row(
                "SELECT path FROM photos WHERE path=?1 OR path=?2 ORDER BY path=?1 DESC LIMIT 1",
                params![
                    path.to_string_lossy().as_ref(),
                    canonical.to_string_lossy().as_ref()
                ],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|error| SpError::Catalog(error.to_string()))?
            .map(PathBuf::from);
        let stored_path = existing_path.unwrap_or(canonical);
        let parent = stored_path.parent().ok_or_else(|| {
            SpError::Catalog(format!("{} has no parent folder", stored_path.display()))
        })?;
        let folder_id = self.add_folder(parent)?;
        let metadata = std::fs::metadata(&stored_path)
            .map_err(|error| SpError::io(stored_path.display().to_string(), error))?;
        let modified_s = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |duration| duration.as_secs() as i64);
        let extension = stored_path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        let hash = crate::workflow::content_hash(&stored_path).unwrap_or_default();
        self.connection
            .execute(
                "INSERT INTO photos(path,folder_id,file_type,modified_s,file_hash)
                 VALUES(?1,?2,?3,?4,?5)
                 ON CONFLICT(path) DO UPDATE SET folder_id=excluded.folder_id,
                   file_type=excluded.file_type,modified_s=excluded.modified_s,file_hash=excluded.file_hash",
                params![
                    stored_path.to_string_lossy().as_ref(),
                    folder_id,
                    extension,
                    modified_s,
                    hash
                ],
            )
            .map_err(|error| SpError::Catalog(error.to_string()))?;
        self.connection
            .query_row(
                "SELECT id FROM photos WHERE path=?1",
                [stored_path.to_string_lossy().as_ref()],
                |row| row.get(0),
            )
            .map_err(|error| SpError::Catalog(error.to_string()))
    }

    pub fn save_smart_collection(
        &self,
        name: &str,
        filter: &LibraryFilter,
    ) -> Result<i64, SpError> {
        let json = serde_json::to_string(filter).map_err(|e| SpError::Catalog(e.to_string()))?;
        self.connection
            .execute(
                "INSERT INTO smart_collections(name,filter_json) VALUES(?1,?2)
             ON CONFLICT(name) DO UPDATE SET filter_json=excluded.filter_json",
                params![name, json],
            )
            .map_err(|e| SpError::Catalog(e.to_string()))?;
        self.connection
            .query_row(
                "SELECT id FROM smart_collections WHERE name=?1",
                [name],
                |row| row.get(0),
            )
            .map_err(|e| SpError::Catalog(e.to_string()))
    }

    pub fn smart_collections(&self) -> Result<Vec<SmartCollection>, SpError> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT id,name,filter_json FROM smart_collections ORDER BY name COLLATE NOCASE",
            )
            .map_err(|e| SpError::Catalog(e.to_string()))?;
        let rows = statement
            .query_map([], |row| {
                let json: String = row.get(2)?;
                let filter = serde_json::from_str(&json).unwrap_or_default();
                Ok(SmartCollection {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    filter,
                })
            })
            .map_err(|e| SpError::Catalog(e.to_string()))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| SpError::Catalog(e.to_string()))
    }

    /// Mirror metadata learned by the decode pool. Sidecars remain the edit
    /// authority; catalog metadata is a rebuildable search/sort index.
    pub fn set_metadata(&self, path: &Path, meta: &ImageMeta) -> Result<(), SpError> {
        self.connection
            .execute(
                "UPDATE photos SET captured_at=?2,camera_model=?3,lens_model=?4,iso=?5 WHERE path=?1",
                params![
                    path.to_string_lossy().as_ref(),
                    meta.captured_at,
                    meta.camera_model,
                    meta.lens_model.as_deref().unwrap_or(""),
                    meta.iso,
                ],
            )
            .map_err(|e| SpError::Catalog(e.to_string()))?;
        Ok(())
    }

    /// Store a serialized export preset. The catalog intentionally treats
    /// settings as opaque JSON so it does not depend on the export crate.
    pub fn save_export_preset(&self, name: &str, settings_json: &str) -> Result<(), SpError> {
        self.connection
            .execute(
                "INSERT INTO export_presets(name,settings_json) VALUES(?1,?2)
                 ON CONFLICT(name) DO UPDATE SET settings_json=excluded.settings_json",
                params![name, settings_json],
            )
            .map_err(|e| SpError::Catalog(e.to_string()))?;
        Ok(())
    }

    pub fn export_presets(&self) -> Result<Vec<(String, String)>, SpError> {
        let mut statement = self
            .connection
            .prepare("SELECT name,settings_json FROM export_presets ORDER BY name COLLATE NOCASE")
            .map_err(|e| SpError::Catalog(e.to_string()))?;
        let rows = statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(|e| SpError::Catalog(e.to_string()))?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| SpError::Catalog(e.to_string()))?;
        Ok(rows)
    }

    pub fn create_collection(&self, name: &str) -> Result<i64, SpError> {
        self.create_collection_in(name, None)
    }

    pub fn create_collection_in(&self, name: &str, parent: Option<i64>) -> Result<i64, SpError> {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 160 {
            return Err(SpError::Catalog(
                "Collection names must contain 1–160 characters".into(),
            ));
        }
        self.connection
            .execute(
                "INSERT INTO collections(name,parent_id) VALUES(?1,?2) ON CONFLICT(name) DO NOTHING",
                params![name,parent],
            )
            .map_err(|e| SpError::Catalog(e.to_string()))?;
        let (id, existing): (i64, Option<i64>) = self
            .connection
            .query_row(
                "SELECT id,parent_id FROM collections WHERE name=?1",
                [name],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(|e| SpError::Catalog(e.to_string()))?;
        if existing != parent {
            return Err(SpError::Catalog(
                "A collection with that name already exists elsewhere".into(),
            ));
        }
        Ok(id)
    }

    pub fn move_collection(&self, id: i64, parent: Option<i64>) -> Result<(), SpError> {
        if let Some(parent) = parent {
            let cycle:bool = self.connection.query_row("WITH RECURSIVE ancestors(id,parent_id) AS (SELECT id,parent_id FROM collections WHERE id=?1 UNION SELECT c.id,c.parent_id FROM collections c JOIN ancestors a ON c.id=a.parent_id) SELECT EXISTS(SELECT 1 FROM ancestors WHERE id=?2)",params![parent,id],|row|row.get(0)).map_err(|e|SpError::Catalog(e.to_string()))?;
            if cycle {
                return Err(SpError::Catalog(
                    "A collection cannot be placed inside itself or its descendants".into(),
                ));
            }
        }
        self.connection
            .execute(
                "UPDATE collections SET parent_id=?2 WHERE id=?1",
                params![id, parent],
            )
            .map_err(|e| SpError::Catalog(e.to_string()))?;
        Ok(())
    }

    pub fn delete_collection(&self, id: i64) -> Result<(), SpError> {
        self.connection
            .execute("DELETE FROM collections WHERE id=?1", [id])
            .map_err(|e| SpError::Catalog(e.to_string()))?;
        Ok(())
    }

    pub fn collections(&self) -> Result<Vec<Collection>, SpError> {
        let mut statement = self
            .connection
            .prepare("SELECT id,name,parent_id FROM collections ORDER BY name COLLATE NOCASE")
            .map_err(|e| SpError::Catalog(e.to_string()))?;
        let rows = statement
            .query_map([], |row| {
                Ok(Collection {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    parent_id: row.get(2)?,
                })
            })
            .map_err(|e| SpError::Catalog(e.to_string()))?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| SpError::Catalog(e.to_string()))?;
        Ok(rows)
    }

    pub fn add_to_collection(&self, collection_id: i64, photo: &Path) -> Result<(), SpError> {
        let photo_id: Option<i64> = self
            .connection
            .query_row(
                "SELECT id FROM photos WHERE path=?1 OR path=?2",
                params![
                    photo.to_string_lossy().as_ref(),
                    crate::workflow::normalized_source_path(photo)
                        .to_string_lossy()
                        .as_ref()
                ],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| SpError::Catalog(e.to_string()))?;
        let Some(photo_id) = photo_id else {
            return Err(SpError::Catalog("photo is not catalogued".into()));
        };
        self.connection
            .execute(
                "INSERT OR IGNORE INTO collection_photos(collection_id,photo_id) VALUES(?1,?2)",
                params![collection_id, photo_id],
            )
            .map_err(|e| SpError::Catalog(e.to_string()))?;
        Ok(())
    }
}

/// Turn free text into an FTS5 MATCH expression: every whitespace-separated
/// term becomes a quoted prefix phrase, and terms are ANDed. Quoting keeps
/// user input such as `AND`, `-` or `(` from being read as query syntax.
/// `IMG_0001` becomes the phrase `img 0001*`, which still finds
/// `IMG_00012.3FR` the way the old substring search did.
fn is_catalog_media(path: &Path) -> bool {
    path.extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| {
            let extension = value.to_ascii_lowercase();
            sp_core::raw::SUPPORTED_RAW_EXTENSIONS.contains(&extension.as_str())
                || sp_core::source::SUPPORTED_RASTER_EXTENSIONS.contains(&extension.as_str())
        })
}

fn fts_query(text: &str) -> String {
    text.split_whitespace()
        .filter(|term| term.chars().any(char::is_alphanumeric))
        .map(|term| format!("\"{}\"*", term.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" ")
}

fn flag_to_i64(flag: Flag) -> i64 {
    match flag {
        Flag::None => 0,
        Flag::Pick => 1,
        Flag::Reject => 2,
    }
}

fn i64_to_flag(value: i64) -> Flag {
    match value {
        1 => Flag::Pick,
        2 => Flag::Reject,
        _ => Flag::None,
    }
}

fn color_label_to_i64(label: crate::ColorLabel) -> i64 {
    match label {
        crate::ColorLabel::None => 0,
        crate::ColorLabel::Red => 1,
        crate::ColorLabel::Yellow => 2,
        crate::ColorLabel::Green => 3,
        crate::ColorLabel::Blue => 4,
        crate::ColorLabel::Purple => 5,
    }
}

fn i64_to_color_label(value: i64) -> crate::ColorLabel {
    match value {
        1 => crate::ColorLabel::Red,
        2 => crate::ColorLabel::Yellow,
        3 => crate::ColorLabel::Green,
        4 => crate::ColorLabel::Blue,
        5 => crate::ColorLabel::Purple,
        _ => crate::ColorLabel::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{save_sidecar, Sidecar};

    #[test]
    fn nested_collections_reject_cycles_and_field_search_matches_hierarchies() {
        let dir = tempfile::tempdir().unwrap();
        let photo = dir.path().join("portrait.dng");
        std::fs::write(&photo, b"raw").unwrap();
        let mut db = CatalogDb::open_memory().unwrap();
        db.scan_folder(dir.path(), |_| {}).unwrap();
        let parent = db.create_collection("People").unwrap();
        let child = db.create_collection_in("Family", Some(parent)).unwrap();
        db.add_to_collection(child, &photo).unwrap();
        assert!(db.move_collection(parent, Some(child)).is_err());
        db.set_descriptive_metadata(
            &photo,
            &crate::DescriptiveMetadata {
                keywords: vec!["People|Family|Alice".into()],
                caption: "Blue hour portrait".into(),
                ..Default::default()
            },
        )
        .unwrap();
        db.set_annotations(
            &photo,
            Annotations {
                rating: 4,
                flag: Flag::Pick,
            },
        )
        .unwrap();
        let search = |text: &str| {
            db.photos(
                &LibraryFilter {
                    text: text.into(),
                    ..Default::default()
                },
                LibrarySort::Name,
            )
            .unwrap()
        };
        assert_eq!(
            search("keyword:people|family rating:>=4 -flag:reject caption:\"Blue hour\"").len(),
            1
        );
        assert_eq!(search("keyword:People|Fam").len(), 0);
        assert_eq!(search("type:jpg OR flag:pick").len(), 1);
        db.delete_collection(parent).unwrap();
        let collections = db.collections().unwrap();
        assert_eq!(collections[0].parent_id, None);
        assert_eq!(
            db.photos(&LibraryFilter::default(), LibrarySort::Name)
                .unwrap()[0]
                .collection_ids,
            [child]
        );
    }

    #[test]
    fn stack_membership_and_cover_follow_photo_ids_and_unstack_preserves_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let first = root.join("first.dng");
        let second = root.join("second.dng");
        std::fs::write(&first, b"one").unwrap();
        std::fs::write(&second, b"two").unwrap();
        let db = CatalogDb::open_memory().unwrap();
        let stack = db.stack_photos(&[first.clone(), second.clone()]).unwrap();
        assert!(db.stack_photos(&[first.clone(), second.clone()]).is_err());
        db.set_stack_cover(stack, &second).unwrap();
        let renamed = root.join("renamed.dng");
        std::fs::rename(&second, &renamed).unwrap();
        db.apply_file_moves(&[(second, renamed.clone())]).unwrap();
        let photos = db
            .photos(&LibraryFilter::default(), LibrarySort::Name)
            .unwrap();
        assert!(photos
            .iter()
            .all(|photo| photo.stack_id == Some(stack) && photo.stack_size == 2));
        assert!(
            photos
                .iter()
                .find(|photo| photo.path == renamed)
                .unwrap()
                .stack_cover
        );
        db.dissolve_stack(stack).unwrap();
        assert!(db
            .photos(&LibraryFilter::default(), LibrarySort::Name)
            .unwrap()
            .iter()
            .all(|photo| photo.stack_id.is_none()));
        assert_eq!(std::fs::read(&first).unwrap(), b"one");
        assert_eq!(std::fs::read(&renamed).unwrap(), b"two");
        db.integrity_check().unwrap();
    }

    #[test]
    fn restore_keeps_live_readers_valid_and_preserves_a_rescue_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog.sqlite");
        let mut db = CatalogDb::open(&path).unwrap();
        db.create_collection("Before backup").unwrap();
        let backup = dir.path().join("backup.sqlite");
        db.backup_to(&backup).unwrap();
        db.create_collection("After backup").unwrap();
        let reader = CatalogDb::open(&path).unwrap();
        assert_eq!(reader.collections().unwrap().len(), 2);
        let rescue = dir.path().join("rescue.sqlite");
        db.restore_from(&backup, &rescue).unwrap();
        assert_eq!(reader.collections().unwrap().len(), 1);
        assert_eq!(reader.collections().unwrap()[0].name, "Before backup");
        assert_eq!(
            CatalogDb::open(&rescue)
                .unwrap()
                .collections()
                .unwrap()
                .len(),
            2
        );
        let broken = dir.path().join("broken.sqlite");
        std::fs::write(&broken, b"not a sqlite database").unwrap();
        assert!(db
            .restore_from(&broken, &dir.path().join("should-not-exist.sqlite"))
            .is_err());
        assert_eq!(reader.collections().unwrap().len(), 1);
        assert!(!dir.path().join("should-not-exist.sqlite").exists());
        db.restore_from(&rescue, &dir.path().join("second-rescue.sqlite"))
            .unwrap();
        assert_eq!(reader.collections().unwrap().len(), 2);
    }

    #[test]
    fn derived_provenance_retains_every_source_when_files_move() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let sources = [root.join("one.dng"), root.join("two.dng")];
        let merged = root.join("HDR.tif");
        for path in sources.iter().chain([&merged]) {
            std::fs::write(path, b"pixels").unwrap();
        }
        let db = CatalogDb::open_memory().unwrap();
        db.link_derived_sources(&merged, &sources, "HDR merge")
            .unwrap();
        let moved = root.join("renamed.dng");
        std::fs::rename(&sources[0], &moved).unwrap();
        db.apply_file_moves(&[(sources[0].clone(), moved.clone())])
            .unwrap();
        let photos = db
            .photos(&LibraryFilter::default(), LibrarySort::Name)
            .unwrap();
        let derivative = photos.iter().find(|photo| photo.path == merged).unwrap();
        assert_eq!(derivative.derived_sources, [moved, sources[1].clone()]);
        assert_eq!(derivative.derived_operation.as_deref(), Some("HDR merge"));
        assert!(db
            .link_derived_sources(&merged, std::slice::from_ref(&merged), "Invalid")
            .is_err());
        let photos = db
            .photos(&LibraryFilter::default(), LibrarySort::Name)
            .unwrap();
        assert_eq!(
            photos
                .iter()
                .find(|photo| photo.path == merged)
                .unwrap()
                .derived_sources
                .len(),
            2
        );
    }

    #[test]
    fn moving_and_relinking_preserve_ids_collections_and_companions() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let originals = root.join("originals");
        std::fs::create_dir(&originals).unwrap();
        let raw = originals.join("photo.dng");
        let companion = originals.join("photo-edit.tif");
        std::fs::write(&raw, b"raw pixels").unwrap();
        std::fs::write(&companion, b"rendered pixels").unwrap();
        let mut db = CatalogDb::open(&root.join("catalog.sqlite")).unwrap();
        db.scan_folder(&originals, |_| {}).unwrap();
        db.add_companion(&raw, &companion).unwrap();
        let collection = db.create_collection("Portfolio").unwrap();
        db.add_to_collection(collection, &raw).unwrap();
        let id: i64 = db
            .connection
            .query_row(
                "SELECT id FROM photos WHERE path=?1",
                [raw.to_string_lossy().as_ref()],
                |row| row.get(0),
            )
            .unwrap();
        let renamed = originals.join("renamed.dng");
        crate::workflow::move_photo(&raw, &renamed).unwrap();
        db.apply_file_moves(&[(raw.clone(), renamed.clone())])
            .unwrap();
        db.scan_folder(&originals, |_| {}).unwrap();
        assert_eq!(
            db.companion_source(&companion).unwrap(),
            Some(renamed.clone())
        );
        let actual: i64 = db
            .connection
            .query_row(
                "SELECT photo_id FROM collection_photos WHERE collection_id=?1",
                [collection],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(actual, id);
        let folder = db
            .folders()
            .unwrap()
            .into_iter()
            .find(|folder| folder.path == originals)
            .unwrap();
        let relocated = root.join("relocated");
        std::fs::rename(&originals, &relocated).unwrap();
        assert_eq!(db.relink_folder(folder.id, &relocated).unwrap(), 2);
        assert_eq!(
            db.companion_source(&relocated.join("photo-edit.tif"))
                .unwrap(),
            Some(relocated.join("renamed.dng"))
        );
        db.integrity_check().unwrap();
        let backup = root.join("backup.sqlite");
        db.backup_to(&backup).unwrap();
        let restored = CatalogDb::open(&backup).unwrap();
        restored.integrity_check().unwrap();
        assert_eq!(
            restored
                .photos(&LibraryFilter::default(), LibrarySort::Name)
                .unwrap()
                .len(),
            2
        );
        assert!(db.backup_to(&backup).is_err());
    }

    #[test]
    fn relink_refuses_same_named_wrong_photo_without_changing_catalog() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let old = root.join("old");
        let target = root.join("target");
        std::fs::create_dir(&old).unwrap();
        std::fs::create_dir(&target).unwrap();
        std::fs::write(old.join("photo.dng"), b"original capture").unwrap();
        std::fs::write(target.join("photo.dng"), b"unrelated capture").unwrap();
        let mut db = CatalogDb::open(&root.join("catalog.sqlite")).unwrap();
        db.scan_folder(&old, |_| {}).unwrap();
        let folder = db.folders().unwrap().remove(0);
        assert!(db.relink_folder(folder.id, &target).is_err());
        assert_eq!(db.folders().unwrap()[0].path, old);
    }

    #[cfg(unix)]
    #[test]
    fn relink_handles_aliases_after_original_folder_is_missing() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let alias = root.join("alias");
        let actual = root.join("actual");
        std::fs::create_dir(&actual).unwrap();
        std::os::unix::fs::symlink(&actual, &alias).unwrap();
        std::fs::create_dir(actual.join("old")).unwrap();
        std::fs::write(actual.join("old/photo.dng"), b"raw").unwrap();
        let mut db = CatalogDb::open(&root.join("catalog.sqlite")).unwrap();
        db.scan_folder(&alias.join("old"), |_| {}).unwrap();
        let folder = db.folders().unwrap().remove(0);
        let moved = actual.join("moved");
        std::fs::rename(actual.join("old"), &moved).unwrap();
        assert_eq!(db.relink_folder(folder.id, &moved).unwrap(), 1);
        assert_eq!(
            db.photos(&LibraryFilter::default(), LibrarySort::Name)
                .unwrap()[0]
                .path,
            moved.join("photo.dng")
        );
    }

    #[test]
    fn scan_rebuilds_annotations_from_sidecars() {
        let dir = tempfile::tempdir().expect("tempdir");
        let raw = dir.path().join("photo.3FR");
        std::fs::write(&raw, b"fixture").expect("raw");
        let mut sidecar = Sidecar::default();
        sidecar.annotations.rating = 5;
        sidecar.annotations.flag = Flag::Pick;
        sidecar.edit.tone.exposure_ev = 1.0;
        save_sidecar(&raw, &sidecar).expect("sidecar");
        let mut db = CatalogDb::open_memory().expect("db");
        assert_eq!(db.scan_folder(dir.path(), |_| {}).expect("scan"), 1);
        let photos = db
            .photos(&LibraryFilter::default(), LibrarySort::Name)
            .expect("photos");
        assert_eq!(photos[0].annotations, sidecar.annotations);
        assert!(photos[0].edited);
    }

    #[test]
    fn scan_adopts_foreign_xmp_when_there_is_no_sidecar() {
        let dir = tempfile::tempdir().expect("tempdir");
        let raw = dir.path().join("photo.3FR");
        std::fs::write(&raw, b"fixture").expect("raw");
        // A rating made in another app, with no .spedit of ours.
        crate::xmp::save(
            &raw,
            Annotations {
                rating: 4,
                flag: Flag::None,
            },
        )
        .expect("xmp");
        let mut db = CatalogDb::open_memory().expect("db");
        db.scan_folder(dir.path(), |_| {}).expect("scan");
        let photos = db
            .photos(&LibraryFilter::default(), LibrarySort::Name)
            .expect("photos");
        assert_eq!(photos[0].annotations.rating, 4);
        assert!(!photos[0].edited, "an XMP rating is not an edit");

        // Our own sidecar wins once it exists.
        let mut sidecar = Sidecar::default();
        sidecar.annotations.rating = 1;
        save_sidecar(&raw, &sidecar).expect("sidecar");
        db.scan_folder(dir.path(), |_| {}).expect("rescan");
        let photos = db
            .photos(&LibraryFilter::default(), LibrarySort::Name)
            .expect("photos");
        assert_eq!(photos[0].annotations.rating, 1);
    }

    #[test]
    fn metadata_and_export_presets_round_trip() {
        let dir = tempfile::tempdir().expect("tempdir");
        let raw = dir.path().join("photo.fff");
        std::fs::write(&raw, b"fixture").expect("raw");
        let mut db = CatalogDb::open_memory().expect("db");
        db.scan_folder(dir.path(), |_| {}).expect("scan");
        let meta = ImageMeta {
            camera_model: "X2D 100C".into(),
            captured_at: Some("2026-01-02T03:04:05Z".into()),
            ..Default::default()
        };
        db.set_metadata(&raw, &meta).expect("metadata");
        let photos = db
            .photos(&LibraryFilter::default(), LibrarySort::CaptureTime)
            .expect("photos");
        assert_eq!(photos[0].camera_model, "X2D 100C");
        assert_eq!(photos[0].captured_at, meta.captured_at);

        db.save_export_preset("Web", r#"{"quality":90}"#)
            .expect("preset");
        assert_eq!(
            db.export_presets().expect("presets"),
            vec![("Web".into(), r#"{"quality":90}"#.into())]
        );
    }

    #[test]
    fn external_tiff_is_catalogued_as_a_companion_and_survives_rescan() {
        let dir = tempfile::tempdir().expect("tempdir");
        let raw = dir.path().join("photo.fff");
        let tiff = dir.path().join("photo-external.tif");
        std::fs::write(&raw, b"raw fixture").expect("raw");
        std::fs::write(&tiff, b"tiff fixture").expect("tiff");
        let mut db = CatalogDb::open_memory().expect("db");
        db.scan_folder(dir.path(), |_| {}).expect("scan");
        db.add_companion(&raw, &tiff).expect("companion");
        assert_eq!(
            db.companion_source(&tiff).expect("relation"),
            Some(raw.clone())
        );
        db.scan_folder(dir.path(), |_| {}).expect("rescan");
        let photos = db
            .photos(&LibraryFilter::default(), LibrarySort::Name)
            .expect("photos");
        assert_eq!(photos.len(), 2);
        let derivative = photos
            .iter()
            .find(|photo| photo.path.extension().and_then(|value| value.to_str()) == Some("tif"))
            .expect("derivative");
        assert_eq!(derivative.companion_of.as_deref(), Some(raw.as_path()));
    }

    #[test]
    fn network_scan_hash_never_reads_a_new_or_changed_file() {
        let missing = Path::new("/definitely-not-present/shopphoto.raw");
        assert_eq!(scan_file_hash(missing, true, 10, None), "");
        assert_eq!(
            scan_file_hash(missing, true, 11, Some(&(10, "old".into()))),
            ""
        );
        assert_eq!(
            scan_file_hash(missing, true, 10, Some(&(10, "cached".into()))),
            "cached"
        );
    }

    #[test]
    fn unavailable_folder_does_not_erase_cached_catalog_rows() {
        let dir = tempfile::tempdir().expect("tempdir");
        let share = dir.path().join("share");
        std::fs::create_dir(&share).expect("share");
        std::fs::write(share.join("photo.fff"), b"fixture").expect("raw");
        let mut db = CatalogDb::open_memory().expect("db");
        db.scan_folder(&share, |_| {}).expect("initial scan");
        std::fs::remove_dir_all(&share).expect("disconnect fixture");
        assert!(db.scan_folder(&share, |_| {}).is_err());
        assert_eq!(
            db.photos(&LibraryFilter::default(), LibrarySort::Name)
                .expect("cached photos")
                .len(),
            1
        );
    }

    #[test]
    fn fts_query_quotes_prefix_terms_and_drops_punctuation_only_terms() {
        assert_eq!(fts_query("IMG_0001 sky"), "\"IMG_0001\"* \"sky\"*");
        assert_eq!(fts_query("  - ( AND "), "\"AND\"*");
        assert_eq!(fts_query("say \"hi\""), "\"say\"* \"\"\"hi\"\"\"*");
        assert_eq!(fts_query(""), "");
    }

    #[test]
    fn text_search_matches_prefixes_across_columns_and_tracks_updates() {
        let dir = tempfile::tempdir().expect("tempdir");
        let first = dir.path().join("IMG_0001.fff");
        let second = dir.path().join("B0002063.3FR");
        std::fs::write(&first, b"fixture").expect("raw");
        std::fs::write(&second, b"fixture").expect("raw");
        let mut db = CatalogDb::open_memory().expect("db");
        db.scan_folder(dir.path(), |_| {}).expect("scan");
        let search = |db: &CatalogDb, text: &str| -> Vec<String> {
            let filter = LibraryFilter {
                text: text.into(),
                ..LibraryFilter::default()
            };
            db.photos(&filter, LibrarySort::Name)
                .expect("photos")
                .into_iter()
                .map(|photo| {
                    photo
                        .path
                        .file_name()
                        .expect("name")
                        .to_string_lossy()
                        .into_owned()
                })
                .collect()
        };
        // File names: token prefix, case-insensitive, punctuation split.
        assert_eq!(search(&db, "img_00"), vec!["IMG_0001.fff"]);
        assert_eq!(search(&db, "IMG 0001"), vec!["IMG_0001.fff"]);
        assert_eq!(search(&db, "b00020"), vec!["B0002063.3FR"]);
        assert_eq!(search(&db, "fff"), vec!["IMG_0001.fff"]);
        assert_eq!(search(&db, "nothing-here").len(), 0);
        // Punctuation-only input is not a query: everything matches.
        assert_eq!(search(&db, "-- (").len(), 2);

        // Metadata written later is searchable, and terms AND together.
        let meta = ImageMeta {
            camera_model: "X2D II 100C".into(),
            lens_model: Some("XCD 4/28P".into()),
            ..Default::default()
        };
        db.set_metadata(&second, &meta).expect("metadata");
        assert_eq!(search(&db, "x2d"), vec!["B0002063.3FR"]);
        assert_eq!(search(&db, "28p"), vec!["B0002063.3FR"]);
        assert_eq!(search(&db, "x2d img").len(), 0);
        db.set_descriptive_metadata(
            &first,
            &crate::DescriptiveMetadata {
                caption: "Godafoss at dusk".into(),
                keywords: vec!["Iceland".into(), "waterfall".into()],
                ..Default::default()
            },
        )
        .expect("descriptive");
        assert_eq!(search(&db, "godaf"), vec!["IMG_0001.fff"]);
        assert_eq!(search(&db, "waterfall icel"), vec!["IMG_0001.fff"]);

        // Deleted rows leave the index.
        std::fs::remove_file(&first).expect("remove");
        db.scan_folder(dir.path(), |_| {}).expect("rescan");
        assert_eq!(search(&db, "godaf").len(), 0);
        assert_eq!(search(&db, "b0002063"), vec!["B0002063.3FR"]);
    }

    #[test]
    fn smart_collection_round_trips_the_full_metadata_filter() {
        let db = CatalogDb::open_memory().unwrap();
        let filter = LibraryFilter {
            minimum_rating: 3,
            camera_model: Some("X2D 100C".into()),
            lens_model: Some("XCD 55V".into()),
            iso_min: Some(100),
            iso_max: Some(800),
            date_from: Some("2026-01-01".into()),
            date_to: Some("2026-12-31".into()),
            color_label: Some(crate::ColorLabel::Red),
            ..LibraryFilter::default()
        };
        let id = db.save_smart_collection("Portfolio", &filter).unwrap();
        let collections = db.smart_collections().unwrap();
        assert_eq!(collections.len(), 1);
        assert_eq!(collections[0].id, id);
        assert_eq!(collections[0].filter, filter);
    }

    #[test]
    fn rescan_removes_missing_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        let raw = dir.path().join("gone.dng");
        std::fs::write(&raw, b"fixture").expect("raw");
        let mut db = CatalogDb::open_memory().expect("db");
        db.scan_folder(dir.path(), |_| {}).expect("first scan");
        std::fs::remove_file(&raw).expect("remove fixture");
        db.scan_folder(dir.path(), |_| {}).expect("rescan");
        assert!(db
            .photos(&LibraryFilter::default(), LibrarySort::Name)
            .expect("photos")
            .is_empty());
    }

    #[test]
    fn removing_folder_only_removes_catalog_rows() {
        let dir = tempfile::tempdir().expect("tempdir");
        let raw = dir.path().join("kept.nef");
        std::fs::write(&raw, b"fixture").expect("raw");
        let mut db = CatalogDb::open_memory().expect("db");
        db.scan_folder(dir.path(), |_| {}).expect("scan");
        let folder = db.folders().expect("folders").pop().expect("folder");

        assert!(db.remove_folder(folder.id).expect("remove"));
        assert!(db.folders().expect("folders").is_empty());
        assert!(db
            .photos(&LibraryFilter::default(), LibrarySort::Name)
            .expect("photos")
            .is_empty());
        assert!(raw.exists(), "catalog removal must not delete the RAW");
    }
}
