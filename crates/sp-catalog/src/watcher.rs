//! Debounced folder invalidation. Event kinds are intentionally ignored;
//! callers re-stat/rescan after the 500 ms quiet period.

use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use sp_core::SpError;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub struct FolderWatcher {
    _watcher: RecommendedWatcher,
    events: std::sync::mpsc::Receiver<PathBuf>,
    pending_since: Option<Instant>,
}

impl FolderWatcher {
    pub fn new(path: &Path) -> Result<Self, SpError> {
        let (sender, events) = std::sync::mpsc::channel();
        let mut watcher =
            notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                if let Ok(event) = event {
                    for path in event.paths {
                        let _ = sender.send(path);
                    }
                }
            })
            .map_err(|e| SpError::Catalog(e.to_string()))?;
        watcher
            .watch(path, RecursiveMode::Recursive)
            .map_err(|e| SpError::Catalog(e.to_string()))?;
        Ok(Self {
            _watcher: watcher,
            events,
            pending_since: None,
        })
    }

    /// True once events have been quiet for at least 500 ms.
    pub fn poll_rescan(&mut self) -> bool {
        let had_event = self.events.try_iter().count() > 0;
        if had_event {
            self.pending_since = Some(Instant::now());
        }
        if self
            .pending_since
            .is_some_and(|when| when.elapsed() >= Duration::from_millis(500))
        {
            self.pending_since = None;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_poll_drains_the_whole_burst() {
        let (sender, events) = std::sync::mpsc::channel();
        let mut watcher = FolderWatcher {
            _watcher: notify::recommended_watcher(|_: notify::Result<notify::Event>| {}).unwrap(),
            events,
            pending_since: None,
        };
        for _ in 0..1000 {
            sender.send(PathBuf::from("photo.3fr")).unwrap();
        }
        assert!(!watcher.poll_rescan());
        assert!(watcher.events.try_recv().is_err());
        watcher.pending_since = Some(Instant::now() - Duration::from_millis(501));
        assert!(watcher.poll_rescan());
        assert!(!watcher.poll_rescan());
    }
}
