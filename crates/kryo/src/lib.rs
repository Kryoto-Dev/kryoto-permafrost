//! The `.kryo` archive format behind Kryoto Permafrost.
//!
//! `freeze` turns a game folder into a repack. `thaw` installs one. With the default
//! `freeze` feature off, this crate is only what a setup needs to install.

pub mod codec;
pub mod format;
pub mod progress;
pub mod thaw;
pub mod unwrap;

#[cfg(feature = "freeze")]
pub mod classify;
#[cfg(feature = "freeze")]
pub mod freeze;
#[cfg(feature = "freeze")]
pub mod scan;

use anyhow::Result;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex};

/// Runs `job(0..count)` on `threads` threads. The first error stops the rest and is returned.
pub fn parallel<F>(threads: usize, count: usize, job: F) -> Result<()>
where
    F: Fn(usize) -> Result<()> + Sync,
{
    let next = AtomicUsize::new(0);
    let failed = AtomicBool::new(false);
    let error = Mutex::new(None);
    std::thread::scope(|scope| {
        for _ in 0..threads.min(count).max(1) {
            scope.spawn(|| loop {
                let index = next.fetch_add(1, Ordering::SeqCst);
                if index >= count || failed.load(Ordering::SeqCst) {
                    return;
                }
                if let Err(e) = job(index) {
                    failed.store(true, Ordering::SeqCst);
                    error.lock().unwrap().get_or_insert(e);
                    return;
                }
            });
        }
    });
    match error.into_inner().unwrap() {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

/// Lets work run side by side only while it fits in a memory budget.
pub struct MemoryGate {
    limit: u64,
    used: Mutex<u64>,
    freed: Condvar,
}

pub struct Held<'a> {
    gate: &'a MemoryGate,
    amount: u64,
}

impl MemoryGate {
    pub fn new(limit: u64) -> Self {
        MemoryGate { limit, used: Mutex::new(0), freed: Condvar::new() }
    }

    /// Waits for room. Something too big for the budget still runs, alone.
    pub fn hold(&self, amount: u64) -> Held<'_> {
        let amount = amount.min(self.limit);
        let mut used = self.used.lock().unwrap();
        while *used + amount > self.limit {
            used = self.freed.wait(used).unwrap();
        }
        *used += amount;
        Held { gate: self, amount }
    }
}

impl Drop for Held<'_> {
    fn drop(&mut self) {
        *self.gate.used.lock().unwrap() -= self.amount;
        self.gate.freed.notify_all();
    }
}
