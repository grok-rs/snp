//! `os.path` as every caller uses it: [`crate::posixpath`] on Unix, [`crate::ntpath`] over UTF-16
//! units on Windows, on `OsStr`/`Path` with an explicit cwd (snp never chdirs).

use std::borrow::Cow;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use crate::PathError;

/// Home-directory lookup injected by snp-os: the environment snapshot, and (Unix only) the passwd
/// entries of the real uid (`pw_current`, used only when `HOME` is unset) and of `~user`.
///
/// `!Send + !Sync` by design (ARCHITECTURE §1.3): it borrows plain `&dyn Fn` lookups for one
/// `expanduser` call on the calling thread.
pub struct HomeLookup<'a> {
    /// `os.environ.get(name)`.
    pub env: &'a dyn Fn(&str) -> Option<OsString>,
    /// `pwd.getpwuid(os.getuid()).pw_dir`.
    pub pw_current: &'a dyn Fn() -> Option<OsString>,
    /// `pwd.getpwnam(name).pw_dir`.
    pub pw_named: &'a dyn Fn(&OsStr) -> Option<OsString>,
}

impl fmt::Debug for HomeLookup<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HomeLookup").finish_non_exhaustive()
    }
}

#[cfg(unix)]
mod imp {
    use std::ffi::{OsStr, OsString};
    use std::os::unix::ffi::{OsStrExt, OsStringExt};

    pub(super) fn units(s: &OsStr) -> &[u8] {
        s.as_bytes()
    }
    pub(super) fn os(v: Vec<u8>) -> OsString {
        OsString::from_vec(v)
    }
    pub(super) fn os_ref(v: &[u8]) -> &OsStr {
        OsStr::from_bytes(v)
    }
}

#[cfg(windows)]
mod imp {
    use std::ffi::{OsStr, OsString};
    use std::os::windows::ffi::{OsStrExt, OsStringExt};

    pub(super) fn units(s: &OsStr) -> Vec<u16> {
        s.encode_wide().collect()
    }
    pub(super) fn os(v: impl AsRef<[u16]>) -> OsString {
        OsString::from_wide(v.as_ref())
    }
}

/// `os.path.normpath(p)`.
#[must_use]
pub fn normpath(p: &OsStr) -> OsString {
    #[cfg(unix)]
    {
        imp::os(crate::posixpath::normpath(imp::units(p)))
    }
    #[cfg(windows)]
    {
        imp::os(crate::ntpath::normpath(&imp::units(p)))
    }
}

/// `os.path.join(a, *rest)`.
#[must_use]
pub fn join(a: &OsStr, rest: &[&OsStr]) -> OsString {
    #[cfg(unix)]
    {
        let rest: Vec<&[u8]> = rest.iter().map(|r| imp::units(r)).collect();
        imp::os(crate::posixpath::join(imp::units(a), &rest))
    }
    #[cfg(windows)]
    {
        let owned: Vec<Vec<u16>> = rest.iter().map(|r| imp::units(r)).collect();
        let rest: Vec<&[u16]> = owned.iter().map(Vec::as_slice).collect();
        imp::os(crate::ntpath::join(&imp::units(a), &rest))
    }
}

/// `os.path.split(p)`.
#[must_use]
pub fn split(p: &OsStr) -> (OsString, OsString) {
    #[cfg(unix)]
    {
        let (h, t) = crate::posixpath::split(imp::units(p));
        (imp::os(h.to_vec()), imp::os(t.to_vec()))
    }
    #[cfg(windows)]
    {
        let u = imp::units(p);
        let (h, t) = crate::ntpath::split(&u);
        (imp::os(h), imp::os(t))
    }
}

/// `os.path.dirname(p)`.
#[must_use]
pub fn dirname(p: &OsStr) -> OsString {
    split(p).0
}

/// `os.path.basename(p)`.
#[must_use]
pub fn basename(p: &OsStr) -> OsString {
    split(p).1
}

/// `os.path.isabs(p)` (CPython 3.12 semantics on both flavours).
#[must_use]
pub fn isabs(p: &OsStr) -> bool {
    #[cfg(unix)]
    {
        crate::posixpath::isabs(imp::units(p))
    }
    #[cfg(windows)]
    {
        crate::ntpath::isabs_312(&imp::units(p))
    }
}

/// `os.path.abspath(p)` against an explicit `cwd`. Unix: lexical `normpath(join(cwd, p))`.
/// Windows: `GetFullPathNameW(normpath(…))` through `std::path::absolute`, so drive-relative paths
/// of another drive (`D:x`) resolve against that drive's cwd as in CPython.
#[must_use]
pub fn abspath(p: &OsStr, cwd: &Path) -> PathBuf {
    #[cfg(unix)]
    {
        PathBuf::from(imp::os(crate::posixpath::abspath(
            imp::units(p),
            imp::units(cwd.as_os_str()),
        )))
    }
    #[cfg(windows)]
    {
        use crate::ntpath;
        let u = imp::units(p);
        let c = imp::units(cwd.as_os_str());
        let (pd, pr, _) = ntpath::splitroot(&u);
        let (cd, _, _) = ntpath::splitroot(&c);
        let other_drive =
            !pd.is_empty() && pr.is_empty() && ntpath::normcase(pd) != ntpath::normcase(cd);
        let joined = if ntpath::isabs_313(&u) || other_drive {
            u.clone()
        } else {
            ntpath::join(&c, &[&u])
        };
        let norm = imp::os(ntpath::normpath(&joined));
        std::path::absolute(&norm).unwrap_or_else(|_| PathBuf::from(norm))
    }
}

/// `os.path.relpath(p, start)` against an explicit `cwd` (`start = None` means the cwd).
pub fn relpath(p: &OsStr, start: Option<&OsStr>, cwd: &Path) -> Result<OsString, PathError> {
    #[cfg(unix)]
    {
        crate::posixpath::relpath(
            imp::units(p),
            start.map(imp::units),
            imp::units(cwd.as_os_str()),
        )
        .map(imp::os)
    }
    #[cfg(windows)]
    {
        let start = start.map(imp::units);
        crate::ntpath::relpath(
            &imp::units(p),
            start.as_deref(),
            &imp::units(cwd.as_os_str()),
        )
        .map(imp::os)
    }
}

/// `os.path.commonpath(paths)`.
pub fn commonpath(ps: &[&OsStr]) -> Result<OsString, PathError> {
    #[cfg(unix)]
    {
        let v: Vec<&[u8]> = ps.iter().map(|p| imp::units(p)).collect();
        crate::posixpath::commonpath(&v).map(imp::os)
    }
    #[cfg(windows)]
    {
        let owned: Vec<Vec<u16>> = ps.iter().map(|p| imp::units(p)).collect();
        let v: Vec<&[u16]> = owned.iter().map(Vec::as_slice).collect();
        crate::ntpath::commonpath(&v).map(imp::os)
    }
}

/// `os.path.normcase(p)`: identity on Unix; `/` → `\` + lowercase on Windows.
#[must_use]
pub fn normcase(p: &OsStr) -> OsString {
    #[cfg(unix)]
    {
        p.to_owned()
    }
    #[cfg(windows)]
    {
        imp::os(crate::ntpath::normcase(&imp::units(p)))
    }
}

/// `os.path.expanduser(p)` with the injected [`HomeLookup`].
#[must_use]
pub fn expanduser<'a>(p: &'a OsStr, home: &HomeLookup<'_>) -> Cow<'a, OsStr> {
    #[cfg(unix)]
    {
        let home_env = (home.env)("HOME");
        let pw_current = || (home.pw_current)().map(|h| imp::units(&h).to_vec());
        let pw_named = |u: &[u8]| (home.pw_named)(imp::os_ref(u)).map(|h| imp::units(&h).to_vec());
        match crate::posixpath::expanduser(
            imp::units(p),
            home_env.as_deref().map(imp::units),
            &pw_current,
            &pw_named,
        ) {
            Cow::Borrowed(b) => Cow::Borrowed(imp::os_ref(b)),
            Cow::Owned(v) => Cow::Owned(imp::os(v)),
        }
    }
    #[cfg(windows)]
    {
        let env = |k: &str| (home.env)(k).map(|v| imp::units(&v));
        let u = imp::units(p);
        match crate::ntpath::expanduser(&u, &env) {
            Cow::Borrowed(_) => Cow::Borrowed(p),
            Cow::Owned(v) => Cow::Owned(imp::os(v)),
        }
    }
}

/// Non-strict `os.path.realpath(p)` against an explicit `cwd`.
pub fn realpath(p: &OsStr, cwd: &Path) -> io::Result<PathBuf> {
    #[cfg(unix)]
    {
        crate::posixpath::realpath(imp::units(p), imp::units(cwd.as_os_str()))
            .map(|v| PathBuf::from(imp::os(v)))
    }
    #[cfg(windows)]
    {
        crate::ntpath::realpath(p, cwd)
    }
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use super::*;

    #[cfg(unix)]
    #[test]
    fn facade_matches_posixpath() {
        let cwd = Path::new("/r/sub");
        assert_eq!(normpath(OsStr::new("a//b/../c")), OsString::from("a/c"));
        assert_eq!(
            join(OsStr::new("a"), &[OsStr::new("b"), OsStr::new("/c")]),
            OsString::from("/c")
        );
        assert_eq!(
            split(OsStr::new("a/b")),
            (OsString::from("a"), OsString::from("b"))
        );
        assert_eq!(abspath(OsStr::new("../x"), cwd), PathBuf::from("/r/x"));
        assert_eq!(
            relpath(OsStr::new("/r/a"), None, cwd),
            Ok(OsString::from("../a"))
        );
        assert_eq!(
            commonpath(&[OsStr::new("/a/b"), OsStr::new("/a/c")]),
            Ok(OsString::from("/a"))
        );
        assert!(isabs(OsStr::new("/x")) && !isabs(OsStr::new("x")));
        assert_eq!(normcase(OsStr::new("A/B")), OsString::from("A/B"));
        let env = |k: &str| (k == "HOME").then(|| OsString::from("/home/u"));
        let none = || None;
        let named = |_: &OsStr| None;
        let home = HomeLookup {
            env: &env,
            pw_current: &none,
            pw_named: &named,
        };
        assert_eq!(
            expanduser(OsStr::new("~/.cache"), &home),
            OsStr::new("/home/u/.cache")
        );
        assert!(matches!(
            expanduser(OsStr::new("x"), &home),
            Cow::Borrowed(_)
        ));
        assert!(format!("{home:?}").starts_with("HomeLookup"));
    }
}
