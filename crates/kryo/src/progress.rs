//! Shared counters a window reads while the work runs on other threads.

use anyhow::Result;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::Mutex;
use std::time::Duration;

/// Returned through `anyhow` when the person pressed Cancel.
#[derive(Debug)]
pub struct Cancelled;

impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Cancelled.")
    }
}

impl std::error::Error for Cancelled {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Stage {
    Idle,
    Scanning,
    Reading,
    Compressing,
    Writing,
    Testing,
    Checking,
    Unpacking,
    Linking,
    Verifying,
    Finishing,
}

impl Stage {
    fn from_u8(v: u8) -> Self {
        use Stage::*;
        [Idle, Scanning, Reading, Compressing, Writing, Testing, Checking, Unpacking, Linking, Verifying, Finishing]
            .get(v as usize)
            .copied()
            .unwrap_or(Idle)
    }

    pub fn id(self) -> &'static str {
        match self {
            Stage::Idle => "idle",
            Stage::Scanning => "scanning",
            Stage::Reading => "reading",
            Stage::Compressing => "compressing",
            Stage::Writing => "writing",
            Stage::Testing => "testing",
            Stage::Checking => "checking",
            Stage::Unpacking => "unpacking",
            Stage::Linking => "linking",
            Stage::Verifying => "verifying",
            Stage::Finishing => "finishing",
        }
    }
}

#[derive(Default)]
pub struct Progress {
    stage: AtomicU8,
    pub done: AtomicU64,
    pub total: AtomicU64,
    /// Compressed bytes so far, while compressing.
    pub packed: AtomicU64,
    /// The whole game folder, known once it has been read.
    pub original: AtomicU64,
    /// Bytes the dedupe removed before compressing.
    pub repeated: AtomicU64,
    pub blocks_done: AtomicU64,
    pub blocks_total: AtomicU64,
    cancel: AtomicBool,
    pause: AtomicBool,
    detail: Mutex<String>,
}

impl Progress {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn start(&self, stage: Stage, total: u64) {
        self.stage.store(stage as u8, Ordering::SeqCst);
        self.total.store(total, Ordering::SeqCst);
        self.done.store(0, Ordering::SeqCst);
    }

    pub fn stage(&self) -> Stage {
        Stage::from_u8(self.stage.load(Ordering::SeqCst))
    }

    pub fn advance(&self, bytes: u64) {
        self.done.fetch_add(bytes, Ordering::Relaxed);
    }

    pub fn set_detail(&self, text: &str) {
        if let Ok(mut d) = self.detail.lock() {
            d.clear();
            d.push_str(text);
        }
    }

    pub fn detail(&self) -> String {
        self.detail.lock().map(|d| d.clone()).unwrap_or_default()
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }

    pub fn set_paused(&self, paused: bool) {
        self.pause.store(paused, Ordering::SeqCst);
    }

    pub fn is_paused(&self) -> bool {
        self.pause.load(Ordering::SeqCst)
    }

    /// Called between pieces of work: waits out a pause, stops on a cancel.
    pub fn checkpoint(&self) -> Result<()> {
        loop {
            if self.is_cancelled() {
                return Err(Cancelled.into());
            }
            if !self.is_paused() {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}
