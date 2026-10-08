//! Durable filesystem primitives (temp + fsync + rename, contained deletion, unnamed temp files).
//!
//! Owner: WP-15.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-15")]
#![expect(unused_variables, reason = "W0 stub parameters, removed by WP-15")]

use std::fs::File;
use std::io;
use std::path::Path;

/// Windows (ADR-068): every rename-based primitive here (`rename_with_retry`, `atomic_replace`, `retire_dir`, and
/// the store's materialise/marker renames, which call `rename_with_retry`) retries on `ERROR_ACCESS_DENIED`,
/// `ERROR_SHARING_VIOLATION` and `ERROR_LOCK_VIOLATION` (antivirus, indexer, 11 §A.19.2, 06 §5.6) with 50·2^n ms
/// backoff, at most 5 retries (≈ 1.5 s), then returns the last error. Unix: a single attempt.
pub fn rename_with_retry(from: &Path, to: &Path) -> io::Result<()> {
    todo!()
}

/// `O_EXCL` create (0600) + `write_all` + `sync_all` + fsync(parent). Fails if `path` exists.
pub fn write_new_durable(path: &Path, bytes: &[u8]) -> io::Result<()> {
    todo!()
}

/// Streams `src` into a new `O_EXCL` durable file; returns bytes written. Used for stash patches (never in memory).
pub fn copy_into_new_durable(path: &Path, src: &mut dyn io::Read) -> io::Result<u64> {
    todo!()
}

/// temp file in the same dir + fsync + rename over `path` (+ fsync dir). Replaces a symlink instead of following it.
pub fn atomic_replace(path: &Path, bytes: &[u8], mode: Option<u32>) -> io::Result<()> {
    todo!()
}

/// In-place rewrite preserving inode/mode/hardlinks (native fixers; 09 §9.8): open without `O_CREAT`, write, `set_len`.
pub fn rewrite_in_place(path: &Path, bytes: &[u8]) -> io::Result<()> {
    todo!()
}

/// rmtree parity (chmod + retry on EACCES/EPERM). Unix: fd-relative walk from an `O_DIRECTORY|O_NOFOLLOW` handle
/// of `root` (rustix `openat`, `unlinkat`, `fchmodat` on the dirfd), so a directory swapped for a symlink
/// mid-deletion is never followed (CVE-2022-21658 class); Windows: `std::fs::remove_dir_all` (handle-relative since
/// 1.58.1) + read-only attribute clearing. Refuses unless `path` is strictly inside `root`. Missing path → Ok.
pub fn remove_tree_within(path: &Path, root: &Path) -> io::Result<()> {
    todo!()
}

/// Unnamed temporary file in `dir`: `O_TMPFILE` (Linux), else `O_EXCL` 0600 create + immediate unlink, Windows
/// `FILE_FLAG_DELETE_ON_CLOSE`. Nothing survives a crash (output spill files, ADR-077).
pub fn unnamed_temp(dir: &Path) -> io::Result<File> {
    todo!()
}

/// rename(path → `trash/<rand>`) then remove; gc/clean never leave half-deleted dirs at live paths.
pub fn retire_dir(path: &Path, trash: &Path) -> io::Result<()> {
    todo!()
}

/// Reads at most `cap` bytes of `path`.
pub fn read_capped(path: &Path, cap: u64) -> io::Result<Vec<u8>> {
    todo!()
}

/// Create/truncate without following a final symlink (crash log).
pub fn create_truncate_nofollow(path: &Path) -> io::Result<File> {
    todo!()
}

/// mkdir -p, created components 0700 (Unix). Refuses (`PermissionDenied`) only a world-writable non-sticky final dir;
/// returns `Ownership::Foreign { root_over_user }` when owned by another uid (ADR-044). The store refuses a Foreign
/// store when `root_over_user` (euid 0, owner non-root) unless `SNP_ALLOW_FOREIGN_STORE=1`, and otherwise warns
/// once on stderr (ADR-077, D-SEC-13, D-DIAG-05).
pub fn ensure_private_dir(path: &Path) -> io::Result<Ownership> {
    todo!()
}

/// Owner of a directory checked by [`ensure_private_dir`].
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Ownership {
    /// Owned by the effective uid.
    Own,
    /// Owned by another uid.
    Foreign {
        /// euid 0 and the owner is not root.
        root_over_user: bool,
    },
}
