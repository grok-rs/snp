//! `os.path.samefile`.
//!
//! Owner: WP-15.

use std::io;
use std::path::Path;

/// `os.path.samefile(a, b)`: same device and inode (Windows: volume serial and file index).
///
/// `genericpath.samefile` (`os.stat` of both, following symlinks; an error for a missing path propagates), through
/// the `same-file` crate (`dev`/`ino` on Unix, `GetFileInformationByHandle` on Windows).
pub fn same_file(a: &Path, b: &Path) -> io::Result<bool> {
    same_file::is_same_file(a, b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_and_different_files() {
        let d = tempfile::tempdir().unwrap();
        let a = d.path().join("a");
        let b = d.path().join("b");
        std::fs::write(&a, b"x").unwrap();
        std::fs::write(&b, b"x").unwrap();
        assert!(same_file(&a, &a).unwrap());
        assert!(same_file(&a, &d.path().join(".").join("a")).unwrap());
        assert!(!same_file(&a, &b).unwrap());
        assert!(same_file(&a, &d.path().join("missing")).is_err());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&a, d.path().join("l")).unwrap();
            assert!(same_file(&a, &d.path().join("l")).unwrap());
            std::fs::hard_link(&a, d.path().join("h")).unwrap();
            assert!(same_file(&a, &d.path().join("h")).unwrap());
        }
    }
}
