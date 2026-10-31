//! Persistent delivery tasks hold an immutable edit/metadata snapshot.
use crate::{ExportSettings, PhotoMetadata};
use serde::{Deserialize, Serialize};
use sp_core::EditState;
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
};

/// Held for the entire session using a queue, including between saves. A
/// separate inode survives atomic JSON replacement and releases on crashes.
pub struct QueueLock {
    _file: std::fs::File,
}
impl QueueLock {
    pub fn acquire(path: &Path) -> Result<Self, String> {
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        let parent = parent.canonicalize().map_err(|e| e.to_string())?;
        let name = path.file_name().ok_or("Queue requires a filename")?;
        let mut lock_name = name.to_os_string();
        lock_name.push(".lock");
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(parent.join(lock_name))
            .map_err(|e| e.to_string())?;
        file.try_lock().map_err(|e|format!("Export queue is in use or cannot be locked: {e}. Close the other ShopPhoto process before retrying."))?;
        Ok(Self { _file: file })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskStatus {
    Pending,
    Running,
    Finished,
    Failed(String),
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportTask {
    pub id: u64,
    pub source: PathBuf,
    pub recipe: String,
    pub sequence: usize,
    pub edit: EditState,
    pub metadata: PhotoMetadata,
    pub settings: ExportSettings,
    pub status: TaskStatus,
    pub output: Option<PathBuf>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportQueue {
    pub version: u32,
    pub paused: bool,
    pub tasks: Vec<ExportTask>,
    next_id: u64,
}
impl Default for ExportQueue {
    fn default() -> Self {
        Self {
            version: 1,
            paused: true,
            tasks: Vec::new(),
            next_id: 1,
        }
    }
}
impl ExportQueue {
    pub fn add(&mut self, mut task: ExportTask) -> u64 {
        task.id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        task.status = TaskStatus::Pending;
        task.output = None;
        let id = task.id;
        self.tasks.push(task);
        id
    }
    pub fn start_next(&mut self) -> Option<ExportTask> {
        if self.paused || self.tasks.iter().any(|t| t.status == TaskStatus::Running) {
            return None;
        }
        let task = self
            .tasks
            .iter_mut()
            .find(|t| t.status == TaskStatus::Pending)?;
        task.status = TaskStatus::Running;
        Some(task.clone())
    }
    pub fn finish(&mut self, id: u64, result: Result<PathBuf, String>) {
        if let Some(task) = self.tasks.iter_mut().find(|t| t.id == id) {
            match result {
                Ok(output) => {
                    task.status = TaskStatus::Finished;
                    task.output = Some(output);
                }
                Err(error) => {
                    task.status = TaskStatus::Failed(error);
                }
            }
        }
    }
    pub fn retry(&mut self, id: u64) {
        if let Some(task) = self
            .tasks
            .iter_mut()
            .find(|t| t.id == id && matches!(t.status, TaskStatus::Failed(_)))
        {
            task.status = TaskStatus::Pending;
        }
    }
    pub fn remove(&mut self, id: u64) {
        self.tasks
            .retain(|t| t.id != id || t.status == TaskStatus::Running);
    }
    pub fn save(&self, path: &Path) -> Result<(), String> {
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        let mut file = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
        serde_json::to_writer(&mut file, self).map_err(|e| e.to_string())?;
        file.flush().map_err(|e| e.to_string())?;
        if file.as_file().metadata().map_err(|e| e.to_string())?.len() > 128 * 1024 * 1024 {
            return Err("Export queue exceeds 128 MiB".into());
        }
        file.as_file().sync_all().map_err(|e| e.to_string())?;
        file.persist(path).map_err(|e| e.to_string())?;
        Ok(())
    }
    pub fn load(path: &Path) -> Result<Self, String> {
        let file = match std::fs::File::open(path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(e.to_string()),
        };
        let mut bytes = Vec::new();
        file.take(128 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.len() > 128 * 1024 * 1024 {
            return Err("Export queue exceeds 128 MiB".into());
        }
        let mut queue: Self = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        if queue.version != 1 {
            return Err("Export queue has an unsupported version".into());
        }
        let mut ids = std::collections::HashSet::new();
        if queue
            .tasks
            .iter()
            .any(|task| task.id == 0 || task.id == u64::MAX || !ids.insert(task.id))
        {
            return Err("Export queue has invalid or duplicate task identifiers".into());
        }
        // Reopening the app never starts writes before the photographer
        // chooses Resume. Interrupted work is retryable, not marked done.
        queue.paused = true;
        for task in &mut queue.tasks {
            if task.status == TaskStatus::Running {
                task.status = TaskStatus::Pending;
            }
            task.edit = std::mem::take(&mut task.edit).clamped();
        }
        queue.next_id = queue.next_id.max(
            queue
                .tasks
                .iter()
                .map(|t| t.id)
                .max()
                .unwrap_or(0)
                .saturating_add(1),
        );
        Ok(queue)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn queue_has_one_writer_and_releases_after_owner_exits() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("queue.json");
        let owner = QueueLock::acquire(&path).unwrap();
        assert!(QueueLock::acquire(&path).is_err());
        ExportQueue::default().save(&path).unwrap();
        assert!(
            QueueLock::acquire(&path).is_err(),
            "Atomic save must retain ownership"
        );
        drop(owner);
        assert!(QueueLock::acquire(&path).is_ok());
    }
    fn task() -> ExportTask {
        ExportTask {
            id: 0,
            source: "photo.tif".into(),
            recipe: "Print".into(),
            sequence: 0,
            edit: EditState::default(),
            metadata: PhotoMetadata {
                caption: "Captured caption".into(),
                ..Default::default()
            },
            settings: ExportSettings::default(),
            status: TaskStatus::Pending,
            output: None,
        }
    }
    #[test]
    fn queue_restarts_paused_with_snapshot_and_retries_failed_jobs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("queue.json");
        let mut queue = ExportQueue::default();
        let mut first = task();
        first.edit.tone.exposure_ev = 1.25;
        let id = queue.add(first);
        queue.paused = false;
        assert_eq!(queue.start_next().unwrap().id, id);
        assert!(queue.start_next().is_none());
        queue.save(&path).unwrap();
        let mut restored = ExportQueue::load(&path).unwrap();
        assert!(restored.paused);
        assert!(restored.start_next().is_none());
        restored.paused = false;
        let task = restored.start_next().unwrap();
        assert_eq!(task.edit.tone.exposure_ev, 1.25);
        assert_eq!(task.metadata.caption, "Captured caption");
        restored.finish(id, Err("disk unavailable".into()));
        restored.retry(id);
        assert!(restored.start_next().is_some());
        restored.remove(id);
        assert_eq!(restored.tasks.len(), 1);
        restored.finish(id, Ok("result.jpg".into()));
        restored.remove(id);
        assert!(restored.tasks.is_empty());
    }
}
