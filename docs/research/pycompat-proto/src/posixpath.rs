//! `posixpath` (CPython 3.12) on raw bytes (`OsStr::as_bytes()` on Unix).
//! Purely lexical unless stated otherwise.
use std::borrow::Cow;

/// Python `ValueError` texts raised by `os.path` functions (both flavors).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PathError {
    /// `relpath('')`
    NoPathSpecified,
    /// `commonpath([])` (3.12 text; 3.13 ntpath says "iterable")
    EmptySequence,
    /// `commonpath` with absolute and relative inputs
    MixAbsRel,
    /// ntpath `commonpath` with different drives
    DifferentDrives,
    /// ntpath `relpath` across drives: `path is on mount %r, start on mount %r`
    DifferentMounts { path_drive: String, start_drive: String },
}

impl core::fmt::Display for PathError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NoPathSpecified => f.write_str("no path specified"),
            Self::EmptySequence => f.write_str("commonpath() arg is an empty sequence"),
            Self::MixAbsRel => f.write_str("Can't mix absolute and relative paths"),
            Self::DifferentDrives => f.write_str("Paths don't have the same drive"),
            Self::DifferentMounts { path_drive, start_drive } => write!(
                f,
                "path is on mount {}, start on mount {}",
                crate::repr::repr_str(path_drive),
                crate::repr::repr_str(start_drive)
            ),
        }
    }
}
impl std::error::Error for PathError {}

const SEP: u8 = b'/';

/// `os.path.isabs(p)`.
#[must_use]
pub fn isabs(p: &[u8]) -> bool {
    p.first() == Some(&SEP)
}

/// `os.path.normpath(p)`: `''`→`'.'`; exactly two leading slashes are kept,
/// three or more collapse to one; `..` at the root is dropped; relative
/// leading `..` are kept; no trailing slash.
#[must_use]
pub fn normpath(p: &[u8]) -> Vec<u8> {
    if p.is_empty() {
        return b".".to_vec();
    }
    let initial = if p.starts_with(b"//") && !p.starts_with(b"///") {
        2
    } else {
        usize::from(p.starts_with(b"/"))
    };
    let mut comps: Vec<&[u8]> = Vec::new();
    for comp in p.split(|&b| b == SEP) {
        if comp.is_empty() || comp == b"." {
            continue;
        }
        if comp != b".." || (initial == 0 && comps.is_empty()) || comps.last() == Some(&&b".."[..]) {
            comps.push(comp);
        } else if !comps.is_empty() {
            comps.pop();
        }
    }
    let mut out = vec![SEP; initial];
    for (i, c) in comps.iter().enumerate() {
        if i > 0 {
            out.push(SEP);
        }
        out.extend_from_slice(c);
    }
    if out.is_empty() {
        out.push(b'.');
    }
    out
}

/// `os.path.join(a, *rest)`.
#[must_use]
pub fn join(a: &[u8], rest: &[&[u8]]) -> Vec<u8> {
    let mut path = a.to_vec();
    for b in rest {
        if b.first() == Some(&SEP) {
            path = b.to_vec();
        } else if path.is_empty() || path.last() == Some(&SEP) {
            path.extend_from_slice(b);
        } else {
            path.push(SEP);
            path.extend_from_slice(b);
        }
    }
    path
}

/// `os.path.split(p)`: head has trailing slashes removed unless it is all slashes.
#[must_use]
pub fn split(p: &[u8]) -> (&[u8], &[u8]) {
    let i = p.iter().rposition(|&b| b == SEP).map_or(0, |i| i + 1);
    let (head, tail) = p.split_at(i);
    if !head.is_empty() && !head.iter().all(|&b| b == SEP) {
        let n = head.iter().rev().take_while(|&&b| b == SEP).count();
        (head.get(..head.len() - n).unwrap_or_default(), tail)
    } else {
        (head, tail)
    }
}

/// `os.path.dirname(p)`.
#[must_use]
pub fn dirname(p: &[u8]) -> &[u8] {
    split(p).0
}

/// `os.path.basename(p)`.
#[must_use]
pub fn basename(p: &[u8]) -> &[u8] {
    split(p).1
}

/// `genericpath._splitext(p, sep, altsep, '.')`, shared by both flavors.
#[must_use]
pub fn splitext_generic<'a>(p: &'a [u8], seps: &[u8]) -> (&'a [u8], &'a [u8]) {
    let sep_idx = p.iter().rposition(|b| seps.contains(b));
    let Some(dot) = p.iter().rposition(|&b| b == b'.') else {
        return (p, b"");
    };
    if sep_idx.is_none_or(|s| dot > s) {
        let fname_start = sep_idx.map_or(0, |s| s + 1);
        // skip all leading dots of the file name
        if p.get(fname_start..dot).is_some_and(|lead| lead.iter().any(|&b| b != b'.')) {
            return p.split_at(dot);
        }
    }
    (p, b"")
}

/// `os.path.splitext(p)`.
#[must_use]
pub fn splitext(p: &[u8]) -> (&[u8], &[u8]) {
    splitext_generic(p, b"/")
}

/// `os.path.abspath(p)` with an explicit cwd (`os.getcwd()`): lexical only.
///
/// NOT `std::path::absolute`, which keeps `..` on Unix.
#[must_use]
pub fn abspath(p: &[u8], cwd: &[u8]) -> Vec<u8> {
    if isabs(p) { normpath(p) } else { normpath(&join(cwd, &[p])) }
}

/// `os.path.relpath(path, start)`; `start=None` means the cwd.
pub fn relpath(path: &[u8], start: Option<&[u8]>, cwd: &[u8]) -> Result<Vec<u8>, PathError> {
    if path.is_empty() {
        return Err(PathError::NoPathSpecified);
    }
    let start_abs = abspath(start.unwrap_or(b"."), cwd);
    let path_abs = abspath(path, cwd);
    let sl: Vec<&[u8]> = start_abs.split(|&b| b == SEP).filter(|c| !c.is_empty()).collect();
    let pl: Vec<&[u8]> = path_abs.split(|&b| b == SEP).filter(|c| !c.is_empty()).collect();
    let i = sl.iter().zip(&pl).take_while(|(a, b)| a == b).count();
    let mut rel: Vec<&[u8]> = vec![b".."; sl.len() - i];
    rel.extend(pl.get(i..).unwrap_or_default());
    if rel.is_empty() {
        return Ok(b".".to_vec());
    }
    Ok(rel.join(&SEP))
}

/// `os.path.commonpath(paths)`: component-wise, ignores `''`/`'.'`
/// components, does NOT resolve `..`.
pub fn commonpath(paths: &[&[u8]]) -> Result<Vec<u8>, PathError> {
    let Some(first) = paths.first() else {
        return Err(PathError::EmptySequence);
    };
    let abs = isabs(first);
    if paths.iter().any(|p| isabs(p) != abs) {
        return Err(PathError::MixAbsRel);
    }
    let split: Vec<Vec<&[u8]>> = paths
        .iter()
        .map(|p| p.split(|&b| b == SEP).filter(|c| !c.is_empty() && *c != b".").collect())
        .collect();
    let (Some(s1), Some(s2)) = (split.iter().min(), split.iter().max()) else {
        return Err(PathError::EmptySequence);
    };
    let n = s1.iter().zip(s2).take_while(|(a, b)| a == b).count();
    let mut out = if abs { vec![SEP] } else { Vec::new() };
    out.extend_from_slice(&s1.get(..n).unwrap_or_default().join(&SEP));
    Ok(out)
}

/// `os.path.expanduser(p)` (POSIX).
///
/// * `home_env`: `os.environ.get('HOME')` — `Some(b"")` is honored (gives `/`).
/// * `pw_current`: home of the real uid from the passwd db (used only when
///   `HOME` is unset); `pw_named`: home of a named user (`~user`).
pub fn expanduser<'a>(
    p: &'a [u8],
    home_env: Option<&[u8]>,
    pw_current: impl FnOnce() -> Option<Vec<u8>>,
    pw_named: impl FnOnce(&[u8]) -> Option<Vec<u8>>,
) -> Cow<'a, [u8]> {
    if p.first() != Some(&b'~') {
        return Cow::Borrowed(p);
    }
    let i = p.iter().skip(1).position(|&b| b == SEP).map_or(p.len(), |x| x + 1);
    let userhome = if i == 1 {
        match home_env {
            Some(h) => h.to_vec(),
            None => match pw_current() {
                Some(h) => h,
                None => return Cow::Borrowed(p),
            },
        }
    } else {
        match pw_named(p.get(1..i).unwrap_or_default()) {
            Some(h) => h,
            None => return Cow::Borrowed(p),
        }
    };
    let n = userhome.iter().rev().take_while(|&&b| b == SEP).count();
    let mut out = userhome.get(..userhome.len() - n).unwrap_or_default().to_vec();
    out.extend_from_slice(p.get(i..).unwrap_or_default());
    if out.is_empty() {
        out.push(SEP);
    }
    Cow::Owned(out)
}

/// Non-strict `os.path.realpath(p)` (3.12 `_joinrealpath`): resolves symlinks
/// component by component, applies `..` after resolution, leaves missing
/// components and symlink loops unresolved. Never fails on missing paths.
#[cfg(unix)]
pub fn realpath(p: &[u8], cwd: &[u8]) -> std::io::Result<Vec<u8>> {
    use std::collections::HashMap;
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    // `join(newpath, rest)`; a trailing '/' from an empty `rest` is removed by
    // the final `abspath`.
    fn join_rest(path: &[u8], rest: &[u8]) -> Vec<u8> {
        join(path, &[rest])
    }

    fn inner(
        mut path: Vec<u8>,
        rest: &[u8],
        cwd: &[u8],
        seen: &mut HashMap<Vec<u8>, Option<Vec<u8>>>,
    ) -> std::io::Result<(Vec<u8>, bool)> {
        let mut rest = rest;
        if isabs(rest) {
            rest = rest.get(1..).unwrap_or_default();
            path = b"/".to_vec();
        }
        while !rest.is_empty() {
            let (name, tail) = match rest.iter().position(|&b| b == SEP) {
                Some(i) => (rest.get(..i).unwrap_or_default(), rest.get(i + 1..).unwrap_or_default()),
                None => (rest, &b""[..]),
            };
            rest = tail;
            if name.is_empty() || name == b"." {
                continue;
            }
            if name == b".." {
                if path.is_empty() {
                    path = b"..".to_vec();
                } else {
                    let (head, last) = split(&path);
                    let (head, last) = (head.to_vec(), last.to_vec());
                    path = if last == b".." { join(&head, &[b"..", b".."]) } else { head };
                }
                continue;
            }
            let newpath = join(&path, &[name]);
            let abs_new = if isabs(&newpath) { newpath.clone() } else { join(cwd, &[&newpath]) };
            let is_link = std::fs::symlink_metadata(OsStr::from_bytes(&abs_new))
                .map(|m| m.file_type().is_symlink())
                .unwrap_or(false);
            if !is_link {
                path = newpath;
                continue;
            }
            if let Some(cached) = seen.get(&newpath) {
                if let Some(resolved) = cached {
                    path = resolved.clone();
                    continue;
                }
                return Ok((join_rest(&newpath, rest), false));
            }
            seen.insert(newpath.clone(), None);
            let target = std::fs::read_link(OsStr::from_bytes(&abs_new))?;
            let (resolved, ok) = inner(path, target.as_os_str().as_bytes(), cwd, seen)?;
            if !ok {
                return Ok((join_rest(&resolved, rest), false));
            }
            seen.insert(newpath, Some(resolved.clone()));
            path = resolved;
        }
        Ok((path, true))
    }

    let (path, _) = inner(Vec::new(), p, cwd, &mut HashMap::new())?;
    Ok(abspath(&path, cwd))
}
