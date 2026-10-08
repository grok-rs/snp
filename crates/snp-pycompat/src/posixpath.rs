//! CPython 3.12 `posixpath` on raw bytes (`OsStr::as_bytes()` on Unix); compiled on every OS.
//! Purely lexical unless stated otherwise (15 §A4.9).

use std::borrow::Cow;

use crate::PathError;

const SEP: u8 = b'/';

fn components(p: &[u8]) -> impl Iterator<Item = &[u8]> {
    p.split(|&b| b == SEP)
}

/// `os.path.isabs(p)`: starts with `/`.
#[must_use]
pub fn isabs(p: &[u8]) -> bool {
    p.first() == Some(&SEP)
}

/// `os.path.normpath(p)`: `''` → `'.'`; exactly two leading slashes are kept, one or three and
/// more collapse to one; `''`/`.` components are dropped; `..` pops a previous non-`..` component,
/// is dropped at the root and kept when it leads a relative path; no trailing slash.
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
    for comp in components(p) {
        if comp.is_empty() || comp == b"." {
            continue;
        }
        if comp != b".." || (initial == 0 && comps.is_empty()) || comps.last() == Some(&&b".."[..])
        {
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

/// `os.path.join(a, *rest)`: an absolute later part replaces everything; otherwise a `/` is added
/// unless the path is empty or already ends in `/` (`join('a', '')` → `'a/'`).
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

/// `os.path.split(p)`: the tail follows the last `/`; the head loses its trailing slashes unless it
/// consists of slashes only.
#[must_use]
pub fn split(p: &[u8]) -> (&[u8], &[u8]) {
    let i = p
        .iter()
        .rposition(|&b| b == SEP)
        .map_or(0, |i| i.saturating_add(1));
    let (head, tail) = p.split_at_checked(i).unwrap_or((p, &[]));
    if !head.is_empty() && !head.iter().all(|&b| b == SEP) {
        let n = head.iter().rev().take_while(|&&b| b == SEP).count();
        (
            head.get(..head.len().saturating_sub(n)).unwrap_or_default(),
            tail,
        )
    } else {
        (head, tail)
    }
}

/// `os.path.dirname(p)`.
#[must_use]
pub fn dirname(p: &[u8]) -> &[u8] {
    split(p).0
}

/// `os.path.basename(p)` (`basename('a/b/') == ''`, unlike `Path::file_name`).
#[must_use]
pub fn basename(p: &[u8]) -> &[u8] {
    split(p).1
}

/// `genericpath._splitext(p, sep, altsep, '.')` over the separator set `seps`; shared with ntpath.
pub(crate) fn splitext_generic<'a>(p: &'a [u8], seps: &[u8]) -> (&'a [u8], &'a [u8]) {
    let sep_idx = p.iter().rposition(|b| seps.contains(b));
    let Some(dot) = p.iter().rposition(|&b| b == b'.') else {
        return (p, &[]);
    };
    if sep_idx.is_none_or(|s| dot > s) {
        let fname_start = sep_idx.map_or(0, |s| s.saturating_add(1));
        // leading dots of the file name do not start an extension
        if p.get(fname_start..dot)
            .is_some_and(|lead| lead.iter().any(|&b| b != b'.'))
        {
            return p.split_at_checked(dot).unwrap_or((p, &[]));
        }
    }
    (p, &[])
}

/// `os.path.splitext(p)` (`'...md'` → no extension; `'foo.'` → `('foo', '.')`).
#[must_use]
pub fn splitext(p: &[u8]) -> (&[u8], &[u8]) {
    splitext_generic(p, b"/")
}

/// `os.path.abspath(p)` with an explicit cwd (`os.getcwd()`; snp never chdirs): lexical, i.e.
/// `normpath(join(cwd, p))` — not `std::path::absolute`, which keeps `..` on Unix.
#[must_use]
pub fn abspath(p: &[u8], cwd: &[u8]) -> Vec<u8> {
    if isabs(p) {
        normpath(p)
    } else {
        normpath(&join(cwd, &[p]))
    }
}

/// `os.path.relpath(path, start)`; `start = None` means the cwd.
pub fn relpath(p: &[u8], start: Option<&[u8]>, cwd: &[u8]) -> Result<Vec<u8>, PathError> {
    if p.is_empty() {
        return Err(PathError::NoPathSpecified);
    }
    let start_abs = abspath(start.unwrap_or(b"."), cwd);
    let path_abs = abspath(p, cwd);
    let sl: Vec<&[u8]> = components(&start_abs).filter(|c| !c.is_empty()).collect();
    let pl: Vec<&[u8]> = components(&path_abs).filter(|c| !c.is_empty()).collect();
    let i = sl.iter().zip(&pl).take_while(|(a, b)| a == b).count();
    let mut rel: Vec<&[u8]> = vec![b".."; sl.len().saturating_sub(i)];
    rel.extend(pl.get(i..).unwrap_or_default());
    if rel.is_empty() {
        return Ok(b".".to_vec());
    }
    Ok(rel.join(&SEP))
}

/// `os.path.commonpath(paths)`: component-wise, ignores `''`/`.` components, does not resolve
/// `..` (`['/a/../b', '/a/b']` → `'/a'`).
pub fn commonpath(ps: &[&[u8]]) -> Result<Vec<u8>, PathError> {
    let Some(first) = ps.first() else {
        return Err(PathError::EmptySequence);
    };
    let abs = isabs(first);
    if ps.iter().any(|p| isabs(p) != abs) {
        return Err(PathError::MixAbsRel);
    }
    let split: Vec<Vec<&[u8]>> = ps
        .iter()
        .map(|p| {
            components(p)
                .filter(|c| !c.is_empty() && *c != b".")
                .collect()
        })
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
/// * `home_env`: `os.environ.get('HOME')` — `Some(b"")` is honoured (gives `/`).
/// * `pw_current`: the passwd home of the real uid, used only when `HOME` is unset.
/// * `pw_named`: the passwd home of `~user`.
pub fn expanduser<'a>(
    p: &'a [u8],
    home_env: Option<&[u8]>,
    pw_current: &dyn Fn() -> Option<Vec<u8>>,
    pw_named: &dyn Fn(&[u8]) -> Option<Vec<u8>>,
) -> Cow<'a, [u8]> {
    if p.first() != Some(&b'~') {
        return Cow::Borrowed(p);
    }
    let i = p
        .iter()
        .skip(1)
        .position(|&b| b == SEP)
        .map_or(p.len(), |x| x.saturating_add(1));
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
    let mut out = userhome
        .get(..userhome.len().saturating_sub(n))
        .unwrap_or_default()
        .to_vec();
    out.extend_from_slice(p.get(i..).unwrap_or_default());
    if out.is_empty() {
        out.push(SEP);
    }
    Cow::Owned(out)
}

/// Non-strict `os.path.realpath(p)` (3.12 `_joinrealpath`, 15 §A4.9): resolves symlinks component
/// by component relative to `cwd`, applies `..` after resolution, leaves missing components and
/// symlink loops unresolved; never fails for missing paths. An error of `readlink` after `lstat`
/// reported a link propagates.
#[cfg(unix)]
pub fn realpath(p: &[u8], cwd: &[u8]) -> std::io::Result<Vec<u8>> {
    use std::collections::HashMap;
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    type Seen = HashMap<Vec<u8>, Option<Vec<u8>>>;

    fn inner(
        mut path: Vec<u8>,
        rest: &[u8],
        cwd: &[u8],
        seen: &mut Seen,
    ) -> std::io::Result<(Vec<u8>, bool)> {
        let mut rest = rest;
        if isabs(rest) {
            rest = rest.get(1..).unwrap_or_default();
            path = vec![SEP];
        }
        while !rest.is_empty() {
            let (name, tail) = match rest.iter().position(|&b| b == SEP) {
                Some(i) => (
                    rest.get(..i).unwrap_or_default(),
                    rest.get(i.saturating_add(1)..).unwrap_or_default(),
                ),
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
                    path = if last == b".." {
                        join(&head, &[b"..", b".."])
                    } else {
                        head
                    };
                }
                continue;
            }
            let newpath = join(&path, &[name]);
            let abs_new = if isabs(&newpath) {
                newpath.clone()
            } else {
                join(cwd, &[&newpath])
            };
            let is_link = std::fs::symlink_metadata(OsStr::from_bytes(&abs_new))
                .is_ok_and(|m| m.file_type().is_symlink());
            if !is_link {
                path = newpath;
                continue;
            }
            if let Some(cached) = seen.get(&newpath) {
                if let Some(resolved) = cached {
                    path.clone_from(resolved);
                    continue;
                }
                // a symlink loop: return the resolved part + the rest unchanged
                return Ok((join(&newpath, &[rest]), false));
            }
            seen.insert(newpath.clone(), None);
            let target = std::fs::read_link(OsStr::from_bytes(&abs_new))?;
            let (resolved, ok) = inner(path, target.as_os_str().as_bytes(), cwd, seen)?;
            if !ok {
                return Ok((join(&resolved, &[rest]), false));
            }
            seen.insert(newpath, Some(resolved.clone()));
            path = resolved;
        }
        Ok((path, true))
    }

    let (path, _) = inner(Vec::new(), p, cwd, &mut HashMap::new())?;
    Ok(abspath(&path, cwd))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[u8]) -> &str {
        core::str::from_utf8(v).unwrap()
    }

    #[test]
    fn normpath_cases() {
        let cases = [
            ("", "."),
            ("/", "/"),
            ("//", "//"),
            ("///", "/"),
            ("//a//b/", "//a/b"),
            ("a/../..", ".."),
            ("/../a", "/a"),
            ("./a/./b/", "a/b"),
            ("a/b/../../..", ".."),
        ];
        for (p, want) in cases {
            assert_eq!(s(&normpath(p.as_bytes())), want, "{p:?}");
        }
    }

    #[test]
    fn join_split_ext() {
        assert_eq!(s(&join(b"a", &[b""])), "a/");
        assert_eq!(s(&join(b"a", &[b"/b", b"c"])), "/b/c");
        assert_eq!(split(b"a/b/"), (&b"a/b"[..], &b""[..]));
        assert_eq!(split(b"//x"), (&b"//"[..], &b"x"[..]));
        assert_eq!(splitext(b"...md"), (&b"...md"[..], &b""[..]));
        assert_eq!(splitext(b"foo."), (&b"foo"[..], &b"."[..]));
        assert_eq!(splitext(b"a.b/c"), (&b"a.b/c"[..], &b""[..]));
        assert_eq!(splitext(b"x/.bashrc"), (&b"x/.bashrc"[..], &b""[..]));
    }

    #[test]
    fn abs_rel_common() {
        let cwd = b"/r/sub";
        assert_eq!(s(&abspath(b"../x", cwd)), "/r/x");
        assert_eq!(
            relpath(b"/r/sub/a.py", None, b"/r").map(|v| s(&v).to_owned()),
            Ok("sub/a.py".into())
        );
        assert_eq!(
            relpath(b"/outside/c.py", None, b"/r").map(|v| s(&v).to_owned()),
            Ok("../outside/c.py".into())
        );
        assert_eq!(
            relpath(b"/r", None, b"/r").map(|v| s(&v).to_owned()),
            Ok(".".into())
        );
        assert_eq!(relpath(b"", None, b"/r"), Err(PathError::NoPathSpecified));
        assert_eq!(
            commonpath(&[b"/a/../b", b"/a/b"]).map(|v| s(&v).to_owned()),
            Ok("/a".into())
        );
        assert_eq!(commonpath(&[b"/a", b"b"]), Err(PathError::MixAbsRel));
        assert_eq!(commonpath(&[]), Err(PathError::EmptySequence));
    }

    #[test]
    fn expanduser_cases() {
        let none = || None;
        let named = |_: &[u8]| None;
        assert_eq!(
            &*expanduser(b"~/.cache", Some(b"/home/u/"), &none, &named),
            b"/home/u/.cache"
        );
        assert_eq!(&*expanduser(b"~", Some(b""), &none, &named), b"/");
        assert_eq!(&*expanduser(b"x/~", Some(b"/h"), &none, &named), b"x/~");
        assert_eq!(
            &*expanduser(b"~other/x", Some(b"/h"), &none, &named),
            b"~other/x"
        );
        let named = |u: &[u8]| (u == b"other").then(|| b"/home/other".to_vec());
        assert_eq!(
            &*expanduser(b"~other/x", Some(b"/h"), &none, &named),
            b"/home/other/x"
        );
        let current = || Some(b"/pw/home".to_vec());
        assert_eq!(&*expanduser(b"~", None, &current, &named), b"/pw/home");
    }
}
