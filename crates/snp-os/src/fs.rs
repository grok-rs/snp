//! Durable filesystem primitives (temp + fsync + rename, contained deletion, unnamed temp files).
//!
//! Every write snp makes to a file it owns goes through these functions (ARCHITECTURE §11.5): new files are created
//! `O_EXCL` 0600 and fsynced with their directory, replacements are temp + fsync + rename (replacing a symlink instead
//! of writing through it), renames retry Windows sharing violations (ADR-068), deletion is contained in a root and
//! fd-relative on Unix (ADR-077), and spill files are unnamed (nothing survives a crash).
//!
//! Owner: WP-15.

use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::hash::{BuildHasher, RandomState};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Windows (ADR-068): every rename-based primitive here (`rename_with_retry`, `atomic_replace`, `retire_dir`, and
/// the store's materialise/marker renames, which call `rename_with_retry`) retries on `ERROR_ACCESS_DENIED`,
/// `ERROR_SHARING_VIOLATION` and `ERROR_LOCK_VIOLATION` (antivirus, indexer, 11 §A.19.2, 06 §5.6) with 50·2^n ms
/// backoff, at most 5 retries (≈ 1.5 s), then returns the last error. Unix: a single attempt.
///
/// Reproduces prek's `rename_with_retry` (`fs.rs:54-95`) with the 11 §A.19.2 schedule; pre-commit itself calls
/// `os.replace` once (`store.py`, `install_uninstall.py`).
pub fn rename_with_retry(from: &Path, to: &Path) -> io::Result<()> {
    retry_transient(|| std::fs::rename(from, to))
}

/// Runs `op`, retrying the Windows transient sharing errors of ADR-068 (a single attempt elsewhere).
fn retry_transient(mut op: impl FnMut() -> io::Result<()>) -> io::Result<()> {
    #[cfg(windows)]
    {
        /// `ERROR_ACCESS_DENIED`, `ERROR_SHARING_VIOLATION`, `ERROR_LOCK_VIOLATION`.
        const TRANSIENT: [i32; 3] = [5, 32, 33];
        /// 50·2^n ms for n = 0..5 (≈ 1.55 s in total).
        const BACKOFF_MS: [u64; 5] = [50, 100, 200, 400, 800];
        let mut result = op();
        for delay in BACKOFF_MS {
            match &result {
                Err(e) if e.raw_os_error().is_some_and(|c| TRANSIENT.contains(&c)) => {
                    std::thread::sleep(std::time::Duration::from_millis(delay));
                    result = op();
                }
                _ => break,
            }
        }
        result
    }
    #[cfg(not(windows))]
    {
        op()
    }
}

/// `O_EXCL` create (0600) + `write_all` + `sync_all` + fsync(parent). Fails if `path` exists. A partially written
/// file is removed again when writing or syncing fails (so a retry is not blocked by `EEXIST`).
///
/// snp's journal/pointer writes (ADR-067); pre-commit writes its patch with a plain `open(…, 'wb')`
/// (`staged_files_only.py:68`) and never fsyncs (03 §20.3, 02 §21.10).
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
///
/// The streaming counterpart of [`write_new_durable`] (`staged_files_only.py:66-69` writes the `diff-index` output
/// it holds in memory; snp streams it, ADR-067). A failed read, write or sync removes the partial file.
pub fn copy_into_new_durable(path: &Path, src: &mut dyn Read) -> io::Result<u64> {
    let mut f = create_new_private(path)?;
    let copied = copy_all(src, &mut f).and_then(|n| f.sync_all().map(|()| n));
    drop(f);
    match copied {
        Ok(n) => {
            sync_parent_dir(path)?;
            Ok(n)
        }
        Err(e) => {
            let _ignored: io::Result<()> = std::fs::remove_file(path);
            Err(e)
        }
    }
}

/// `io::copy` with a 64 KiB heap buffer (retrying `Interrupted` reads).
fn copy_all(src: &mut dyn Read, dst: &mut File) -> io::Result<u64> {
    let mut buf = vec![0_u8; 64 * 1024];
    let mut total: u64 = 0;
    loop {
        let n = match src.read(&mut buf) {
            Ok(0) => return Ok(total),
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        dst.write_all(buf.get(..n).unwrap_or_default())?;
        total = total.saturating_add(u64::try_from(n).unwrap_or(u64::MAX));
    }
}

/// temp file in the same dir + fsync + rename over `path` (+ fsync dir). Replaces a symlink instead of following it.
///
/// `mode` (Unix permission bits, applied exactly, not through the umask): `None` keeps the mode of the regular file
/// currently at `path` (a symlink or a missing file gets the default `0o666 & !umask`). Windows ignores `mode`.
/// On failure the temporary file is removed and `path` is untouched. The rename goes through
/// [`rename_with_retry`] (ADR-068). Hook scripts (`install_uninstall.py:92-108`, D-DATA-05) and store metadata.
pub fn atomic_replace(path: &Path, bytes: &[u8], mode: Option<u32>) -> io::Result<()> {
    let mode = mode.or_else(|| existing_mode(path));
    let (tmp, mut f) = create_temp_sibling(path)?;
    let result = (|| {
        set_mode(&f, mode)?;
        f.write_all(bytes)?;
        f.sync_all()
    })();
    drop(f);
    let result = result.and_then(|()| rename_with_retry(&tmp, path));
    if let Err(e) = result {
        let _ignored: io::Result<()> = std::fs::remove_file(&tmp);
        return Err(e);
    }
    sync_parent_dir(path)
}

/// In-place rewrite preserving inode/mode/hardlinks (native fixers; 09 §9.8): open without `O_CREAT`, write, `set_len`.
///
/// pre-commit-hooks' fixers do `open(filename, 'wb').write(…)`: the existing inode (and every hard link to it) keeps
/// its mode and identity, a symlink is written through. A missing file is an error (`NotFound`), never created.
pub fn rewrite_in_place(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut f = OpenOptions::new().write(true).open(path)?;
    f.write_all(bytes)?;
    f.set_len(u64::try_from(bytes.len()).unwrap_or(u64::MAX))?;
    f.flush()
}

/// rmtree parity (chmod + retry on EACCES/EPERM). Unix: fd-relative walk from an `O_DIRECTORY|O_NOFOLLOW` handle
/// of `root` (rustix `openat`, `unlinkat`, `fchmodat` on the dirfd), so a directory swapped for a symlink
/// mid-deletion is never followed (CVE-2022-21658 class); Windows: `std::fs::remove_dir_all` (handle-relative since
/// 1.58.1) + read-only attribute clearing. Refuses unless `path` is strictly inside `root`. Missing path → Ok.
///
/// `util.rmtree` (`util.py:205-235`): `shutil.rmtree` whose `onexc` makes the path and its parent writable and
/// retries `rmdir`/`unlink` on `EACCES`/`EPERM` (read-only Go module caches, T10-UTL-04). Containment is lexical on
/// absolute paths: `..` components are refused, and every directory between `root` and `path` is opened without
/// following symlinks (a symlinked parent fails with `ELOOP`/`ENOTDIR` instead of redirecting the deletion). `root`
/// itself is trusted (it may be a symlink, e.g. macOS `/tmp`).
pub fn remove_tree_within(path: &Path, root: &Path) -> io::Result<()> {
    let rel = contained_relative(path, root)?;
    imp::remove_tree_within(root, &rel)
}

/// The components of `path` below `root` (at least one), or `InvalidInput` when `path` is not strictly inside
/// `root` or contains `..`.
fn contained_relative(path: &Path, root: &Path) -> io::Result<Vec<OsString>> {
    let refuse = |what: &str| {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "refusing to remove {}: {what} {}",
                path.display(),
                root.display()
            ),
        ))
    };
    let abs_path = std::path::absolute(path)?;
    let abs_root = std::path::absolute(root)?;
    let mut root_comps = abs_root
        .components()
        .filter(|c| !matches!(c, Component::CurDir))
        .peekable();
    let mut rel = Vec::new();
    for comp in abs_path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => return refuse("`..` in a path below"),
            other => {
                if let Some(r) = root_comps.peek() {
                    if !component_eq(r, &other) {
                        return refuse("not inside");
                    }
                    root_comps.next();
                } else {
                    match other {
                        Component::Normal(name) => rel.push(name.to_os_string()),
                        _ => return refuse("not inside"),
                    }
                }
            }
        }
    }
    if root_comps.peek().is_some() || rel.is_empty() {
        return refuse("not strictly inside");
    }
    Ok(rel)
}

/// Component equality: exact on Unix, ASCII-case-insensitive on Windows (NTFS names).
fn component_eq(a: &Component<'_>, b: &Component<'_>) -> bool {
    if cfg!(windows) {
        a.as_os_str()
            .as_encoded_bytes()
            .eq_ignore_ascii_case(b.as_os_str().as_encoded_bytes())
    } else {
        a == b
    }
}

/// Unnamed temporary file in `dir`: `O_TMPFILE` (Linux), else `O_EXCL` 0600 create + immediate unlink, Windows
/// `FILE_FLAG_DELETE_ON_CLOSE`. Nothing survives a crash (output spill files, ADR-077).
///
/// The file is open for reading and writing. snp-only (pre-commit keeps every hook output in memory).
pub fn unnamed_temp(dir: &Path) -> io::Result<File> {
    imp::unnamed_temp(dir)
}

/// rename(path → `trash/<rand>`) then remove; gc/clean never leave half-deleted dirs at live paths.
///
/// `trash` is created (0700) when missing and must be on the same filesystem. A missing `path` is `Ok`. The rename
/// goes through [`rename_with_retry`]; the removal through [`remove_tree_within`] (contained in `trash`). Replaces
/// pre-commit's in-place `rmtree` of `gc`/`clean` (`commands/gc.py`, `commands/clean.py`).
pub fn retire_dir(path: &Path, trash: &Path) -> io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
        Ok(_) => {}
    }
    create_private_dirs(trash)?;
    let base = path.file_name().unwrap_or(path.as_os_str());
    let mut last = io::Error::new(io::ErrorKind::AlreadyExists, "no free trash name");
    for _ in 0..16 {
        let mut name = OsString::from(base);
        name.push(format!(".{:016x}", random_u64()));
        let dest = trash.join(name);
        if std::fs::symlink_metadata(&dest).is_ok() {
            continue;
        }
        match rename_with_retry(path, &dest) {
            Ok(()) => return remove_tree_within(&dest, trash),
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(e) => last = e,
        }
    }
    Err(last)
}

/// Reads at most `cap` bytes of `path`.
///
/// Bounded `open(path, 'rb').read(cap)` for files that may be hostile or huge (markers, hook scripts, `README`
/// signatures).
pub fn read_capped(path: &Path, cap: u64) -> io::Result<Vec<u8>> {
    let f = File::open(path)?;
    let mut out = Vec::new();
    // Pre-size from the file length when it is known and bounded (never more than `cap`).
    if let Ok(md) = f.metadata() {
        let hint = md.len().min(cap).min(64 * 1024 * 1024);
        out.try_reserve(usize::try_from(hint).unwrap_or(0))
            .map_err(|_| io::Error::from(io::ErrorKind::OutOfMemory))?;
    }
    f.take(cap).read_to_end(&mut out)?;
    Ok(out)
}

/// Create/truncate without following a final symlink (crash log).
///
/// `open(log_path, 'wb')` of `error_handler.py:34-40`, hardened (ARCHITECTURE §11.5): a symlink planted at `path`
/// fails with `ELOOP` (Unix `O_NOFOLLOW`) instead of truncating its target; Windows removes the link first.
pub fn create_truncate_nofollow(path: &Path) -> io::Result<File> {
    imp::create_truncate_nofollow(path)
}

/// mkdir -p, created components 0700 (Unix). Refuses (`PermissionDenied`) only a world-writable non-sticky final dir;
/// returns `Ownership::Foreign { root_over_user }` when owned by another uid (ADR-044). The store refuses a Foreign
/// store when `root_over_user` (euid 0, owner non-root) unless `SNP_ALLOW_FOREIGN_STORE=1`, and otherwise warns
/// once on stderr (ADR-077, D-SEC-13, D-DIAG-05).
///
/// pre-commit's `Store.__init__` (`store.py:60-81`) only `makedirs`; the ownership policy is snp's (ARCHITECTURE
/// §5.1). Windows: `create_dir_all`, always `Own`.
pub fn ensure_private_dir(path: &Path) -> io::Result<Ownership> {
    create_private_dirs(path)?;
    imp::check_private_dir(path)
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

/// `mkdir -p` with every created component 0700 on Unix.
fn create_private_dirs(path: &Path) -> io::Result<()> {
    let mut b = std::fs::DirBuilder::new();
    b.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        b.mode(0o700);
    }
    b.create(path)
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

/// A process-unique pseudo-random 64-bit value (std's per-process random hasher keys + a counter).
fn random_u64() -> u64 {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    RandomState::new().hash_one((std::process::id(), n))
}

/// A fresh `.{name}.{random}.snp-tmp` file next to `path`, created with `O_EXCL` (retried on a name collision).
fn create_temp_sibling(path: &Path) -> io::Result<(PathBuf, File)> {
    let dir = parent_dir(path);
    let name = path.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "atomic_replace: path has no file name",
        )
    })?;
    let mut last = io::Error::new(io::ErrorKind::AlreadyExists, "no free temporary name");
    for _ in 0..16 {
        let mut tmp_name = OsString::from(".");
        tmp_name.push(name);
        tmp_name.push(format!(".{:016x}.snp-tmp", random_u64()));
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

/// A fresh `.snp-spill.{random}` name in `dir` (create + unlink fallback of [`unnamed_temp`]).
#[cfg(unix)]
fn create_unlinked_temp(dir: &Path) -> io::Result<File> {
    let mut last = io::Error::new(io::ErrorKind::AlreadyExists, "no free temporary name");
    for _ in 0..16 {
        let p = dir.join(format!(".snp-spill.{:016x}", random_u64()));
        let mut o = OpenOptions::new();
        o.read(true).write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            o.mode(0o600);
        }
        match o.open(&p) {
            Ok(f) => {
                if let Err(e) = std::fs::remove_file(&p) {
                    drop(f);
                    let _ignored: io::Result<()> = std::fs::remove_file(&p);
                    return Err(e);
                }
                return Ok(f);
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => last = e,
            Err(e) => return Err(e),
        }
    }
    Err(last)
}

#[cfg(unix)]
mod imp {
    use std::ffi::{CStr, CString, OsStr, OsString};
    use std::fs::File;
    use std::io;
    use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::MetadataExt;
    use std::path::Path;

    use rustix::fs::{AtFlags, Dir, FileType, Mode, OFlags};

    use super::Ownership;

    /// `O_RDONLY|O_DIRECTORY|O_NOFOLLOW|O_CLOEXEC` relative to `dirfd`.
    fn open_subdir(dirfd: BorrowedFd<'_>, name: &CStr) -> io::Result<OwnedFd> {
        rustix::fs::openat(
            dirfd,
            name,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(io::Error::from)
    }

    fn cstring(name: &OsStr) -> io::Result<CString> {
        CString::new(name.as_bytes())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains a NUL byte"))
    }

    fn is_missing(e: &io::Error) -> bool {
        e.kind() == io::ErrorKind::NotFound
    }

    /// `unlinkat(dirfd, name, flags)`; on `EACCES`/`EPERM` makes the directory writable for its owner (`fchmod` on
    /// the held dirfd: `_handle_readonly`'s chmod of the parent) and retries once. A vanished entry is `Ok`.
    fn unlink_retry(dirfd: BorrowedFd<'_>, name: &CStr, flags: AtFlags) -> io::Result<()> {
        match rustix::fs::unlinkat(dirfd, name, flags) {
            Ok(()) | Err(rustix::io::Errno::NOENT) => return Ok(()),
            Err(e) if e == rustix::io::Errno::ACCESS || e == rustix::io::Errno::PERM => {}
            Err(e) => return Err(e.into()),
        }
        let st = rustix::fs::fstat(dirfd)?;
        let mode = Mode::from_raw_mode(st.st_mode) | Mode::WUSR | Mode::XUSR | Mode::RUSR;
        rustix::fs::fchmod(dirfd, mode)?;
        match rustix::fs::unlinkat(dirfd, name, flags) {
            Ok(()) | Err(rustix::io::Errno::NOENT) => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    /// One directory being emptied: its handle and its name in the parent frame.
    struct Frame {
        dir: Dir,
        fd: OwnedFd,
        name: CString,
    }

    /// Opens `name` (a directory) below `parent` for walking.
    fn frame(parent: BorrowedFd<'_>, name: CString) -> io::Result<Frame> {
        let fd = open_subdir(parent, &name)?;
        let dir = Dir::read_from(&fd)?;
        Ok(Frame { dir, fd, name })
    }

    /// Whether `name` in `dirfd` is a directory (not following a final symlink).
    fn is_dir_at(dirfd: BorrowedFd<'_>, name: &CStr) -> io::Result<Option<bool>> {
        match rustix::fs::statat(dirfd, name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(st) => Ok(Some(
                FileType::from_raw_mode(st.st_mode) == FileType::Directory,
            )),
            Err(rustix::io::Errno::NOENT) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Empties and removes the directory `name` of `parent` without following any symlink (iterative: the depth of
    /// the tree is bounded by the fd limit, never by the stack).
    fn remove_dir_tree(parent: BorrowedFd<'_>, name: &CStr) -> io::Result<()> {
        let mut stack: Vec<Frame> = Vec::new();
        match frame(parent, name.to_owned()) {
            Ok(f) => stack.push(f),
            Err(e) if is_missing(&e) => return Ok(()),
            // Swapped for a non-directory between the check and the open: remove the entry itself.
            Err(e)
                if e.raw_os_error() == Some(rustix::io::Errno::NOTDIR.raw_os_error())
                    || e.raw_os_error() == Some(rustix::io::Errno::LOOP.raw_os_error()) =>
            {
                return unlink_retry(parent, name, AtFlags::empty());
            }
            Err(e) => return Err(e),
        }
        let mut rescans: u32 = 0;
        while let Some(top) = stack.last_mut() {
            match top.dir.read() {
                Some(Ok(entry)) => {
                    let child = entry.file_name();
                    if child == c"." || child == c".." {
                        continue;
                    }
                    let child = child.to_owned();
                    let kind = entry.file_type();
                    let is_dir = match kind {
                        FileType::Directory => true,
                        FileType::Unknown => is_dir_at(top.fd.as_fd(), &child)?.unwrap_or(false),
                        _ => false,
                    };
                    if !is_dir {
                        unlink_retry(top.fd.as_fd(), &child, AtFlags::empty())?;
                        continue;
                    }
                    match frame(top.fd.as_fd(), child.clone()) {
                        Ok(f) => stack.push(f),
                        Err(e) if is_missing(&e) => {}
                        Err(e)
                            if e.raw_os_error()
                                == Some(rustix::io::Errno::NOTDIR.raw_os_error())
                                || e.raw_os_error()
                                    == Some(rustix::io::Errno::LOOP.raw_os_error()) =>
                        {
                            // A directory swapped for a symlink mid-deletion: unlink the link, never follow it.
                            unlink_retry(top.fd.as_fd(), &child, AtFlags::empty())?;
                        }
                        Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
                            // An unreadable directory: make it accessible to its owner through the parent handle
                            // (rmtree's chmod), then retry once.
                            let st = rustix::fs::statat(
                                top.fd.as_fd(),
                                &child,
                                AtFlags::SYMLINK_NOFOLLOW,
                            )?;
                            if FileType::from_raw_mode(st.st_mode) != FileType::Directory {
                                unlink_retry(top.fd.as_fd(), &child, AtFlags::empty())?;
                                continue;
                            }
                            rustix::fs::chmodat(
                                top.fd.as_fd(),
                                &child,
                                Mode::from_raw_mode(st.st_mode) | Mode::RWXU,
                                AtFlags::empty(),
                            )?;
                            let f = frame(top.fd.as_fd(), child)?;
                            stack.push(f);
                        }
                        Err(e) => return Err(e),
                    }
                }
                Some(Err(e)) => return Err(e.into()),
                None => {
                    let Some(done) = stack.pop() else { break };
                    let Frame { dir, fd, name } = done;
                    drop(dir);
                    drop(fd);
                    let parent_fd = match stack.last() {
                        Some(f) => f.fd.as_fd(),
                        None => parent,
                    };
                    match unlink_retry(parent_fd, &name, AtFlags::REMOVEDIR) {
                        Ok(()) => {}
                        // Something was created concurrently: rescan the directory (bounded).
                        Err(e)
                            if e.raw_os_error()
                                == Some(rustix::io::Errno::NOTEMPTY.raw_os_error())
                                && rescans < 8 =>
                        {
                            rescans = rescans.saturating_add(1);
                            let f = frame(parent_fd, name)?;
                            stack.push(f);
                        }
                        Err(e) => return Err(e),
                    }
                }
            }
        }
        Ok(())
    }

    pub(super) fn remove_tree_within(root: &Path, rel: &[OsString]) -> io::Result<()> {
        let root_fd = match rustix::fs::open(
            root,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        ) {
            Ok(fd) => fd,
            Err(rustix::io::Errno::NOENT) => return Ok(()),
            Err(e) => return Err(e.into()),
        };
        let Some((last, parents)) = rel.split_last() else {
            return Ok(());
        };
        let mut dir = root_fd;
        for comp in parents {
            let name = cstring(comp)?;
            dir = match open_subdir(dir.as_fd(), &name) {
                Ok(fd) => fd,
                Err(e) if is_missing(&e) => return Ok(()),
                Err(e) => return Err(e),
            };
        }
        let name = cstring(last)?;
        match is_dir_at(dir.as_fd(), &name)? {
            None => Ok(()),
            Some(true) => remove_dir_tree(dir.as_fd(), &name),
            Some(false) => unlink_retry(dir.as_fd(), &name, AtFlags::empty()),
        }
    }

    pub(super) fn unnamed_temp(dir: &Path) -> io::Result<File> {
        #[cfg(any(target_os = "linux", target_os = "android"))]
        {
            match rustix::fs::open(
                dir,
                OFlags::TMPFILE | OFlags::RDWR | OFlags::CLOEXEC,
                Mode::RUSR | Mode::WUSR,
            ) {
                Ok(fd) => return Ok(File::from(fd)),
                // The filesystem (or kernel) has no O_TMPFILE: create + unlink instead.
                Err(
                    rustix::io::Errno::OPNOTSUPP
                    | rustix::io::Errno::ISDIR
                    | rustix::io::Errno::INVAL
                    | rustix::io::Errno::NOSYS,
                ) => {}
                Err(e) => return Err(e.into()),
            }
        }
        super::create_unlinked_temp(dir)
    }

    pub(super) fn create_truncate_nofollow(path: &Path) -> io::Result<File> {
        let fd = rustix::fs::open(
            path,
            OFlags::WRONLY | OFlags::CREATE | OFlags::TRUNC | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_raw_mode(0o666),
        )?;
        Ok(File::from(fd))
    }

    pub(super) fn check_private_dir(path: &Path) -> io::Result<Ownership> {
        let md = std::fs::metadata(path)?;
        if !md.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::NotADirectory,
                format!("not a directory: {}", path.display()),
            ));
        }
        let mode = md.mode();
        if mode & 0o002 != 0 && mode & 0o1000 == 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "refusing a world-writable directory without the sticky bit: {}",
                    path.display()
                ),
            ));
        }
        let euid = rustix::process::geteuid();
        let owner = md.uid();
        if owner == euid.as_raw() {
            Ok(Ownership::Own)
        } else {
            Ok(Ownership::Foreign {
                root_over_user: euid.is_root() && owner != 0,
            })
        }
    }
}

#[cfg(windows)]
mod imp {
    use std::ffi::OsString;
    use std::fs::{File, OpenOptions};
    use std::io;
    use std::os::windows::fs::{FileTypeExt, OpenOptionsExt};
    use std::path::Path;

    use super::Ownership;

    const GENERIC_READ: u32 = 0x8000_0000;
    const GENERIC_WRITE: u32 = 0x4000_0000;
    const DELETE: u32 = 0x0001_0000;
    const FILE_SHARE_READ: u32 = 0x1;
    const FILE_SHARE_WRITE: u32 = 0x2;
    const FILE_SHARE_DELETE: u32 = 0x4;
    const FILE_ATTRIBUTE_TEMPORARY: u32 = 0x100;
    const FILE_FLAG_DELETE_ON_CLOSE: u32 = 0x0400_0000;
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;

    /// Clears the read-only attribute of every entry below `path` (not following links), as rmtree's `onexc`
    /// chmod does on Windows.
    fn clear_readonly(path: &Path) {
        let Ok(md) = std::fs::symlink_metadata(path) else {
            return;
        };
        if md.file_type().is_symlink() {
            return;
        }
        let mut perms = md.permissions();
        if perms.readonly() {
            #[expect(
                clippy::permissions_set_readonly_false,
                reason = "rmtree parity: make read-only entries deletable (Windows attribute, not a Unix mode)"
            )]
            perms.set_readonly(false);
            let _ignored: io::Result<()> = std::fs::set_permissions(path, perms);
        }
        if md.is_dir()
            && let Ok(rd) = std::fs::read_dir(path)
        {
            for entry in rd.flatten() {
                clear_readonly(&entry.path());
            }
        }
    }

    pub(super) fn remove_tree_within(root: &Path, rel: &[OsString]) -> io::Result<()> {
        // Refuse a reparse point (symlink or junction) between root and the target.
        let mut cur = root.to_path_buf();
        let Some((last, parents)) = rel.split_last() else {
            return Ok(());
        };
        for comp in parents {
            cur.push(comp);
            match std::fs::symlink_metadata(&cur) {
                Ok(md) if md.file_type().is_symlink() => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!("refusing to remove below a link: {}", cur.display()),
                    ));
                }
                Ok(_) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
                Err(e) => return Err(e),
            }
        }
        cur.push(last);
        let md = match std::fs::symlink_metadata(&cur) {
            Ok(md) => md,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e),
        };
        let ft = md.file_type();
        let remove = |p: &Path| -> io::Result<()> {
            let r = if ft.is_symlink_dir() {
                // A directory symlink or junction: remove the link itself.
                std::fs::remove_dir(p)
            } else if ft.is_dir() {
                std::fs::remove_dir_all(p)
            } else {
                std::fs::remove_file(p)
            };
            match r {
                Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
                other => other,
            }
        };
        match super::retry_transient(|| remove(&cur)) {
            Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
                clear_readonly(&cur);
                super::retry_transient(|| remove(&cur))
            }
            other => other,
        }
    }

    pub(super) fn unnamed_temp(dir: &Path) -> io::Result<File> {
        let mut last = io::Error::new(io::ErrorKind::AlreadyExists, "no free temporary name");
        for _ in 0..16 {
            let p = dir.join(format!(".snp-spill.{:016x}", super::random_u64()));
            let mut o = OpenOptions::new();
            o.read(true)
                .write(true)
                .create_new(true)
                .access_mode(GENERIC_READ | GENERIC_WRITE | DELETE)
                .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
                .attributes(FILE_ATTRIBUTE_TEMPORARY)
                .custom_flags(FILE_FLAG_DELETE_ON_CLOSE);
            match o.open(&p) {
                Ok(f) => return Ok(f),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => last = e,
                Err(e) => return Err(e),
            }
        }
        Err(last)
    }

    pub(super) fn create_truncate_nofollow(path: &Path) -> io::Result<File> {
        if std::fs::symlink_metadata(path).is_ok_and(|md| md.file_type().is_symlink()) {
            std::fs::remove_file(path)?;
        }
        OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)
    }

    pub(super) fn check_private_dir(path: &Path) -> io::Result<Ownership> {
        let md = std::fs::metadata(path)?;
        if md.is_dir() {
            Ok(Ownership::Own)
        } else {
            Err(io::Error::new(
                io::ErrorKind::NotADirectory,
                format!("not a directory: {}", path.display()),
            ))
        }
    }
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
    fn copy_into_new_durable_streams_and_counts() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("patch");
        let data: Vec<u8> = (0..200_000_u32)
            .map(|i| u8::try_from(i % 251).unwrap())
            .collect();
        let n = copy_into_new_durable(&p, &mut &data[..]).unwrap();
        assert_eq!(n, 200_000);
        assert_eq!(std::fs::read(&p).unwrap(), data);
        let e = copy_into_new_durable(&p, &mut &b"x"[..]).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::AlreadyExists);
    }

    #[test]
    fn copy_into_new_durable_removes_partial_file_on_read_error() {
        struct Failing(u8);
        impl Read for Failing {
            fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
                if self.0 == 0 {
                    return Err(io::Error::other("boom"));
                }
                self.0 -= 1;
                buf[0] = b'x';
                Ok(1)
            }
        }
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("patch");
        assert!(copy_into_new_durable(&p, &mut Failing(3)).is_err());
        assert!(!p.exists());
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

    #[cfg(unix)]
    #[test]
    fn rewrite_in_place_keeps_inode_mode_and_hardlinks() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("f.txt");
        std::fs::write(&p, b"a long original content\n").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o751)).unwrap();
        let link = d.path().join("hard");
        std::fs::hard_link(&p, &link).unwrap();
        let ino = std::fs::metadata(&p).unwrap().ino();
        rewrite_in_place(&p, b"short\n").unwrap();
        let md = std::fs::metadata(&p).unwrap();
        assert_eq!(md.ino(), ino);
        assert_eq!(md.permissions().mode() & 0o7777, 0o751);
        assert_eq!(std::fs::read(&link).unwrap(), b"short\n");
        rewrite_in_place(&p, b"a much longer replacement content\n").unwrap();
        assert_eq!(
            std::fs::read(&p).unwrap(),
            b"a much longer replacement content\n"
        );
    }

    #[test]
    fn rewrite_in_place_never_creates() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("missing");
        assert_eq!(
            rewrite_in_place(&p, b"x").unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        assert!(!p.exists());
    }

    #[test]
    fn contained_relative_rules() {
        let root = Path::new("/store/root");
        let rel = |p: &str| contained_relative(Path::new(p), root);
        assert_eq!(
            rel("/store/root/a/b").unwrap(),
            [OsString::from("a"), OsString::from("b")]
        );
        assert_eq!(rel("/store/root/./a").unwrap(), [OsString::from("a")]);
        assert!(rel("/store/root").is_err());
        assert!(rel("/store/rootx/a").is_err());
        assert!(rel("/store/root/a/../../x").is_err());
        assert!(rel("/store/root/..").is_err());
        assert!(rel("/elsewhere").is_err());
        assert!(rel("/store").is_err());
    }

    #[test]
    fn remove_tree_within_removes_and_refuses() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("root");
        let victim = root.join("a");
        std::fs::create_dir_all(victim.join("b/c")).unwrap();
        std::fs::write(victim.join("b/c/f"), b"x").unwrap();
        std::fs::write(victim.join("g"), b"y").unwrap();
        let outside = d.path().join("outside");
        std::fs::write(&outside, b"keep").unwrap();
        assert_eq!(
            remove_tree_within(&outside, &root).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(
            remove_tree_within(&root, &root).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        assert!(remove_tree_within(&root.join("a/../../outside"), &root).is_err());
        assert!(outside.exists());
        remove_tree_within(&victim, &root).unwrap();
        assert!(!victim.exists());
        assert!(root.exists());
        // Missing → Ok.
        remove_tree_within(&victim, &root).unwrap();
        remove_tree_within(&root.join("x/y/z"), &root).unwrap();
        // A plain file inside root.
        std::fs::write(root.join("file"), b"").unwrap();
        remove_tree_within(&root.join("file"), &root).unwrap();
        assert!(!root.join("file").exists());
    }

    #[cfg(unix)]
    #[test]
    fn remove_tree_within_never_follows_symlinks() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("root");
        let outside = d.path().join("outside");
        std::fs::create_dir_all(outside.join("deep")).unwrap();
        std::fs::write(outside.join("deep/precious"), b"keep").unwrap();
        std::fs::create_dir_all(root.join("victim")).unwrap();
        // A symlink inside the tree is unlinked, its target untouched.
        std::os::unix::fs::symlink(&outside, root.join("victim/link")).unwrap();
        remove_tree_within(&root.join("victim"), &root).unwrap();
        assert!(outside.join("deep/precious").exists());
        // A symlinked parent component is refused.
        std::os::unix::fs::symlink(&outside, root.join("via")).unwrap();
        assert!(remove_tree_within(&root.join("via/deep"), &root).is_err());
        assert!(outside.join("deep/precious").exists());
        // The final component being a symlink to a directory: the link is removed, not the target.
        remove_tree_within(&root.join("via"), &root).unwrap();
        assert!(!root.join("via").exists());
        assert!(outside.join("deep/precious").exists());
    }

    #[cfg(unix)]
    #[test]
    fn remove_tree_within_handles_read_only_directories() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("root");
        let modcache = root.join("pkg/mod/example.com/x@v1.0.0");
        std::fs::create_dir_all(modcache.join("sub")).unwrap();
        std::fs::write(modcache.join("sub/go.mod"), b"module x\n").unwrap();
        for p in [modcache.join("sub"), modcache.clone()] {
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o555)).unwrap();
        }
        std::fs::set_permissions(
            modcache.join("sub/go.mod"),
            std::fs::Permissions::from_mode(0o444),
        )
        .unwrap();
        remove_tree_within(&root.join("pkg"), &root).unwrap();
        assert!(!root.join("pkg").exists());
    }

    #[test]
    fn unnamed_temp_is_unnamed_and_usable() {
        use std::io::{Seek, SeekFrom};
        let d = tempfile::tempdir().unwrap();
        let mut f = unnamed_temp(d.path()).unwrap();
        f.write_all(b"secret output").unwrap();
        f.seek(SeekFrom::Start(0)).unwrap();
        let mut back = [0_u8; 13];
        f.read_exact(&mut back).unwrap();
        assert_eq!(&back, b"secret output");
        #[cfg(unix)]
        assert_eq!(std::fs::read_dir(d.path()).unwrap().count(), 0);
        drop(f);
        assert_eq!(std::fs::read_dir(d.path()).unwrap().count(), 0);
    }

    #[test]
    fn retire_dir_moves_then_removes() {
        let d = tempfile::tempdir().unwrap();
        let live = d.path().join("repos/abc");
        std::fs::create_dir_all(live.join("x")).unwrap();
        std::fs::write(live.join("x/f"), b"1").unwrap();
        let trash = d.path().join("trash");
        retire_dir(&live, &trash).unwrap();
        assert!(!live.exists());
        assert_eq!(std::fs::read_dir(&trash).unwrap().count(), 0);
        // Missing → Ok.
        retire_dir(&live, &trash).unwrap();
    }

    #[test]
    fn read_capped_bounds_the_read() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("f");
        std::fs::write(&p, b"0123456789").unwrap();
        assert_eq!(read_capped(&p, 4).unwrap(), b"0123");
        assert_eq!(read_capped(&p, 100).unwrap(), b"0123456789");
        assert_eq!(read_capped(&p, 0).unwrap(), b"");
        assert!(read_capped(&d.path().join("missing"), 1).is_err());
    }

    #[test]
    fn create_truncate_nofollow_truncates() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("log");
        std::fs::write(&p, b"old old old").unwrap();
        let mut f = create_truncate_nofollow(&p).unwrap();
        f.write_all(b"new").unwrap();
        drop(f);
        assert_eq!(std::fs::read(&p).unwrap(), b"new");
    }

    #[cfg(unix)]
    #[test]
    fn create_truncate_nofollow_refuses_a_symlink() {
        let d = tempfile::tempdir().unwrap();
        let target = d.path().join("target");
        std::fs::write(&target, b"precious").unwrap();
        let link = d.path().join("log");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(create_truncate_nofollow(&link).is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"precious");
    }

    #[cfg(unix)]
    #[test]
    fn ensure_private_dir_policy() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let store = d.path().join("a/b/store");
        assert_eq!(ensure_private_dir(&store).unwrap(), Ownership::Own);
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o7777;
        assert_eq!(mode(&store), 0o700);
        assert_eq!(mode(&d.path().join("a")), 0o700);
        // Idempotent; group-writable is accepted.
        std::fs::set_permissions(&store, std::fs::Permissions::from_mode(0o770)).unwrap();
        assert_eq!(ensure_private_dir(&store).unwrap(), Ownership::Own);
        // World-writable without sticky: refused; with sticky: accepted.
        std::fs::set_permissions(&store, std::fs::Permissions::from_mode(0o777)).unwrap();
        assert_eq!(
            ensure_private_dir(&store).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
        std::fs::set_permissions(&store, std::fs::Permissions::from_mode(0o1777)).unwrap();
        assert_eq!(ensure_private_dir(&store).unwrap(), Ownership::Own);
        // A directory owned by another uid (`/` is root's) is Foreign.
        if !rustix::process::geteuid().is_root() {
            assert_eq!(
                imp::check_private_dir(Path::new("/")).unwrap(),
                Ownership::Foreign {
                    root_over_user: false
                }
            );
        }
        // A file is not a directory.
        let f = d.path().join("file");
        std::fs::write(&f, b"").unwrap();
        assert!(ensure_private_dir(&f).is_err());
    }

    #[test]
    fn rename_with_retry_renames() {
        let d = tempfile::tempdir().unwrap();
        let a = d.path().join("a");
        let b = d.path().join("b");
        std::fs::write(&a, b"x").unwrap();
        rename_with_retry(&a, &b).unwrap();
        assert!(!a.exists());
        assert_eq!(std::fs::read(&b).unwrap(), b"x");
        assert_eq!(
            rename_with_retry(&a, &b).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
    }

    #[cfg(windows)]
    #[test]
    fn rename_with_retry_survives_an_open_handle() {
        use std::os::windows::fs::OpenOptionsExt;
        let d = tempfile::tempdir().unwrap();
        let a = d.path().join("a");
        let b = d.path().join("b");
        std::fs::write(&a, b"x").unwrap();
        // No FILE_SHARE_DELETE: the rename fails with a sharing violation until the handle closes.
        let held = OpenOptions::new()
            .read(true)
            .share_mode(0x1 | 0x2)
            .open(&a)
            .unwrap();
        let t = std::thread::Builder::new()
            .spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(300));
                drop(held);
            })
            .unwrap();
        rename_with_retry(&a, &b).unwrap();
        t.join().unwrap();
        assert_eq!(std::fs::read(&b).unwrap(), b"x");
    }
}
