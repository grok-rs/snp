//! The per-worktree lock `<git_dir>/snp-stash.lock` with its nested-run token (ARCHITECTURE §4.2 step 1, ADR-026,
//! ADR-067, D-DATA-13, D-EXT-04).
//!
//! The lock is an exclusive advisory lock (`flock` / `LockFileEx`) on the lock file. While it is held the file holds
//! a random token (on Windows, whose byte-range lock makes the locked file unreadable for other handles, a sibling
//! `snp-stash.lock.token`), exported to hooks as `_SNP_STASH_LOCK`. A nested run skips the lock only when its token
//! equals the file's token **and** its own non-blocking attempt fails (the lock is really held); a token left in a
//! released file exempts nobody, and release truncates it.
//!
//! Owner: WP-23.

use std::ffi::OsStr;
use std::hash::{BuildHasher, Hash, Hasher};
use std::io::{self, Seek, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use snp_core::cancel::{CancelReason, CancelToken};
use snp_core::failure::Failure;
use snp_core::output::Printer;
use snp_os::lock::{FileLock, LockMode};

use crate::messages;

/// The variable that carries the token to nested runs (D-EXT-04).
pub(crate) const TOKEN_ENV: &str = "_SNP_STASH_LOCK";

/// The lock file name in the git dir.
pub(crate) const LOCK_NAME: &str = "snp-stash.lock";

/// The default `SNP_STASH_LOCK_TIMEOUT`.
pub(crate) const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);
/// The upper bound of `SNP_STASH_LOCK_TIMEOUT`.
pub(crate) const MAX_TIMEOUT: Duration = Duration::from_secs(3600);

/// `n` random bytes as lower-case hex: std's per-process random hasher keys (seeded from the OS) mixed with the pid,
/// the time and a counter. Not cryptographic; the token only distinguishes runs.
pub(crate) fn random_hex(n: usize) -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let mut out = String::new();
    let mut produced = 0_usize;
    while produced < n {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        COUNTER.fetch_add(1, Ordering::Relaxed).hash(&mut h);
        std::process::id().hash(&mut h);
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos())
            .hash(&mut h);
        std::thread::current().id().hash(&mut h);
        for b in h.finish().to_le_bytes() {
            if produced >= n {
                break;
            }
            for nibble in [b >> 4, b & 0xf] {
                out.push(char::from_digit(u32::from(nibble), 16).unwrap_or('0'));
            }
            produced = produced.saturating_add(1);
        }
    }
    out
}

/// The lock file of a git dir.
pub(crate) fn lock_path(git_dir: &Path) -> PathBuf {
    git_dir.join(LOCK_NAME)
}

/// Where the token is stored (the lock file itself, or its Windows sibling).
fn token_path(lock: &Path) -> PathBuf {
    if cfg!(windows) {
        let mut s = lock.as_os_str().to_os_string();
        s.push(".token");
        PathBuf::from(s)
    } else {
        lock.to_path_buf()
    }
}

/// The token currently stored for `lock` (trimmed; empty when none).
fn read_token(lock: &Path) -> String {
    snp_os::fs::read_capped(&token_path(lock), 1024)
        .map_or_default(|b| String::from_utf8_lossy(&b).trim().to_owned())
}

/// `SNP_STASH_LOCK_TIMEOUT` (seconds, `float()` semantics): default 60 s, at most 1 h; anything invalid → 60 s.
pub(crate) fn timeout_from(v: Option<&OsStr>) -> Duration {
    v.and_then(OsStr::to_str)
        .and_then(|s| s.trim().parse::<f64>().ok())
        .filter(|secs| secs.is_finite() && *secs >= 0.0)
        .and_then(|secs| Duration::try_from_secs_f64(secs.min(MAX_TIMEOUT.as_secs_f64())).ok())
        .unwrap_or(DEFAULT_TIMEOUT)
}

/// A held worktree lock; releasing truncates the token.
#[derive(Debug)]
pub(crate) struct WorktreeLock {
    lock: Option<FileLock>,
    path: PathBuf,
    token: String,
}

impl WorktreeLock {
    /// Writes a fresh token into the held lock (best effort: a failure only disables the nested-run exemption).
    fn publish(lock: FileLock, path: PathBuf) -> Self {
        let token = random_hex(16);
        let written = if cfg!(windows) {
            std::fs::write(token_path(&path), token.as_bytes())
        } else {
            write_token(&lock, token.as_bytes())
        };
        Self {
            lock: Some(lock),
            path,
            token: if written.is_ok() {
                token
            } else {
                String::new()
            },
        }
    }

    /// The token (empty when it could not be written).
    pub(crate) fn token(&self) -> &str {
        &self.token
    }

    /// Truncates the token and releases the lock.
    pub(crate) fn release(mut self) {
        self.release_inner();
    }

    fn release_inner(&mut self) {
        if let Some(lock) = self.lock.take() {
            let _ignored = if cfg!(windows) {
                std::fs::write(token_path(&self.path), b"")
            } else {
                lock.file().set_len(0)
            };
            drop(lock);
        }
    }
}

impl Drop for WorktreeLock {
    fn drop(&mut self) {
        self.release_inner();
    }
}

fn write_token(lock: &FileLock, token: &[u8]) -> io::Result<()> {
    let mut f = lock.file();
    f.set_len(0)?;
    f.rewind()?;
    f.write_all(token)?;
    f.flush()
}

/// How the run holds the worktree.
#[derive(Debug)]
pub(crate) enum Acquired {
    /// This run holds the lock.
    Held(WorktreeLock),
    /// A nested run: the outer run holds the lock and its token matched.
    Nested(String),
}

/// Acquires the worktree lock of `git_dir` (ARCHITECTURE §4.2 step 1).
///
/// `env_token` is this process's `_SNP_STASH_LOCK`. On contention without a matching token: one stderr notice, then
/// wait up to `timeout`, observing `cancel` (→ `Interrupted`); a timeout is a `FatalError`.
pub(crate) fn acquire(
    git_dir: &Path,
    env_token: Option<&OsStr>,
    printer: &Printer,
    cancel: &CancelToken,
    timeout: Duration,
) -> Result<Acquired, Failure> {
    let path = lock_path(git_dir);
    let io_err = |e: &io::Error| Failure::from_io(e, Some(path.as_os_str()), None);
    if let Some(lock) = FileLock::try_acquire(&path, LockMode::Exclusive).map_err(|e| io_err(&e))? {
        return Ok(Acquired::Held(WorktreeLock::publish(lock, path)));
    }
    // Held by someone: a nested run of the holder is exempt (token equal AND lock really held — it is, we just
    // failed to take it).
    if let Some(mine) = env_token.and_then(OsStr::to_str).map(str::trim)
        && !mine.is_empty()
        && read_token(&path) == mine
    {
        return Ok(Acquired::Nested(mine.to_owned()));
    }
    let mut on_wait = || printer.diag(messages::LOCK_WAIT);
    match FileLock::acquire(
        &path,
        LockMode::Exclusive,
        &mut on_wait,
        cancel,
        Some(timeout),
    ) {
        Ok(lock) => Ok(Acquired::Held(WorktreeLock::publish(lock, path))),
        Err(e) if e.kind() == io::ErrorKind::TimedOut => Err(Failure::fatal(
            "FatalError",
            messages::lock_timeout(timeout, &path),
        )),
        Err(e) if e.kind() == io::ErrorKind::Interrupted => Err(Failure::Interrupted(
            cancel.reason().unwrap_or(CancelReason::Interrupt),
        )),
        Err(e) => Err(io_err(&e)),
    }
}

/// Non-blocking probe: `Some(lock)` when nobody holds the worktree (the token is not written).
pub(crate) fn try_probe(git_dir: &Path) -> Option<FileLock> {
    FileLock::try_acquire(&lock_path(git_dir), LockMode::Exclusive)
        .ok()
        .flatten()
}

#[cfg(test)]
mod tests {
    use snp_core::output::PrinterOpts;

    use super::*;

    fn printer() -> Printer {
        Printer::new(
            Box::new(io::sink()),
            Box::new(io::sink()),
            PrinterOpts {
                brand: snp_core::brand::Brand::Snp,
                color: false,
                stdout_is_tty: false,
                windows_text_mode: false,
                cancel: CancelToken::new(),
            },
        )
    }

    #[test]
    fn random_hex_is_hex_and_distinct() {
        let a = random_hex(16);
        let b = random_hex(16);
        assert_eq!(a.len(), 32);
        assert!(a.bytes().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
        assert_eq!(random_hex(3).len(), 6);
    }

    #[test]
    fn timeouts() {
        assert_eq!(timeout_from(None), DEFAULT_TIMEOUT);
        assert_eq!(
            timeout_from(Some(OsStr::new("2.5"))),
            Duration::from_millis(2500)
        );
        assert_eq!(timeout_from(Some(OsStr::new("1e9"))), MAX_TIMEOUT);
        for bad in ["-1", "nan", "inf", "x", ""] {
            assert_eq!(
                timeout_from(Some(OsStr::new(bad))),
                DEFAULT_TIMEOUT,
                "{bad}"
            );
        }
    }

    #[test]
    fn token_lifecycle_and_nesting() {
        let d = tempfile::tempdir().unwrap();
        let p = printer();
        let cancel = CancelToken::new();
        let Acquired::Held(held) = acquire(d.path(), None, &p, &cancel, Duration::ZERO).unwrap()
        else {
            panic!("not held")
        };
        let token = held.token().to_owned();
        assert_eq!(read_token(&lock_path(d.path())), token);
        // A nested run with the token is exempt while the lock is held.
        let nested = acquire(
            d.path(),
            Some(OsStr::new(&token)),
            &p,
            &cancel,
            Duration::ZERO,
        )
        .unwrap();
        assert!(matches!(nested, Acquired::Nested(t) if t == token));
        // A wrong token waits and times out.
        let e = acquire(
            d.path(),
            Some(OsStr::new("bogus")),
            &p,
            &cancel,
            Duration::ZERO,
        )
        .unwrap_err();
        assert_eq!(e.exit_code(), 1);
        assert!(try_probe(d.path()).is_none());
        held.release();
        assert_eq!(read_token(&lock_path(d.path())), "");
        // The stale token exempts nobody: the lock is free, so the run takes it with a new token.
        let Acquired::Held(again) = acquire(
            d.path(),
            Some(OsStr::new(&token)),
            &p,
            &cancel,
            Duration::ZERO,
        )
        .unwrap() else {
            panic!("a stale token must not exempt")
        };
        assert_ne!(again.token(), token);
        drop(again);
        assert!(try_probe(d.path()).is_some());
    }

    #[test]
    fn cancelled_wait_is_interrupted() {
        let d = tempfile::tempdir().unwrap();
        let p = printer();
        let cancel = CancelToken::new();
        let _held = acquire(d.path(), None, &p, &cancel, Duration::ZERO).unwrap();
        cancel.cancel(CancelReason::Terminate);
        let e = acquire(d.path(), None, &p, &cancel, Duration::from_secs(5)).unwrap_err();
        assert!(matches!(e, Failure::Interrupted(CancelReason::Terminate)));
    }
}
