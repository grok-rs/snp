//! Advisory file locks ([`FileLock`]) over `std::fs::File::{lock, lock_shared, try_lock, try_lock_shared}`
//! (`flock(2)` on Unix, `LockFileEx` on Windows): pre-commit's `store.lock()` / `file_lock.lock()` with a
//! cancellable, bounded wait (ARCHITECTURE §5.5).
//!
//! W0 working baseline (ADR-071, Unix-exact); WP-15 completes it without changing a signature.
//!
//! Owner: WP-15.

use std::fs::{File, OpenOptions, TryLockError};
use std::io;
use std::path::Path;
use std::time::{Duration, Instant};

use snp_core::cancel::CancelToken;

/// How often a blocked [`FileLock::acquire`] retries `try_lock` (and observes cancellation).
const POLL: Duration = Duration::from_millis(50);

/// Lock mode.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum LockMode {
    /// `File::lock_shared`.
    Shared,
    /// `File::lock`.
    Exclusive,
}

/// A held lock; released on drop (closing the file releases the lock).
#[derive(Debug)]
pub struct FileLock {
    /// The open, locked lock file.
    file: File,
}

/// Opens (creating it when missing, never truncating) the lock file.
fn open_lock_file(path: &Path) -> io::Result<File> {
    let mut o = OpenOptions::new();
    o.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    o.open(path)
}

/// One non-blocking attempt on an already open file: `Ok(false)` on contention.
fn try_lock(file: &File, mode: LockMode) -> io::Result<bool> {
    let res = match mode {
        LockMode::Shared => file.try_lock_shared(),
        LockMode::Exclusive => file.try_lock(),
    };
    match res {
        Ok(()) => Ok(true),
        Err(TryLockError::WouldBlock) => Ok(false),
        Err(TryLockError::Error(e)) => Err(e),
    }
}

impl FileLock {
    /// Non-blocking attempt; `Ok(None)` on contention. The lock file is created (mode 0600 on Unix) when missing.
    pub fn try_acquire(path: &Path, mode: LockMode) -> io::Result<Option<Self>> {
        let file = open_lock_file(path)?;
        Ok(try_lock(&file, mode)?.then_some(Self { file }))
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
        let file = open_lock_file(path)?;
        if try_lock(&file, mode)? {
            return Ok(Self { file });
        }
        on_wait();
        // An unrepresentable deadline (absurd timeout) means "forever".
        let deadline = timeout.and_then(|t| Instant::now().checked_add(t));
        loop {
            if cancel.is_cancelled() {
                return Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "lock wait cancelled",
                ));
            }
            let tick = match deadline {
                Some(d) => {
                    let left = d.saturating_duration_since(Instant::now());
                    if left.is_zero() {
                        return Err(io::Error::new(
                            io::ErrorKind::TimedOut,
                            "timed out waiting for the lock",
                        ));
                    }
                    left.min(POLL)
                }
                None => POLL,
            };
            // Returns early when the token is cancelled; the loop head reports it.
            if !cancel.wait_timeout(tick) && try_lock(&file, mode)? {
                return Ok(Self { file });
            }
        }
    }

    /// The locked file.
    pub fn file(&self) -> &File {
        &self.file
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lock_path() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join(".lock");
        (dir, p)
    }

    #[test]
    fn exclusive_excludes_and_drop_releases() {
        let (_d, p) = lock_path();
        let a = FileLock::try_acquire(&p, LockMode::Exclusive)
            .unwrap()
            .unwrap();
        assert!(
            FileLock::try_acquire(&p, LockMode::Exclusive)
                .unwrap()
                .is_none()
        );
        assert!(
            FileLock::try_acquire(&p, LockMode::Shared)
                .unwrap()
                .is_none()
        );
        drop(a);
        assert!(
            FileLock::try_acquire(&p, LockMode::Exclusive)
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn shared_locks_coexist() {
        let (_d, p) = lock_path();
        let a = FileLock::try_acquire(&p, LockMode::Shared).unwrap();
        let b = FileLock::try_acquire(&p, LockMode::Shared).unwrap();
        assert!(a.is_some() && b.is_some());
        assert!(
            FileLock::try_acquire(&p, LockMode::Exclusive)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn acquire_uncontended_does_not_call_on_wait() {
        let (_d, p) = lock_path();
        let mut waited = 0;
        let l = FileLock::acquire(
            &p,
            LockMode::Exclusive,
            &mut || waited += 1,
            &CancelToken::new(),
            Some(Duration::ZERO),
        )
        .unwrap();
        assert_eq!(waited, 0);
        assert!(l.file().metadata().unwrap().is_file());
    }

    #[test]
    fn acquire_times_out_after_one_on_wait() {
        let (_d, p) = lock_path();
        let _held = FileLock::try_acquire(&p, LockMode::Exclusive)
            .unwrap()
            .unwrap();
        let mut waited = 0;
        let start = Instant::now();
        let e = FileLock::acquire(
            &p,
            LockMode::Shared,
            &mut || waited += 1,
            &CancelToken::new(),
            Some(Duration::from_millis(120)),
        )
        .unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::TimedOut);
        assert_eq!(waited, 1);
        assert!(start.elapsed() >= Duration::from_millis(120));
    }

    #[test]
    fn acquire_observes_cancellation() {
        let (_d, p) = lock_path();
        let _held = FileLock::try_acquire(&p, LockMode::Exclusive)
            .unwrap()
            .unwrap();
        let cancel = CancelToken::new();
        let c2 = cancel.clone();
        let t = std::thread::Builder::new()
            .spawn(move || {
                std::thread::sleep(Duration::from_millis(80));
                c2.cancel(snp_core::cancel::CancelReason::Interrupt);
            })
            .unwrap();
        let e = FileLock::acquire(&p, LockMode::Exclusive, &mut || {}, &cancel, None).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::Interrupted);
        t.join().unwrap();
    }

    #[test]
    fn acquire_waits_for_release() {
        let (_d, p) = lock_path();
        let held = FileLock::try_acquire(&p, LockMode::Exclusive)
            .unwrap()
            .unwrap();
        let t = std::thread::Builder::new()
            .spawn(move || {
                std::thread::sleep(Duration::from_millis(80));
                drop(held);
            })
            .unwrap();
        let l = FileLock::acquire(
            &p,
            LockMode::Exclusive,
            &mut || {},
            &CancelToken::new(),
            Some(Duration::from_secs(10)),
        );
        assert!(l.is_ok());
        t.join().unwrap();
    }
}
