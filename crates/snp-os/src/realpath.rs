//! Non-strict `os.path.realpath`.
//!
//! Owner: WP-15.

use std::io;
use std::path::{Path, PathBuf};

/// posix `_joinrealpath` / nt rules (15 §A4.9-10), with an explicit cwd.
///
/// `os.path.realpath(p)` (non-strict, CPython 3.12): POSIX resolves symlinks component by component, keeps a link
/// loop or a missing tail unresolved, never fails for missing paths and returns an absolute path; Windows normalises
/// first (`..` is lexical), resolves the existing prefix to its final path and re-appends the missing tail. Used for
/// the store location only (never for cwd, config or file arguments: 14 P-61/P-64). An error from `readlink` after
/// `lstat` reported a link propagates (Python does the same). Delegates to `snp_pycompat::ospath::realpath`.
///
/// Runtime profile: CPython 3.12 (COMPATIBILITY.md, the `pc462-py312` oracle). CPython 3.13 rewrote the POSIX
/// `realpath` loop: after a symlink loop it keeps resolving the remaining components where 3.12 appends them
/// unresolved (`loop//a` → `/a` on 3.12, `$ROOT/loop/a` on 3.13; `self/../dangling` → `$ROOT/dangling` vs
/// `$ROOT/nonexistent`). Only paths through a symlink loop differ; snp keeps the 3.12 results (pinned by the
/// loop vectors below).
pub fn realpath_nonstrict(p: &Path, cwd: &Path) -> io::Result<PathBuf> {
    snp_pycompat::ospath::realpath(p.as_os_str(), cwd)
}

#[cfg(test)]
#[cfg(unix)]
mod tests {
    use super::*;

    /// V-RP: `posixpath.realpath` (non-strict) on the 15 §A4.9 fixture.
    #[cfg(unix)]
    #[test]
    fn v_rp_realpath_vectors() {
        use std::os::unix::fs::symlink;
        let d = tempfile::tempdir().unwrap();
        // The fixture root must itself be free of symlinks (macOS /var → /private/var).
        let root = realpath_nonstrict(d.path(), Path::new("/")).unwrap();
        std::fs::create_dir_all(root.join("a/b")).unwrap();
        std::fs::write(root.join("a/f"), b"").unwrap();
        symlink("a", root.join("link_a")).unwrap();
        symlink(root.join("a/b"), root.join("link_abs")).unwrap();
        symlink("a/b/..", root.join("link_dotdot")).unwrap();
        symlink("loop2", root.join("loop1")).unwrap();
        symlink("loop1", root.join("loop2")).unwrap();
        symlink("self", root.join("self")).unwrap();
        symlink("nonexistent", root.join("dangling")).unwrap();
        symlink("a/f", root.join("link_file")).unwrap();
        symlink("link_a", root.join("chain")).unwrap();
        symlink("../..", root.join("a/b/up")).unwrap();
        let r = root.to_str().unwrap().to_owned();
        let rp = |input: &str| -> String {
            let input = input.replace("$ROOT", &r);
            let got = realpath_nonstrict(Path::new(&input), &root).unwrap();
            got.to_str().unwrap().replace(&r, "$ROOT")
        };
        let table = [
            ("$ROOT", "$ROOT"),
            ("$ROOT/a", "$ROOT/a"),
            ("$ROOT/link_a", "$ROOT/a"),
            ("$ROOT/link_a/b", "$ROOT/a/b"),
            ("$ROOT/link_a/..", "$ROOT"),
            ("$ROOT/link_abs", "$ROOT/a/b"),
            ("$ROOT/link_abs/..", "$ROOT/a"),
            ("$ROOT/link_dotdot", "$ROOT/a"),
            ("$ROOT/loop1", "$ROOT/loop1"),
            ("$ROOT/loop1/x", "$ROOT/loop1/x"),
            ("$ROOT/self", "$ROOT/self"),
            ("$ROOT/dangling", "$ROOT/nonexistent"),
            ("$ROOT/dangling/x", "$ROOT/nonexistent/x"),
            ("$ROOT/link_file", "$ROOT/a/f"),
            ("$ROOT/link_file/x", "$ROOT/a/f/x"),
            ("$ROOT/chain/b", "$ROOT/a/b"),
            ("$ROOT/missing/../a", "$ROOT/a"),
            ("$ROOT/missing/x", "$ROOT/missing/x"),
            ("$ROOT/a/b/up", "$ROOT"),
            ("$ROOT/a/b/up/a", "$ROOT/a"),
            ("$ROOT/./a//b/", "$ROOT/a/b"),
            ("link_a", "$ROOT/a"),
            (".", "$ROOT"),
            ("", "$ROOT"),
            ("missing", "$ROOT/missing"),
            ("$ROOT/a/f/..", "$ROOT/a"),
            ("$ROOT/missing/../link_a", "$ROOT/a"),
            ("$ROOT/missing/../dangling", "$ROOT/nonexistent"),
            ("$ROOT/link_a/../link_file", "$ROOT/a/f"),
            // Symlink loops followed by `..` / `//` (CPython 3.12.3; 3.13 differs, see the function docs).
            ("loop1/../a", "$ROOT/a"),
            ("loop1//a", "/a"),
            ("loop1//link_a", "/link_a"),
            ("self//a", "/a"),
            ("self/../dangling", "$ROOT/dangling"),
            ("loop2/../link_abs", "$ROOT/link_abs"),
            ("$ROOT/loop1/../link_abs", "$ROOT/link_abs"),
            ("loop1/x/../..", "$ROOT"),
            ("self/..//link_a", "$ROOT/link_a"),
            ("loop1/..", "$ROOT"),
        ];
        for (input, want) in table {
            assert_eq!(rp(input), want, "realpath({input:?})");
        }
    }
}
