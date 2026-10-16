use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

/// Shared state of one long-running operation: a cancel flag plus progress counters that worker
/// threads bump without locking. Progress is sent as periodic snapshots, never one event per
/// record.
#[derive(Default)]
pub struct Job {
    cancelled: AtomicBool,
    done: AtomicU64,
    total: AtomicU64,
    found: AtomicU64,
    bytes: AtomicU64,
    stage: Mutex<String>,
    current: Mutex<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProgressSnapshot {
    pub stage: String,
    pub done: u64,
    pub total: u64,
    pub found: u64,
    pub bytes: u64,
    pub current: String,
}

impl Job {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }

    pub fn set_current(&self, s: &str) {
        if let Some(mut g) = self.current.try_lock() {
            g.clear();
            g.push_str(s);
        }
    }

    pub fn add_bytes(&self, n: u64) {
        self.bytes.fetch_add(n, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> ProgressSnapshot {
        ProgressSnapshot {
            stage: self.stage.lock().clone(),
            done: self.done.load(Ordering::Relaxed),
            total: self.total.load(Ordering::Relaxed),
            found: self.found.load(Ordering::Relaxed),
            bytes: self.bytes.load(Ordering::Relaxed),
            current: self.current.lock().clone(),
        }
    }
}

impl unlost_formats::Progress for Job {
    fn stage(&self, stage: &str) {
        *self.stage.lock() = stage.to_string();
    }
    fn set_total(&self, total: u64) {
        self.total.store(total, Ordering::Relaxed);
        self.done.store(0, Ordering::Relaxed);
    }
    fn advance(&self, n: u64) {
        self.done.fetch_add(n, Ordering::Relaxed);
    }
    fn found(&self, n: u64) {
        self.found.fetch_add(n, Ordering::Relaxed);
    }
    fn cancelled(&self) -> bool {
        self.is_cancelled()
    }
}
