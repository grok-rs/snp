//! The filesystem steps of the stash's restore by rename (ADR-152): whether a file carries extended attributes
//! (which a fresh file would not get back, and which also hold POSIX ACLs and security labels), and a
//! `futimens(UTIME_NOW, UTIME_NOW)` that stamps a file the way a fresh write does (the kernel's clock, the same one
//! the rename's ctime comes from).
//!
//! Linux only: elsewhere every call answers [`io::ErrorKind::Unsupported`], and the stash keeps `git apply`.
//!
//! Owner: WP-15.

use std::fs::File;
use std::io;
use std::path::Path;

/// Whether `path` itself (not a symlink's target) has no extended attribute at all: `llistxattr` reports an empty
/// name list. A filesystem without xattr support (`ENOTSUP`) has none.
pub fn path_has_no_xattrs(path: &Path) -> io::Result<bool> {
    imp::path_has_no_xattrs(path)
}

/// [`path_has_no_xattrs`] for an open file (`flistxattr`).
pub fn file_has_no_xattrs(f: &File) -> io::Result<bool> {
    imp::file_has_no_xattrs(f)
}

/// Sets the access and modification times of `f` to the kernel's current time (`futimens` with `UTIME_NOW`), as
/// writing a fresh file leaves them.
pub fn touch_now(f: &File) -> io::Result<()> {
    imp::touch_now(f)
}

#[cfg(any(target_os = "linux", target_os = "android"))]
mod imp {
    use std::fs::File;
    use std::io;
    use std::path::Path;

    use rustix::fs::{Timespec, Timestamps, UTIME_NOW};

    /// An empty list, or no xattr support on the filesystem.
    fn none_listed(r: rustix::io::Result<usize>) -> io::Result<bool> {
        match r {
            Ok(n) => Ok(n == 0),
            Err(rustix::io::Errno::NOTSUP) => Ok(true),
            Err(e) => Err(e.into()),
        }
    }

    pub(super) fn path_has_no_xattrs(path: &Path) -> io::Result<bool> {
        none_listed(rustix::fs::llistxattr_query(path))
    }

    pub(super) fn file_has_no_xattrs(f: &File) -> io::Result<bool> {
        none_listed(rustix::fs::flistxattr_query(f))
    }

    pub(super) fn touch_now(f: &File) -> io::Result<()> {
        let now = Timespec {
            tv_sec: 0,
            tv_nsec: UTIME_NOW,
        };
        rustix::fs::futimens(
            f,
            &Timestamps {
                last_access: now,
                last_modification: now,
            },
        )
        .map_err(io::Error::from)
    }
}

#[cfg(not(any(target_os = "linux", target_os = "android")))]
mod imp {
    use std::fs::File;
    use std::io;
    use std::path::Path;

    pub(super) fn path_has_no_xattrs(_path: &Path) -> io::Result<bool> {
        Err(io::ErrorKind::Unsupported.into())
    }

    pub(super) fn file_has_no_xattrs(_f: &File) -> io::Result<bool> {
        Err(io::ErrorKind::Unsupported.into())
    }

    pub(super) fn touch_now(_f: &File) -> io::Result<()> {
        Err(io::ErrorKind::Unsupported.into())
    }
}

#[cfg(test)]
#[cfg(any(target_os = "linux", target_os = "android"))]
mod tests {
    use std::os::unix::fs::MetadataExt;

    use super::*;

    #[test]
    fn a_plain_temp_file_has_no_xattrs_and_touch_moves_its_times() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("f");
        std::fs::write(&p, b"x").unwrap();
        // a labelled system (SELinux) gives every file an attribute: then the check is simply `false`
        let plain = path_has_no_xattrs(&p).unwrap();
        let f = File::open(&p).unwrap();
        assert_eq!(file_has_no_xattrs(&f).unwrap(), plain);
        let old = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000);
        f.set_times(
            std::fs::FileTimes::new()
                .set_modified(old)
                .set_accessed(old),
        )
        .unwrap();
        assert_eq!(std::fs::metadata(&p).unwrap().mtime(), 1_000_000);
        touch_now(&f).unwrap();
        let md = std::fs::metadata(&p).unwrap();
        assert!(md.mtime() > 1_000_000 && md.atime() > 1_000_000, "{md:?}");
        assert!(path_has_no_xattrs(&d.path().join("missing")).is_err());
    }
}
