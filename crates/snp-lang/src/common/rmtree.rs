//! `common::rmtree`: readonly-aware, contained, never follows a symlink (`util.rmtree`, `util.py:205-235`).
//!
//! Owner: WP-16.

use std::ffi::OsString;
use std::io;
use std::path::{Component, Path};

/// The components of `path` below `root` (lexical; both made comparable with `normpath`), or `PermissionDenied` when
/// `path` is not strictly inside `root` or uses `..`.
fn relative(path: &Path, root: &Path) -> io::Result<Vec<OsString>> {
    let norm = |p: &Path| std::path::PathBuf::from(snp_pycompat::ospath::normpath(p.as_os_str()));
    let (p, r) = (norm(path), norm(root));
    let refused = || {
        io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "refusing to remove {}: not inside {}",
                path.display(),
                root.display()
            ),
        )
    };
    let rel = p.strip_prefix(&r).map_err(|_| refused())?;
    let comps: Vec<OsString> = rel
        .components()
        .map(|c| match c {
            Component::Normal(n) => Ok(n.to_os_string()),
            _ => Err(refused()),
        })
        .collect::<io::Result<_>>()?;
    if comps.is_empty() {
        return Err(refused());
    }
    Ok(comps)
}

/// Removes `path` (strictly inside `root`).
pub(super) fn remove(path: &Path, root: &Path) -> io::Result<()> {
    let rel = relative(path, root)?;
    imp::remove(root, &rel)
}

#[cfg(unix)]
mod imp {
    use std::ffi::OsString;
    use std::io;
    use std::path::Path;

    use rustix::fd::{AsFd, BorrowedFd, OwnedFd};
    use rustix::fs::{AtFlags, Dir, FileType, Mode, OFlags};
    use rustix::io::Errno;

    /// Directory open flags: never follow the final component.
    fn dir_flags() -> OFlags {
        OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::RDONLY
    }

    /// The file type of `name` in `dir` (not following a symlink); `None` when it does not exist.
    fn kind(dir: BorrowedFd<'_>, name: &OsString) -> io::Result<Option<FileType>> {
        match rustix::fs::statat(dir, name.as_os_str(), AtFlags::SYMLINK_NOFOLLOW) {
            Ok(st) => Ok(Some(FileType::from_raw_mode(st.st_mode))),
            Err(Errno::NOENT) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Owner rwx on the directory `fd` (so its entries can be listed and unlinked).
    fn make_writable(fd: BorrowedFd<'_>) -> io::Result<()> {
        let st = rustix::fs::fstat(fd)?;
        let mode = Mode::from_raw_mode(st.st_mode) & Mode::from_bits_truncate(0o7777);
        rustix::fs::fchmod(fd, mode | Mode::RWXU)?;
        Ok(())
    }

    /// Opens the directory `name` in `parent`; one that refuses (mode 0, `EACCES`) is made owner-accessible first.
    fn open_dir(parent: BorrowedFd<'_>, name: &OsString) -> io::Result<OwnedFd> {
        match rustix::fs::openat(parent, name.as_os_str(), dir_flags(), Mode::empty()) {
            Ok(fd) => Ok(fd),
            Err(Errno::ACCESS | Errno::PERM) => {
                if kind(parent, name)? != Some(FileType::Directory) {
                    return Err(Errno::NOTDIR.into());
                }
                rustix::fs::chmodat(parent, name.as_os_str(), Mode::RWXU, AtFlags::empty())?;
                Ok(rustix::fs::openat(
                    parent,
                    name.as_os_str(),
                    dir_flags(),
                    Mode::empty(),
                )?)
            }
            Err(e) => Err(e.into()),
        }
    }

    /// `unlinkat(dir, name, flags)`, retried once after making `dir` writable (pre-commit's `_handle_readonly`).
    fn unlink(dir: BorrowedFd<'_>, name: &OsString, flags: AtFlags) -> io::Result<()> {
        match rustix::fs::unlinkat(dir, name.as_os_str(), flags) {
            Ok(()) | Err(Errno::NOENT) => Ok(()),
            Err(Errno::ACCESS | Errno::PERM) => {
                make_writable(dir)?;
                match rustix::fs::unlinkat(dir, name.as_os_str(), flags) {
                    Ok(()) | Err(Errno::NOENT) => Ok(()),
                    Err(e) => Err(e.into()),
                }
            }
            Err(e) => Err(e.into()),
        }
    }

    /// A directory being emptied: its handle, its name in the parent, and its subdirectories still to visit.
    struct Frame {
        fd: OwnedFd,
        name: OsString,
        subdirs: Vec<OsString>,
    }

    /// Opens `name` in `parent`, unlinks every non-directory entry and records the subdirectories.
    fn enter(parent: BorrowedFd<'_>, name: OsString) -> io::Result<Frame> {
        let fd = open_dir(parent, &name)?;
        make_writable(fd.as_fd())?;
        let mut subdirs = Vec::new();
        let mut entries = Dir::read_from(&fd)?;
        while let Some(entry) = entries.read() {
            let entry = entry?;
            let n = entry.file_name().to_bytes();
            if n == b"." || n == b".." {
                continue;
            }
            let name: OsString = {
                use std::os::unix::ffi::OsStrExt;
                std::ffi::OsStr::from_bytes(n).to_os_string()
            };
            match kind(fd.as_fd(), &name)? {
                Some(FileType::Directory) => subdirs.push(name),
                Some(_) => unlink(fd.as_fd(), &name, AtFlags::empty())?,
                None => {}
            }
        }
        Ok(Frame { fd, name, subdirs })
    }

    /// Removes `rel` below `root`: the parent is reached component by component without following links.
    pub(super) fn remove(root: &Path, rel: &[OsString]) -> io::Result<()> {
        let Some((target, parents)) = rel.split_last() else {
            return Ok(());
        };
        let root_fd = rustix::fs::open(
            root,
            OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::RDONLY,
            Mode::empty(),
        )?;
        let mut parent = root_fd;
        for c in parents {
            parent = match rustix::fs::openat(&parent, c.as_os_str(), dir_flags(), Mode::empty()) {
                Ok(fd) => fd,
                Err(Errno::NOENT) => return Ok(()),
                Err(e) => return Err(e.into()),
            };
        }
        match kind(parent.as_fd(), target)? {
            None => return Ok(()),
            Some(FileType::Directory) => {}
            Some(_) => return unlink(parent.as_fd(), target, AtFlags::empty()),
        }
        // Iterative depth-first removal (no recursion: a deep tree cannot exhaust the stack).
        let mut stack = vec![enter(parent.as_fd(), target.clone())?];
        while let Some(top) = stack.last_mut() {
            if let Some(sub) = top.subdirs.pop() {
                let frame = enter(top.fd.as_fd(), sub)?;
                stack.push(frame);
                continue;
            }
            let Some(done) = stack.pop() else { break };
            drop(done.fd);
            let p = stack
                .last()
                .map_or_else(|| parent.as_fd(), |f| f.fd.as_fd());
            unlink(p, &done.name, AtFlags::REMOVEDIR)?;
        }
        Ok(())
    }
}

#[cfg(not(unix))]
mod imp {
    use std::ffi::OsString;
    use std::io;
    use std::path::Path;

    /// `FILE_ATTRIBUTE_REPARSE_POINT`.
    const REPARSE_POINT: u32 = 0x400;

    /// Whether `p` is a symlink or another reparse point (never traversed).
    fn is_link(m: &std::fs::Metadata) -> bool {
        use std::os::windows::fs::MetadataExt;
        m.file_type().is_symlink() || m.file_attributes() & REPARSE_POINT != 0
    }

    /// Clears the read-only attribute of every file below `dir` (reparse points are not entered).
    fn clear_readonly(dir: &Path) {
        let mut stack = vec![dir.to_path_buf()];
        while let Some(d) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&d) else {
                continue;
            };
            for e in entries.flatten() {
                let Ok(m) = std::fs::symlink_metadata(e.path()) else {
                    continue;
                };
                if is_link(&m) {
                    continue;
                }
                if m.is_dir() {
                    stack.push(e.path());
                } else if m.permissions().readonly() {
                    let mut p = m.permissions();
                    p.set_readonly(false);
                    let _ignored = std::fs::set_permissions(e.path(), p);
                }
            }
        }
    }

    /// `std::fs::remove_dir_all` (handle-relative, never follows reparse points) with read-only attributes cleared
    /// on `PermissionDenied`.
    pub(super) fn remove(root: &Path, rel: &[OsString]) -> io::Result<()> {
        let mut p = root.to_path_buf();
        for (i, c) in rel.iter().enumerate() {
            p.push(c);
            let m = match std::fs::symlink_metadata(&p) {
                Ok(m) => m,
                Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
                Err(e) => return Err(e),
            };
            let last = i + 1 == rel.len();
            if is_link(&m) {
                if !last {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "path crosses a link",
                    ));
                }
                return if m.is_dir() {
                    std::fs::remove_dir(&p)
                } else {
                    std::fs::remove_file(&p)
                };
            }
            if last && !m.is_dir() {
                if m.permissions().readonly() {
                    let mut perms = m.permissions();
                    perms.set_readonly(false);
                    std::fs::set_permissions(&p, perms)?;
                }
                return std::fs::remove_file(&p);
            }
        }
        match std::fs::remove_dir_all(&p) {
            Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
                clear_readonly(&p);
                std::fs::remove_dir_all(&p)
            }
            other => other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn containment() {
        let r = Path::new("/store/envs");
        assert!(relative(Path::new("/store/envs/py_env-x"), r).is_ok());
        assert!(relative(Path::new("/store/envs/a/b"), r).is_ok());
        for bad in [
            "/store/envs",
            "/store/envs/..",
            "/store",
            "/elsewhere/x",
            "/store/envs/../x",
        ] {
            let e = relative(Path::new(bad), r).unwrap_err();
            assert_eq!(e.kind(), io::ErrorKind::PermissionDenied, "{bad}");
        }
    }
}
