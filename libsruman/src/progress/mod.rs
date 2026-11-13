//! Progress reporting and cancellation for long operations.
//!
//! The library stays synchronous (D2): long calls take a `&mut dyn
//! Progress` to report through and a [`CancelFlag`] to stop on. sruman
//! draws progress bars from it; gaki can forward it to its UI from a
//! `spawn_blocking` task, and cancel from another thread.

use std::{
    io::{self, Read},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

#[cfg(test)]
mod tests;

/// The stage an item is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Download,
    Verify,
    Extract,
    Commit,
}

impl Phase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Download => "downloading",
            Self::Verify => "verifying",
            Self::Extract => "extracting",
            Self::Commit => "installing",
        }
    }
}

/// Receives progress. Calls for one item come in order: `begin`, then per
/// phase a `phase` followed by `advance`s, then `end`.
pub trait Progress {
    /// Work on `item` (e.g. `sniper 3.0.20261110.268881`) starts.
    fn begin(&mut self, item: &str);
    /// A new phase of the current item; `total` in bytes if known.
    fn phase(&mut self, phase: Phase, total: Option<u64>);
    /// `n` more bytes of the current phase are done.
    fn advance(&mut self, n: u64);
    /// The item is finished (successfully or not).
    fn end(&mut self, ok: bool);
}

/// Ignores everything.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoProgress;

impl Progress for NoProgress {
    fn begin(&mut self, _: &str) {}
    fn phase(&mut self, _: Phase, _: Option<u64>) {}
    fn advance(&mut self, _: u64) {}
    fn end(&mut self, _: bool) {}
}

/// Set from any thread (a Ctrl-C handler, a UI button) to stop an
/// operation at its next check. Clones share the flag.
#[derive(Debug, Clone, Default)]
pub struct CancelFlag(Arc<AtomicBool>);

/// Returned when a [`CancelFlag`] stopped an operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("cancelled")]
pub struct Cancelled;

impl CancelFlag {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }

    pub fn check(&self) -> Result<(), Cancelled> {
        if self.is_cancelled() {
            Err(Cancelled)
        } else {
            Ok(())
        }
    }
}

/// Whether an I/O error came from a [`CancelFlag`] (see [`ProgressReader`]).
pub fn is_cancelled(error: &io::Error) -> bool {
    error
        .get_ref()
        .is_some_and(|inner| inner.downcast_ref::<Cancelled>().is_some())
}

/// A reader that reports every read to a [`Progress`] and fails with an
/// error wrapping [`Cancelled`] (see [`is_cancelled`]) once
/// the flag is set, so a stream copy stops promptly.
pub struct ProgressReader<'a, R> {
    inner: R,
    progress: &'a mut dyn Progress,
    cancel: &'a CancelFlag,
}

impl<'a, R: Read> ProgressReader<'a, R> {
    pub fn new(inner: R, progress: &'a mut dyn Progress, cancel: &'a CancelFlag) -> Self {
        Self {
            inner,
            progress,
            cancel,
        }
    }
}

impl<R: Read> Read for ProgressReader<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.cancel.is_cancelled() {
            return Err(io::Error::other(Cancelled));
        }
        let n = self.inner.read(buf)?;
        if n > 0 {
            self.progress.advance(n as u64);
        }
        Ok(n)
    }
}
