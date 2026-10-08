//! Advisory file locks ([`FileLock`]).
//!
//! Owner: WP-15.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-15")]
#![expect(unused_variables, reason = "W0 stub parameters, removed by WP-15")]

use std::fs::File;
use std::io;
use std::path::Path;
use std::time::Duration;

use snp_core::cancel::CancelToken;

/// Lock mode.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum LockMode {
    /// `File::lock_shared`.
    Shared,
    /// `File::lock`.
    Exclusive,
}

/// A held lock; released on drop.
#[derive(Debug)]
pub struct FileLock {
    /// W0 placeholder; the baseline stores the locked `std::fs::File`.
    _priv: (),
}

impl FileLock {
    /// Non-blocking attempt; `Ok(None)` on contention.
    pub fn try_acquire(path: &Path, mode: LockMode) -> io::Result<Option<Self>> {
        todo!()
    }

    /// try first; on contention call `on_wait()` once, then wait, polling `try_lock` every 50 ms so `cancel`
    /// is observed; Err(Interrupted) when cancelled; Err(TimedOut) after `timeout` (None = forever).
    pub fn acquire(
        path: &Path,
        mode: LockMode,
        on_wait: &mut dyn FnMut(),
        cancel: &CancelToken,
        timeout: Option<Duration>,
    ) -> io::Result<Self> {
        todo!()
    }

    /// The locked file.
    pub fn file(&self) -> &File {
        todo!()
    }
}
