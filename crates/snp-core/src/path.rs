//! Repository-relative paths as raw bytes (git `-z` output; D-CRASH-12, ADR-052).

use std::borrow::Cow;
use std::ffi::OsStr;

/// A repo-relative, `/`-separated path as raw bytes (Unix paths may be non-UTF-8). Never empty,
/// never absolute, never containing NUL.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct RepoPath(Box<[u8]>);

impl RepoPath {
    /// Validates `b`: rejects empty, NUL-containing and absolute paths (a leading `/`; on Windows
    /// also a leading `\` or a drive prefix such as `C:`).
    pub fn new(b: &[u8]) -> Option<Self> {
        if b.is_empty() || b.contains(&0) || b.first() == Some(&b'/') {
            return None;
        }
        if cfg!(windows)
            && (b.first() == Some(&b'\\')
                || (b.get(1) == Some(&b':') && b.first().is_some_and(u8::is_ascii_alphabetic)))
        {
            return None;
        }
        Some(Self(b.into()))
    }

    /// The raw bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// The path as an `OsStr` for argv and joins (Unix: the bytes; Windows: lossy UTF-8).
    pub fn to_os(&self) -> Cow<'_, OsStr> {
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            Cow::Borrowed(OsStr::from_bytes(&self.0))
        }
        #[cfg(not(unix))]
        {
            match String::from_utf8_lossy(&self.0) {
                Cow::Borrowed(s) => Cow::Borrowed(OsStr::new(s)),
                Cow::Owned(s) => Cow::Owned(s.into()),
            }
        }
    }

    /// Lossy UTF-8 view (regex matching, messages).
    pub fn lossy(&self) -> Cow<'_, str> {
        String::from_utf8_lossy(&self.0)
    }

    /// No `..` component (native writes, the `root:` extension).
    pub fn is_contained(&self) -> bool {
        let is_sep = |b: &u8| *b == b'/' || (cfg!(windows) && *b == b'\\');
        !self.0.split(is_sep).any(|c| c == b"..")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validation() {
        assert!(RepoPath::new(b"").is_none());
        assert!(RepoPath::new(b"/etc/passwd").is_none());
        assert!(RepoPath::new(b"a\0b").is_none());
        let p = RepoPath::new(b"src/a\xff.rs").unwrap();
        assert_eq!(p.as_bytes(), b"src/a\xff.rs");
        assert_eq!(p.lossy(), "src/a\u{fffd}.rs");
        #[cfg(unix)]
        assert_eq!(p.to_os().as_encoded_bytes(), b"src/a\xff.rs");
        assert_eq!(RepoPath::new(b"C:x").is_some(), cfg!(not(windows)));
    }

    #[test]
    fn containment() {
        assert!(RepoPath::new(b"a/b").unwrap().is_contained());
        assert!(RepoPath::new(b"a..b/c").unwrap().is_contained());
        assert!(!RepoPath::new(b"a/../b").unwrap().is_contained());
        assert!(!RepoPath::new(b"..").unwrap().is_contained());
    }
}
