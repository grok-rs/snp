//! File metadata relevant to identification: kind and executability.

use std::fs;

/// The kind of a file-system entry, as seen by `lstat` (symlinks are not
/// followed).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum FileKind {
    /// A regular file.
    Regular,
    /// A directory (also used for git submodules / gitlinks).
    Directory,
    /// A symbolic link.
    Symlink,
    /// A Unix domain socket.
    Socket,
    /// Anything else: FIFO, character or block device, … identify treats
    /// these as files and opens them (which can block forever on a FIFO);
    /// this crate tags them `file` without ever opening them.
    Other,
}

impl FileKind {
    /// Classify a [`fs::FileType`] (from [`fs::symlink_metadata`]).
    #[must_use]
    pub fn from_file_type(file_type: fs::FileType) -> Self {
        if file_type.is_dir() {
            Self::Directory
        } else if file_type.is_symlink() {
            Self::Symlink
        } else if is_socket(file_type) {
            Self::Socket
        } else if file_type.is_file() {
            Self::Regular
        } else {
            Self::Other
        }
    }
}

#[cfg(unix)]
fn is_socket(file_type: fs::FileType) -> bool {
    use std::os::unix::fs::FileTypeExt;
    file_type.is_socket()
}

#[cfg(not(unix))]
fn is_socket(_file_type: fs::FileType) -> bool {
    false
}

/// How to decide whether a file is executable.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ExecPolicy {
    /// identify's `os.access(path, os.X_OK)`, so results match pre-commit:
    ///
    /// * Unix: `access(2)` semantics evaluated from the file's mode, owner
    ///   and group against the process's real uid, gid and supplementary
    ///   groups (queried once and cached): root needs any `x` bit, the owner
    ///   the user bit, group members the group bit, others the other bit.
    ///   ACLs and `noexec` mounts are not consulted.
    /// * Windows: every file is executable (`os.access` ignores `X_OK`
    ///   there), so every unknown-name file is checked for a shebang.
    #[default]
    PythonCompat,
    /// Any `x` permission bit (`mode & 0o111 != 0`), git's notion of an
    /// executable file. Never executable on non-Unix platforms.
    ModeBits,
}

impl ExecPolicy {
    /// Whether a regular file with this metadata counts as executable.
    #[must_use]
    pub fn is_executable(self, metadata: &fs::Metadata) -> bool {
        imp::is_executable(self, metadata)
    }
}

#[cfg(unix)]
mod imp {
    use super::ExecPolicy;
    use std::fs;
    use std::os::unix::fs::MetadataExt;
    use std::sync::OnceLock;

    /// The process's real uid, gid and supplementary groups, queried lazily
    /// (most files are owned by the current user, so the group list is
    /// rarely needed) and cached for the life of the process.
    fn uid() -> u32 {
        static UID: OnceLock<u32> = OnceLock::new();
        *UID.get_or_init(|| rustix::process::getuid().as_raw())
    }

    fn in_group(gid: u32) -> bool {
        static GID: OnceLock<u32> = OnceLock::new();
        static GROUPS: OnceLock<Box<[u32]>> = OnceLock::new();
        *GID.get_or_init(|| rustix::process::getgid().as_raw()) == gid
            || GROUPS
                .get_or_init(|| {
                    rustix::process::getgroups()
                        .map(|groups| {
                            groups
                                .into_iter()
                                .map(rustix::process::Gid::as_raw)
                                .collect()
                        })
                        .unwrap_or_default()
                })
                .contains(&gid)
    }

    pub(super) fn is_executable(policy: ExecPolicy, md: &fs::Metadata) -> bool {
        let mode = md.mode();
        match policy {
            ExecPolicy::ModeBits => mode & 0o111 != 0,
            ExecPolicy::PythonCompat => {
                let uid = uid();
                if uid == 0 {
                    mode & 0o111 != 0
                } else if md.uid() == uid {
                    mode & 0o100 != 0
                } else if in_group(md.gid()) {
                    mode & 0o010 != 0
                } else {
                    mode & 0o001 != 0
                }
            }
        }
    }
}

#[cfg(not(unix))]
mod imp {
    use super::ExecPolicy;
    use std::fs;

    pub(super) fn is_executable(policy: ExecPolicy, _md: &fs::Metadata) -> bool {
        match policy {
            ExecPolicy::PythonCompat => cfg!(windows),
            ExecPolicy::ModeBits => false,
        }
    }
}

/// What identification needs to know about a path besides its name: its
/// [`FileKind`] and whether it is executable.
///
/// Build it from [`fs::Metadata`] ([`Meta::from_metadata`]), from a git index
/// mode without any system call ([`Meta::from_git_mode`]), or by hand.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Meta {
    kind: FileKind,
    executable: bool,
}

impl Meta {
    /// A meta with the given kind and executable flag.
    #[must_use]
    pub const fn new(kind: FileKind, executable: bool) -> Self {
        Self { kind, executable }
    }

    /// A regular file.
    #[must_use]
    pub const fn regular(executable: bool) -> Self {
        Self::new(FileKind::Regular, executable)
    }

    /// The entry kind.
    #[must_use]
    pub const fn kind(self) -> FileKind {
        self.kind
    }

    /// Whether the entry is executable (only meaningful for regular files
    /// and [`FileKind::Other`]).
    #[must_use]
    pub const fn is_executable(self) -> bool {
        self.executable
    }

    /// The same meta with a different executable flag.
    #[must_use]
    pub const fn with_executable(self, executable: bool) -> Self {
        Self::new(self.kind, executable)
    }

    /// From `lstat` metadata ([`fs::symlink_metadata`]); executability is
    /// decided by `policy` for regular files and [`FileKind::Other`].
    #[must_use]
    pub fn from_metadata(metadata: &fs::Metadata, policy: ExecPolicy) -> Self {
        let kind = FileKind::from_file_type(metadata.file_type());
        let executable =
            matches!(kind, FileKind::Regular | FileKind::Other) && policy.is_executable(metadata);
        Self::new(kind, executable)
    }

    /// From a git index / tree mode, with git's own executable bit:
    ///
    /// | mode | meta |
    /// |---|---|
    /// | `100644` (any `100xxx` without `x` bits) | regular file |
    /// | `100755` (any `100xxx` with an `x` bit) | executable regular file |
    /// | `120000` | symlink |
    /// | `160000` (gitlink / submodule), `040000` (tree) | directory |
    /// | anything else | `None` |
    ///
    /// This needs no system call. It matches what identify would see on a
    /// checkout where the work tree agrees with the index (e.g. after
    /// pre-commit stashed unstaged changes) on Unix with
    /// `core.fileMode=true`. On Windows identify considers every file
    /// executable; use [`Meta::from_git_mode_with`] with
    /// [`ExecPolicy::PythonCompat`] to reproduce that.
    ///
    /// ```
    /// use file_identify::{FileKind, Meta};
    ///
    /// assert_eq!(Meta::from_git_mode(0o100755), Some(Meta::regular(true)));
    /// assert_eq!(Meta::from_git_mode(0o120000).map(Meta::kind), Some(FileKind::Symlink));
    /// assert_eq!(Meta::from_git_mode(0), None);
    /// ```
    #[must_use]
    pub const fn from_git_mode(mode: u32) -> Option<Self> {
        match mode & 0o170_000 {
            0o100_000 => Some(Self::regular(mode & 0o111 != 0)),
            0o120_000 => Some(Self::new(FileKind::Symlink, false)),
            0o160_000 | 0o040_000 => Some(Self::new(FileKind::Directory, false)),
            _ => None,
        }
    }

    /// Like [`Meta::from_git_mode`], with platform executable semantics:
    /// [`ExecPolicy::PythonCompat`] on Windows marks every regular file
    /// executable (as identify does); otherwise git's bit is used.
    #[must_use]
    pub fn from_git_mode_with(mode: u32, policy: ExecPolicy) -> Option<Self> {
        let meta = Self::from_git_mode(mode)?;
        Some(match (policy, meta.kind) {
            (ExecPolicy::PythonCompat, FileKind::Regular) if cfg!(windows) => {
                meta.with_executable(true)
            }
            _ => meta,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_modes() {
        assert_eq!(Meta::from_git_mode(0o100_644), Some(Meta::regular(false)));
        assert_eq!(Meta::from_git_mode(0o100_755), Some(Meta::regular(true)));
        assert_eq!(Meta::from_git_mode(0o100_664), Some(Meta::regular(false)));
        assert_eq!(
            Meta::from_git_mode(0o120_000),
            Some(Meta::new(FileKind::Symlink, false))
        );
        assert_eq!(
            Meta::from_git_mode(0o160_000),
            Some(Meta::new(FileKind::Directory, false))
        );
        assert_eq!(
            Meta::from_git_mode(0o040_000),
            Some(Meta::new(FileKind::Directory, false))
        );
        assert_eq!(Meta::from_git_mode(0o010_644), None);
        assert_eq!(Meta::from_git_mode(u32::MAX), None);
        let compat = Meta::from_git_mode_with(0o100_644, ExecPolicy::PythonCompat);
        assert_eq!(compat, Some(Meta::regular(cfg!(windows))));
        assert_eq!(
            Meta::from_git_mode_with(0o100_644, ExecPolicy::ModeBits),
            Some(Meta::regular(false))
        );
    }

    #[test]
    fn accessors() {
        let m = Meta::new(FileKind::Other, true);
        assert_eq!(m.kind(), FileKind::Other);
        assert!(m.is_executable());
        assert!(!m.with_executable(false).is_executable());
        assert_eq!(ExecPolicy::default(), ExecPolicy::PythonCompat);
    }

    #[cfg(unix)]
    #[test]
    fn unix_exec_policies() -> std::io::Result<()> {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("f");
        fs::write(&path, "x")?;
        let is_root = rustix::process::getuid().is_root();
        for (mode, compat, bits) in [
            (0o644, false, false),
            (0o744, true, true),
            (0o654, is_root, true),
            (0o645, is_root, true),
            (0o755, true, true),
        ] {
            fs::set_permissions(&path, fs::Permissions::from_mode(mode))?;
            let md = fs::symlink_metadata(&path)?;
            assert_eq!(
                ExecPolicy::PythonCompat.is_executable(&md),
                compat,
                "{mode:o}"
            );
            assert_eq!(ExecPolicy::ModeBits.is_executable(&md), bits, "{mode:o}");
            assert_eq!(
                Meta::from_metadata(&md, ExecPolicy::PythonCompat),
                Meta::regular(compat)
            );
        }
        let md = fs::symlink_metadata(dir.path())?;
        assert_eq!(
            Meta::from_metadata(&md, ExecPolicy::PythonCompat),
            Meta::new(FileKind::Directory, false)
        );
        Ok(())
    }
}
