//! Latest-first decode scheduling and a byte-weighted working-memory gate.
use std::{
    collections::VecDeque,
    sync::{Arc, Condvar, Mutex},
};

pub const MIB: usize = 1024 * 1024;

#[derive(Clone, Copy)]
pub struct MemoryBudget {
    pub preview_bytes: usize,
    pub decode_bytes: usize,
    pub thumbnail_bytes: usize,
}

impl MemoryBudget {
    pub fn from_mebibytes(mb: usize) -> Self {
        let bytes = mb.clamp(256, 8192) * MIB;
        Self {
            preview_bytes: (bytes / 8).min(128 * MIB),
            decode_bytes: bytes * 3 / 4,
            thumbnail_bytes: (bytes / 8).min(128 * MIB),
        }
    }

    pub fn configured() -> Self {
        Self::from_mebibytes(
            std::env::var("SHOPPHOTO_MEMORY_MB")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(1024),
        )
    }
}

struct Queue<T> {
    open: Option<T>,
    compare: VecDeque<T>,
    visible: VecDeque<T>,
    neighbors: VecDeque<T>,
    stopped: bool,
}

pub struct WorkQueue<T> {
    state: Mutex<Queue<T>>,
    wake: Condvar,
}

impl<T> Default for WorkQueue<T> {
    fn default() -> Self {
        Self {
            state: Mutex::new(Queue {
                open: None,
                compare: VecDeque::new(),
                visible: VecDeque::new(),
                neighbors: VecDeque::new(),
                stopped: false,
            }),
            wake: Condvar::new(),
        }
    }
}

impl<T> WorkQueue<T> {
    /// An open replaces the previous queued open; running work uses generations.
    pub fn open(&self, job: T) {
        if let Ok(mut state) = self.state.lock() {
            state.open = Some(job);
            self.wake.notify_all();
        }
    }

    pub fn compare(&self, job: T) {
        if let Ok(mut state) = self.state.lock() {
            state.compare.push_back(job);
            self.wake.notify_all();
        }
    }

    pub fn visible(&self, job: T, same: impl Fn(&T) -> bool) {
        if let Ok(mut state) = self.state.lock() {
            state.visible.retain(|queued| !same(queued));
            // Bound pending work even when a frontend supplies a huge visible set.
            if state.visible.len() >= 512 {
                state.visible.pop_back();
            }
            state.visible.push_front(job);
            self.wake.notify_all();
        }
    }

    pub fn clear_visible(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.visible.clear();
        }
    }

    pub fn neighbors(&self, jobs: impl IntoIterator<Item = T>) {
        if let Ok(mut state) = self.state.lock() {
            state.neighbors = jobs.into_iter().take(2).collect();
            self.wake.notify_all();
        }
    }

    pub fn next(&self) -> Option<T> {
        self.next_kind(false)
    }

    pub fn next_foreground(&self) -> Option<T> {
        self.next_kind(true)
    }

    fn next_kind(&self, foreground_only: bool) -> Option<T> {
        let mut state = self.state.lock().ok()?;
        loop {
            if state.stopped {
                return None;
            }
            if let Some(job) = state
                .open
                .take()
                .or_else(|| state.compare.pop_front())
                .or_else(|| {
                    if foreground_only {
                        None
                    } else {
                        state.visible.pop_front()
                    }
                })
                .or_else(|| {
                    if foreground_only {
                        None
                    } else {
                        state.neighbors.pop_front()
                    }
                })
            {
                return Some(job);
            }
            state = self.wake.wait(state).ok()?;
        }
    }

    pub fn stop(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.stopped = true;
            state.open = None;
            state.compare.clear();
            state.visible.clear();
            state.neighbors.clear();
            self.wake.notify_all();
        }
    }
}

pub struct MemoryGate {
    limit: usize,
    used: Mutex<(usize, usize)>,
    wake: Condvar,
}

pub struct MemoryLease {
    gate: Arc<MemoryGate>,
    bytes: usize,
}

impl MemoryGate {
    pub fn new(limit: usize) -> Arc<Self> {
        Arc::new(Self {
            limit,
            used: Mutex::new((0, 0)),
            wake: Condvar::new(),
        })
    }

    /// Oversized decodes run alone. Never block while holding application state.
    pub fn acquire(self: &Arc<Self>, estimate: usize) -> Option<MemoryLease> {
        self.acquire_kind(estimate, false)
    }

    pub fn acquire_background(self: &Arc<Self>, estimate: usize) -> Option<MemoryLease> {
        self.acquire_kind(estimate, true)
    }

    fn acquire_kind(self: &Arc<Self>, estimate: usize, background: bool) -> Option<MemoryLease> {
        let bytes = estimate.min(self.limit).max(1);
        let mut state = self.used.lock().ok()?;
        if !background {
            state.1 += 1;
        }
        while state.0 > self.limit.saturating_sub(bytes) || (background && state.1 > 0) {
            state = self.wake.wait(state).ok()?;
        }
        if !background {
            state.1 -= 1;
        }
        state.0 += bytes;
        Some(MemoryLease {
            gate: self.clone(),
            bytes,
        })
    }
}

impl Drop for MemoryLease {
    fn drop(&mut self) {
        if let Ok(mut used) = self.gate.used.lock() {
            used.0 = used.0.saturating_sub(self.bytes);
            self.gate.wake.notify_all();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opens_replace_backlog_and_visible_work_precedes_neighbors() {
        let queue = WorkQueue::default();
        queue.neighbors([1, 2, 3]);
        queue.visible(4, |old| *old == 4);
        queue.visible(4, |old| *old == 4);
        queue.compare(5);
        queue.open(6);
        queue.open(7);
        for expected in [7, 5, 4, 1, 2] {
            assert_eq!(queue.next(), Some(expected));
        }
        queue.stop();
        assert_eq!(queue.next(), None);
    }

    #[test]
    fn oversized_decode_runs_alone_and_releases_waiters() {
        let gate = MemoryGate::new(100);
        let first = gate.acquire(200).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let _lease = gate.acquire(1).unwrap();
            tx.send(()).unwrap();
        });
        assert!(rx
            .recv_timeout(std::time::Duration::from_millis(20))
            .is_err());
        drop(first);
        rx.recv_timeout(std::time::Duration::from_secs(1)).unwrap();
        worker.join().unwrap();
    }
}
