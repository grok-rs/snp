//! Durable filesystem primitives (temp + fsync + rename, contained deletion, unnamed temp files).
//!
//! `write_new_durable` and `atomic_replace` are W0 working baselines (ADR-071, Unix-exact; the Windows rename
//! retry of ADR-068 is added by WP-15 without changing a signature).
//!
//! Owner: WP-15.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-15")]
#![expect(unused_variables, reason = "W0 stub parameters, removed by WP-15")]

use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::hash::{BuildHasher, RandomState};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Windows (ADR-068): every rename-based primitive here (`rename_with_retry`, `atomic_replace`, `retire_dir`, and
/// the store's materialise/marker renames, which call `rename_with_retry`) retries on `ERROR_ACCESS_DENIED`,
/// `ERROR_SHARING_VIOLATION` and `ERROR_LOCK_VIOLATION` (antivirus, indexer, 11 §A.19.2, 06 §5.6) with 50·2^n ms
/// backoff, at most 5 retries (≈ 1.5 s), then returns the last error. Unix: a single attempt.
pub fn rename_with_retry(from: &Path, to: &Path) -> io::Result<()> {
    todo!()
}

/// `O_EXCL` create (0600) + `write_all` + `sync_all` + fsync(parent). Fails if `path` exists. A partially written
/// file is removed again when writing or syncing fails (so a retry is not blocked by `EEXIST`).
pub fn write_new_durable(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut f = create_new_private(path)?;
    let written = f.write_all(bytes).and_then(|()| f.sync_all());
    drop(f);
    if let Err(e) = written {
        let _ignored: io::Result<()> = std::fs::remove_file(path);
        return Err(e);
    }
    sync_parent_dir(path)
}

/// Streams `src` into a new `O_EXCL` durable file; returns bytes written. Used for stash patches (never in memory).
pub fn copy_into_new_durable(path: &Path, src: &mut dyn io::Read) -> io::Result<u64> {
    todo!()
}

/// temp file in the same dir + fsync + rename over `path` (+ fsync dir). Replaces a symlink instead of following it.
///
/// `mode` (Unix permission bits, applied exactly, not through the umask): `None` keeps the mode of the regular file
/// currently at `path` (a symlink or a missing file gets the default `0o666 & !umask`). Windows ignores `mode`.
/// On failure the temporary file is removed and `path` is untouched.
pub fn atomic_replace(path: &Path, bytes: &[u8], mode: Option<u32>) -> io::Result<()> {
    let mode = mode.or_else(|| existing_mode(path));
    let (tmp, mut f) = create_temp_sibling(path)?;
    let result = (|| {
        set_mode(&f, mode)?;
        f.write_all(bytes)?;
        f.sync_all()
    })();
    drop(f);
    let result = result.and_then(|()| std::fs::rename(&tmp, path));
    if let Err(e) = result {
        let _ignored: io::Result<()> = std::fs::remove_file(&tmp);
        return Err(e);
    }
    sync_parent_dir(path)
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

/// `path`'s directory (`.` for a bare file name).
fn parent_dir(path: &Path) -> &Path {
    match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    }
}

/// `O_CREAT|O_EXCL|O_WRONLY`, mode 0600 on Unix.
fn create_new_private(path: &Path) -> io::Result<File> {
    let mut o = OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    o.open(path)
}

/// Makes the directory entry durable: fsync of the parent directory (Unix). Windows has no directory handle
/// flush; NTFS journals the metadata.
#[cfg_attr(
    not(unix),
    expect(clippy::unnecessary_wraps, reason = "fallible on Unix only")
)]
fn sync_parent_dir(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        File::open(parent_dir(path))?.sync_all()
    }
    #[cfg(not(unix))]
    {
        let _: &Path = path;
        Ok(())
    }
}

/// The permission bits of the regular file at `path` (not following a symlink).
fn existing_mode(path: &Path) -> Option<u32> {
    let md = std::fs::symlink_metadata(path).ok()?;
    if !md.is_file() {
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        Some(md.permissions().mode() & 0o7777)
    }
    #[cfg(not(unix))]
    {
        None
    }
}

/// `fchmod(fd, mode)` (Unix); `None` and Windows: no change.
#[cfg_attr(
    not(unix),
    expect(clippy::unnecessary_wraps, reason = "fallible on Unix only")
)]
fn set_mode(f: &File, mode: Option<u32>) -> io::Result<()> {
    #[cfg(unix)]
    if let Some(m) = mode {
        use std::os::unix::fs::PermissionsExt;
        f.set_permissions(std::fs::Permissions::from_mode(m))?;
    }
    #[cfg(not(unix))]
    let _: (&File, Option<u32>) = (f, mode);
    Ok(())
}

/// A fresh `.{name}.{random}.snp-tmp` file next to `path`, created with `O_EXCL` (retried on a name collision).
fn create_temp_sibling(path: &Path) -> io::Result<(PathBuf, File)> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let dir = parent_dir(path);
    let name = path.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "atomic_replace: path has no file name",
        )
    })?;
    let mut last = io::Error::new(io::ErrorKind::AlreadyExists, "no free temporary name");
    for _ in 0..16 {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let salt = RandomState::new().hash_one((std::process::id(), n));
        let mut tmp_name = OsString::from(".");
        tmp_name.push(name);
        tmp_name.push(format!(".{salt:016x}.snp-tmp"));
        let tmp = dir.join(tmp_name);
        let mut o = OpenOptions::new();
        o.write(true).create_new(true);
        match o.open(&tmp) {
            Ok(f) => return Ok((tmp, f)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => last = e,
            Err(e) => return Err(e),
        }
    }
    Err(last)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_new_durable_creates_private_and_refuses_existing() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("journal");
        write_new_durable(&p, b"state").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"state");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let m = std::fs::metadata(&p).unwrap().permissions().mode();
            assert_eq!(m & 0o777, 0o600);
        }
        let e = write_new_durable(&p, b"other").unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(&p).unwrap(), b"state");
    }

    #[test]
    fn atomic_replace_creates_and_replaces() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("cfg.yaml");
        atomic_replace(&p, b"one", None).unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"one");
        atomic_replace(&p, b"two", None).unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"two");
        // No temporary file is left behind.
        let names: Vec<_> = std::fs::read_dir(d.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, [OsString::from("cfg.yaml")]);
    }

    #[cfg(unix)]
    #[test]
    fn atomic_replace_modes() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("hook");
        atomic_replace(&p, b"#!/bin/sh\n", Some(0o755)).unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o7777;
        assert_eq!(mode(&p), 0o755);
        // None keeps the existing mode.
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o640)).unwrap();
        atomic_replace(&p, b"x", None).unwrap();
        assert_eq!(mode(&p), 0o640);
    }

    #[cfg(unix)]
    #[test]
    fn atomic_replace_replaces_a_symlink_instead_of_following_it() {
        let d = tempfile::tempdir().unwrap();
        let target = d.path().join("target");
        std::fs::write(&target, b"keep").unwrap();
        let link = d.path().join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        atomic_replace(&link, b"new", None).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"keep");
        assert!(std::fs::symlink_metadata(&link).unwrap().is_file());
        assert_eq!(std::fs::read(&link).unwrap(), b"new");
    }

    #[test]
    fn atomic_replace_failure_leaves_no_temp() {
        let d = tempfile::tempdir().unwrap();
        // Renaming a file over a non-empty directory fails.
        let p = d.path().join("dir");
        std::fs::create_dir_all(&p).unwrap();
        std::fs::write(p.join("x"), b"").unwrap();
        assert!(atomic_replace(&p, b"data", None).is_err());
        let n = std::fs::read_dir(d.path()).unwrap().count();
        assert_eq!(n, 1);
    }
}
