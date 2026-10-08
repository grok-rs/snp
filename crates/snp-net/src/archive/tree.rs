//! The directory an archive is extracted into, manipulated without ever following a symlink (ADR-077).
//!
//! Unix: every operation is relative to a directory handle opened `O_DIRECTORY|O_NOFOLLOW` and walks one component at
//! a time with `O_NOFOLLOW` (Linux additionally opens existing directories with `openat2(RESOLVE_BENEATH |
//! RESOLVE_NO_SYMLINKS)` where the kernel supports it); files are created `O_CREAT|O_EXCL|O_NOFOLLOW`. Windows: every
//! path below the root is checked for symlinks and reparse points before it is used. The tree is fresh and private
//! (`<dest>.part-<rand>`), and the archive's own symlinks are only created in the second pass, so a first-pass
//! operation can never meet a symlink unless a concurrent process planted one — which these checks refuse.
//!
//! Owner: WP-16.

use std::ffi::OsString;
use std::io;
use std::path::Path;
#[cfg(not(unix))]
use std::path::PathBuf;

use super::path::RelPath;

/// Mode of directories created implicitly (a file's parent without its own entry), as `tar` gives them.
pub(crate) const IMPLICIT_DIR_MODE: u32 = 0o755;

/// The tree being populated.
#[derive(Debug)]
pub(crate) struct Tree {
    /// The tree root (`<dest>.part-<rand>`).
    #[cfg(not(unix))]
    root: PathBuf,
    /// The root's directory handle.
    #[cfg(unix)]
    fd: rustix::fd::OwnedFd,
    /// Directories created implicitly (they get [`IMPLICIT_DIR_MODE`] at the end).
    implicit: Vec<RelPath>,
}

impl Tree {
    /// Creates the (new, empty) root directory and opens it.
    #[expect(
        clippy::create_dir,
        reason = "the .part directory must be new (EEXIST = name collision)"
    )]
    pub(crate) fn create(root: &Path) -> io::Result<Self> {
        std::fs::create_dir(root)?;
        #[cfg(unix)]
        {
            use rustix::fs::{Mode, OFlags};
            let fd = rustix::fs::open(
                root,
                OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::RDONLY,
                Mode::empty(),
            )?;
            Ok(Self {
                fd,
                implicit: Vec::new(),
            })
        }
        #[cfg(not(unix))]
        {
            Ok(Self {
                root: root.to_path_buf(),
                implicit: Vec::new(),
            })
        }
    }

    /// The root path.
    #[cfg(not(unix))]
    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    /// Directories created implicitly so far.
    pub(crate) fn implicit_dirs(&self) -> &[RelPath] {
        &self.implicit
    }

    /// `rel` joined to the root (Windows path operations).
    #[cfg(not(unix))]
    fn full(&self, rel: &[OsString]) -> PathBuf {
        let mut p = self.root.clone();
        p.extend(rel);
        p
    }
}

/// Splits `rel` into its parent components and its last component.
fn split_last(rel: &[OsString]) -> io::Result<(&[OsString], &OsString)> {
    rel.split_last()
        .map(|(last, parent)| (parent, last))
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "empty archive path"))
}

#[cfg(unix)]
mod imp {
    use std::ffi::OsString;
    use std::fs::File;
    use std::io;

    use rustix::fd::{AsFd, BorrowedFd, OwnedFd};
    use rustix::fs::{AtFlags, FileType, Mode, OFlags, RawMode};
    use rustix::io::Errno;

    use super::{Tree, split_last};

    /// Flags of every directory open.
    fn dir_flags() -> OFlags {
        OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::RDONLY
    }

    /// `m & 0o7777` as a rustix mode.
    pub(super) fn mode(m: u32) -> Mode {
        Mode::from_raw_mode(RawMode::try_from(m & 0o7777).unwrap_or(0o700))
    }

    #[expect(clippy::multiple_inherent_impl, reason = "the Unix half of Tree")]
    impl Tree {
        /// The handle of the directory `rel` (the root for an empty `rel`), creating missing components (mode 0700
        /// until the final modes are applied) when `create`.
        fn dir(&mut self, rel: &[OsString], create: bool) -> io::Result<Option<OwnedFd>> {
            if rel.is_empty() {
                return Ok(None);
            }
            if !create && let Some(fd) = self.open_beneath(rel)? {
                return Ok(Some(fd));
            }
            let mut cur: Option<OwnedFd> = None;
            for (i, comp) in rel.iter().enumerate() {
                let parent: BorrowedFd<'_> =
                    cur.as_ref().map_or_else(|| self.fd.as_fd(), AsFd::as_fd);
                if create {
                    match rustix::fs::mkdirat(parent, comp.as_os_str(), Mode::RWXU) {
                        Ok(()) => self
                            .implicit
                            .push(rel.get(..=i).map(<[OsString]>::to_vec).unwrap_or_default()),
                        Err(Errno::EXIST) => {}
                        Err(e) => return Err(e.into()),
                    }
                }
                let next =
                    rustix::fs::openat(parent, comp.as_os_str(), dir_flags(), Mode::empty())?;
                cur = Some(next);
            }
            Ok(cur)
        }

        /// Linux: `openat2(root, rel, RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS | RESOLVE_NO_MAGICLINKS)`. `Ok(None)`
        /// when the kernel (or a seccomp filter) does not offer `openat2`; the caller walks component by component.
        #[cfg(any(target_os = "linux", target_os = "android"))]
        fn open_beneath(&self, rel: &[OsString]) -> io::Result<Option<OwnedFd>> {
            use rustix::fs::ResolveFlags;
            let mut path = OsString::new();
            for (i, c) in rel.iter().enumerate() {
                if i > 0 {
                    path.push("/");
                }
                path.push(c);
            }
            match rustix::fs::openat2(
                &self.fd,
                path.as_os_str(),
                dir_flags(),
                Mode::empty(),
                ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_MAGICLINKS,
            ) {
                Ok(fd) => Ok(Some(fd)),
                Err(Errno::NOSYS | Errno::PERM | Errno::INVAL | Errno::AGAIN) => Ok(None),
                Err(e) => Err(e.into()),
            }
        }

        /// Other Unix systems have no `openat2`: always walk component by component.
        #[cfg(not(any(target_os = "linux", target_os = "android")))]
        #[expect(
            clippy::unnecessary_wraps,
            clippy::unused_self,
            reason = "same signature as the Linux variant"
        )]
        fn open_beneath(&self, _rel: &[OsString]) -> io::Result<Option<OwnedFd>> {
            Ok(None)
        }

        /// `mkdir -p rel` below the root.
        pub(crate) fn mkdirs(&mut self, rel: &[OsString]) -> io::Result<()> {
            self.dir(rel, true).map(drop)
        }

        /// A new regular file at `rel` (an existing regular file of an earlier entry is replaced; anything else in
        /// the way is an error), mode 0600 until [`Tree::set_file_mode`].
        pub(crate) fn create_file(&mut self, rel: &[OsString]) -> io::Result<File> {
            let (parent, name) = split_last(rel)?;
            let pfd = self.dir(parent, true)?;
            let p = pfd.as_ref().map_or_else(|| self.fd.as_fd(), AsFd::as_fd);
            let flags =
                OFlags::RDWR | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC;
            match rustix::fs::openat(p, name.as_os_str(), flags, Mode::RUSR | Mode::WUSR) {
                Ok(fd) => Ok(File::from(fd)),
                Err(Errno::EXIST) => {
                    replace_regular(p, name)?;
                    let fd =
                        rustix::fs::openat(p, name.as_os_str(), flags, Mode::RUSR | Mode::WUSR)?;
                    Ok(File::from(fd))
                }
                Err(e) => Err(e.into()),
            }
        }

        /// A hard link `dst` to the already extracted regular file `src` (`linkat` without `AT_SYMLINK_FOLLOW`).
        pub(crate) fn hard_link(&mut self, src: &[OsString], dst: &[OsString]) -> io::Result<()> {
            let (sparent, sname) = split_last(src)?;
            let (dparent, dname) = split_last(dst)?;
            let sfd = self.dir(sparent, false)?;
            let dfd = self.dir(dparent, true)?;
            let s = sfd.as_ref().map_or_else(|| self.fd.as_fd(), AsFd::as_fd);
            let d = dfd.as_ref().map_or_else(|| self.fd.as_fd(), AsFd::as_fd);
            match rustix::fs::linkat(s, sname.as_os_str(), d, dname.as_os_str(), AtFlags::empty()) {
                Ok(()) => Ok(()),
                Err(Errno::EXIST) => {
                    replace_regular(d, dname)?;
                    rustix::fs::linkat(s, sname.as_os_str(), d, dname.as_os_str(), AtFlags::empty())
                        .map_err(Into::into)
                }
                Err(e) => Err(e.into()),
            }
        }

        /// The symlink `link -> target` (target verbatim; validated by the caller).
        pub(crate) fn symlink(
            &mut self,
            target: &OsString,
            link: &[OsString],
            _target_is_dir: bool,
        ) -> io::Result<()> {
            let (parent, name) = split_last(link)?;
            let pfd = self.dir(parent, true)?;
            let p = pfd.as_ref().map_or_else(|| self.fd.as_fd(), AsFd::as_fd);
            rustix::fs::symlinkat(target.as_os_str(), p, name.as_os_str()).map_err(Into::into)
        }

        /// `chmod` of the directory `rel` (special bits never set).
        pub(crate) fn set_dir_mode(&mut self, rel: &[OsString], m: u32) -> io::Result<()> {
            let fd = self.dir(rel, false)?;
            let f = fd.as_ref().map_or_else(|| self.fd.as_fd(), AsFd::as_fd);
            rustix::fs::fchmod(f, mode(m & 0o777)).map_err(Into::into)
        }

        /// Whether `rel` exists below the root as a directory (not following symlinks).
        pub(crate) fn is_dir(&mut self, rel: &[OsString]) -> bool {
            self.dir(rel, false).is_ok()
        }
    }

    /// `chmod` of an extracted file (special bits never set).
    pub(crate) fn set_file_mode(f: &File, m: u32) -> io::Result<()> {
        rustix::fs::fchmod(f, mode(m & 0o777)).map_err(Into::into)
    }

    /// Removes `name` in `dir` if it is a regular file; anything else is an `AlreadyExists` error.
    fn replace_regular(dir: BorrowedFd<'_>, name: &OsString) -> io::Result<()> {
        let st = rustix::fs::statat(dir, name.as_os_str(), AtFlags::SYMLINK_NOFOLLOW)?;
        if FileType::from_raw_mode(st.st_mode) != FileType::RegularFile {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!(
                    "{} exists and is not a regular file",
                    name.to_string_lossy()
                ),
            ));
        }
        rustix::fs::unlinkat(dir, name.as_os_str(), AtFlags::empty()).map_err(Into::into)
    }

    #[cfg(test)]
    mod tests {
        use super::super::RelPath;
        use super::*;

        #[test]
        fn modes_are_masked() {
            assert_eq!(mode(0o104_755).as_raw_mode(), 0o4755);
            assert_eq!(mode(0o7777).as_raw_mode(), 0o7777);
        }

        #[test]
        fn create_file_never_follows_a_planted_symlink() {
            let d = tempfile::tempdir().unwrap();
            let root = d.path().join("t");
            let mut t = Tree::create(&root).unwrap();
            let outside = d.path().join("outside");
            std::fs::create_dir_all(&outside).unwrap();
            std::os::unix::fs::symlink(&outside, root.join("evil")).unwrap();
            let rel: RelPath = vec!["evil".into(), "x".into()];
            assert!(t.create_file(&rel).is_err());
            assert!(t.mkdirs(&rel).is_err());
            std::os::unix::fs::symlink(outside.join("f"), root.join("f")).unwrap();
            assert!(t.create_file(&["f".into()]).is_err());
            assert!(!outside.join("f").exists());
            assert!(std::fs::read_dir(&outside).unwrap().next().is_none());
        }
    }
}

#[cfg(not(unix))]
mod imp {
    use std::ffi::OsString;
    use std::fs::{File, OpenOptions};
    use std::io;
    use std::path::Path;

    use super::{RelPath, Tree, split_last};

    /// `FILE_ATTRIBUTE_REPARSE_POINT`.
    const REPARSE_POINT: u32 = 0x400;

    /// Refuses a path whose existing components below the root include a symlink or reparse point.
    fn check(tree: &Tree, rel: &[OsString]) -> io::Result<()> {
        use std::os::windows::fs::MetadataExt;
        let mut p = tree.root().to_path_buf();
        for c in rel {
            p.push(c);
            match std::fs::symlink_metadata(&p) {
                Ok(m) if m.file_type().is_symlink() || m.file_attributes() & REPARSE_POINT != 0 => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!("{} is a link", p.display()),
                    ));
                }
                Ok(_) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    #[expect(clippy::multiple_inherent_impl, reason = "the Windows half of Tree")]
    impl Tree {
        /// `mkdir -p rel` below the root.
        pub(crate) fn mkdirs(&mut self, rel: &[OsString]) -> io::Result<()> {
            check(self, rel)?;
            for i in 1..=rel.len() {
                let sub: RelPath = rel.get(..i).map(<[OsString]>::to_vec).unwrap_or_default();
                #[expect(
                    clippy::create_dir,
                    reason = "one component at a time, each checked for links"
                )]
                let created = std::fs::create_dir(self.full(&sub));
                match created {
                    Ok(()) => self.implicit.push(sub),
                    Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                        if !self.full(&sub).is_dir() {
                            return Err(e);
                        }
                    }
                    Err(e) => return Err(e),
                }
            }
            Ok(())
        }

        /// A new regular file at `rel` (an existing regular file of an earlier entry is replaced).
        pub(crate) fn create_file(&mut self, rel: &[OsString]) -> io::Result<File> {
            let (parent, _) = split_last(rel)?;
            self.mkdirs(parent)?;
            check(self, rel)?;
            let full = self.full(rel);
            let open = |p: &Path| OpenOptions::new().write(true).create_new(true).open(p);
            match open(&full) {
                Ok(f) => Ok(f),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                    if !std::fs::symlink_metadata(&full)?.is_file() {
                        return Err(e);
                    }
                    std::fs::remove_file(&full)?;
                    open(&full)
                }
                Err(e) => Err(e),
            }
        }

        /// A hard link `dst` to the already extracted regular file `src`.
        pub(crate) fn hard_link(&mut self, src: &[OsString], dst: &[OsString]) -> io::Result<()> {
            let (parent, _) = split_last(dst)?;
            self.mkdirs(parent)?;
            check(self, src)?;
            check(self, dst)?;
            let d = self.full(dst);
            if std::fs::symlink_metadata(&d).is_ok_and(|m| m.is_file()) {
                std::fs::remove_file(&d)?;
            }
            std::fs::hard_link(self.full(src), d)
        }

        /// The symlink `link -> target` (needs the symlink privilege or developer mode).
        pub(crate) fn symlink(
            &mut self,
            target: &OsString,
            link: &[OsString],
            target_is_dir: bool,
        ) -> io::Result<()> {
            let (parent, _) = split_last(link)?;
            self.mkdirs(parent)?;
            check(self, link)?;
            let t: OsString = target.to_string_lossy().replace('/', "\\").into();
            if target_is_dir {
                std::os::windows::fs::symlink_dir(t, self.full(link))
            } else {
                std::os::windows::fs::symlink_file(t, self.full(link))
            }
        }

        /// Directory modes do not exist on Windows.
        #[expect(
            clippy::unnecessary_wraps,
            clippy::unused_self,
            clippy::needless_pass_by_ref_mut,
            reason = "same signature as on Unix"
        )]
        pub(crate) fn set_dir_mode(&mut self, _rel: &[OsString], _m: u32) -> io::Result<()> {
            Ok(())
        }

        /// Whether `rel` exists below the root as a directory (not a link).
        #[expect(clippy::needless_pass_by_ref_mut, reason = "same signature as on Unix")]
        pub(crate) fn is_dir(&mut self, rel: &[OsString]) -> bool {
            check(self, rel).is_ok() && self.full(rel).is_dir()
        }
    }

    /// The read-only attribute for files without any write bit.
    pub(crate) fn set_file_mode(f: &File, m: u32) -> io::Result<()> {
        if m & 0o222 == 0 {
            let mut perms = f.metadata()?.permissions();
            perms.set_readonly(true);
            f.set_permissions(perms)?;
        }
        Ok(())
    }
}

pub(crate) use imp::set_file_mode;

/// Removes a `.part` tree after a failure: `remove_dir_all` (never follows symlinks), and when a read-only directory
/// refuses, makes every real directory owner-writable and tries again.
pub(crate) fn remove_part(root: &Path) {
    if std::fs::remove_dir_all(root).is_ok() || !root.exists() {
        return;
    }
    make_writable(root);
    let _ignored = std::fs::remove_dir_all(root);
}

/// `u+rwx` on `dir` and every directory below it (iterative; symlinks are not followed); Windows also clears the
/// read-only attribute of files.
fn make_writable(dir: &Path) {
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ignored = std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o700));
        }
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in entries.flatten() {
            let Ok(t) = e.file_type() else {
                continue;
            };
            if t.is_dir() {
                stack.push(e.path());
            }
            #[cfg(not(unix))]
            if !t.is_dir()
                && !t.is_symlink()
                && let Ok(m) = std::fs::symlink_metadata(e.path())
            {
                let mut perms = m.permissions();
                #[expect(
                    clippy::permissions_set_readonly_false,
                    reason = "Windows: clears the read-only attribute"
                )]
                perms.set_readonly(false);
                let _ignored = std::fs::set_permissions(e.path(), perms);
            }
        }
    }
}
