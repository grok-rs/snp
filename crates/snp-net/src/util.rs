//! Small shared helpers: random file-name suffixes and file-name validation.
//!
//! Owner: WP-16.

use std::ffi::OsStr;
use std::hash::{BuildHasher, RandomState};
use std::io;
use std::path::{Component, Path};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// 16 hex digits that differ between calls, processes and runs: a `RandomState` (per-process random keys) hashing a
/// counter, the pid and the clock. Collisions are handled by the callers (`create_new`/`create_dir` retry).
pub(crate) fn random_suffix() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let h = RandomState::new().hash_one((std::process::id(), n, nanos));
    format!("{h:016x}")
}

/// `name` is one plain file name: not empty, not `.`/`..`, no separator (`/`, and `\` everywhere, because the same
/// manifest data is used on every OS), no NUL, no drive or root.
pub(crate) fn check_file_name(name: &str) -> io::Result<()> {
    let bad = name.is_empty()
        || name.contains(['/', '\\', '\0'])
        || (cfg!(windows) && name.contains(':'))
        || !matches!(
            Path::new(name).components().collect::<Vec<_>>().as_slice(),
            [Component::Normal(c)] if *c == OsStr::new(name)
        );
    if bad {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("invalid download file name {name:?}"),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suffixes_differ() {
        let a = random_suffix();
        let b = random_suffix();
        assert_eq!(a.len(), 16);
        assert_ne!(a, b);
    }

    #[test]
    fn file_names() {
        assert!(check_file_name("node-v22.11.0-linux-x64.tar.xz").is_ok());
        assert!(check_file_name("rustup-init.exe").is_ok());
        for bad in ["", ".", "..", "a/b", "a\\b", "/abs", "x\0y"] {
            assert!(check_file_name(bad).is_err(), "{bad:?}");
        }
    }
}
