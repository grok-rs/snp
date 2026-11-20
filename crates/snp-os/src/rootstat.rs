//! `lstat` relative to one directory descriptor (the `DiffOracle`'s index-stat proof, ADR-134): the kernel resolves
//! each repository path from the work-tree root's descriptor instead of walking the root's own components again for
//! every call (measured: half the kernel instructions of the absolute-path pass).
//!
//! Unix only; elsewhere [`RootDir::open`] always fails, and the proof never runs.
//!
//! Owner: WP-15.

use std::ffi::CStr;
use std::io;
use std::path::Path;

/// The fields of one `lstat` result the proof compares, widened to 64 bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatData {
    /// `st_mode` (type and permission bits).
    pub mode: u32,
    /// `st_ino`.
    pub ino: u64,
    /// `st_uid`.
    pub uid: u32,
    /// `st_gid`.
    pub gid: u32,
    /// `st_size`.
    pub size: u64,
    /// `st_mtime` seconds and nanoseconds.
    pub mtime: (i64, i64),
    /// `st_ctime` seconds and nanoseconds.
    pub ctime: (i64, i64),
}

/// A directory opened for `*at` calls (`O_PATH` on Linux, read-only elsewhere), close-on-exec.
#[derive(Debug)]
pub struct RootDir {
    #[cfg(unix)]
    fd: std::os::fd::OwnedFd,
}

impl RootDir {
    /// Opens `dir`.
    #[cfg(unix)]
    pub fn open(dir: &Path) -> io::Result<Self> {
        use rustix::fs::{Mode, OFlags};
        #[cfg(any(target_os = "linux", target_os = "android"))]
        let flags = OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC;
        #[cfg(not(any(target_os = "linux", target_os = "android")))]
        let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC;
        let fd = rustix::fs::open(dir, flags, Mode::empty())?;
        Ok(Self { fd })
    }

    /// Never on this platform.
    #[cfg(not(unix))]
    pub fn open(_dir: &Path) -> io::Result<Self> {
        Err(io::Error::from(io::ErrorKind::Unsupported))
    }

    /// `fstatat(dir, name, AT_SYMLINK_NOFOLLOW)`; a field that does not fit (never on supported targets) is an
    /// `InvalidData` error.
    #[cfg(unix)]
    pub fn lstat(&self, name: &CStr) -> io::Result<StatData> {
        use rustix::fs::AtFlags;
        let st = rustix::fs::statat(&self.fd, name, AtFlags::SYMLINK_NOFOLLOW)?;
        let bad = || io::Error::from(io::ErrorKind::InvalidData);
        let i = |v: i128| i64::try_from(v).map_err(|_| bad());
        Ok(StatData {
            // through i128: the field types differ between targets (a `u16` mode on macOS)
            mode: u32::try_from(i128::from(st.st_mode)).map_err(|_| bad())?,
            ino: u64::try_from(i128::from(st.st_ino)).map_err(|_| bad())?,
            uid: st.st_uid,
            gid: st.st_gid,
            size: u64::try_from(st.st_size).map_err(|_| bad())?,
            mtime: (
                i(i128::from(st.st_mtime))?,
                i(i128::from(st.st_mtime_nsec))?,
            ),
            ctime: (
                i(i128::from(st.st_ctime))?,
                i(i128::from(st.st_ctime_nsec))?,
            ),
        })
    }

    /// Never on this platform.
    #[cfg(not(unix))]
    pub fn lstat(&self, _name: &CStr) -> io::Result<StatData> {
        Err(io::Error::from(io::ErrorKind::Unsupported))
    }
}

#[cfg(test)]
#[cfg(unix)]
mod tests {
    use std::os::unix::fs::MetadataExt as _;

    use super::*;

    #[test]
    fn lstat_matches_std_and_does_not_follow() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("sub")).unwrap();
        std::fs::write(d.path().join("sub/f"), b"abc").unwrap();
        std::os::unix::fs::symlink("sub", d.path().join("link")).unwrap();
        let root = RootDir::open(d.path()).unwrap();
        let st = root.lstat(c"sub/f").unwrap();
        let md = std::fs::symlink_metadata(d.path().join("sub/f")).unwrap();
        assert_eq!(st.ino, md.ino());
        assert_eq!(st.size, 3);
        assert_eq!(st.mode, md.mode());
        assert_eq!(st.mtime, (md.mtime(), md.mtime_nsec()));
        assert_eq!(st.ctime, (md.ctime(), md.ctime_nsec()));
        assert_eq!((st.uid, st.gid), (md.uid(), md.gid()));
        let link = root.lstat(c"link").unwrap();
        assert_eq!(link.mode & 0o170_000, 0o120_000);
        // a leading symlink is resolved by the kernel: the caller checks every ancestor itself
        assert!(root.lstat(c"link/f").is_ok());
        let missing = root.lstat(c"nope").unwrap_err();
        assert_eq!(missing.kind(), io::ErrorKind::NotFound);
        assert!(RootDir::open(&d.path().join("sub/f")).is_err());
    }
}
