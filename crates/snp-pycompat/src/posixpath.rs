//! CPython 3.12 `posixpath` (`Lib/posixpath.py`, `Lib/genericpath.py`; `normpath` is the C
//! `_path_normpath`) on raw bytes (`OsStr::as_bytes()` on Unix); compiled on every OS. Purely
//! lexical unless stated otherwise (15 §A4.9).

use std::borrow::Cow;

use crate::PathError;

const SEP: u8 = b'/';

fn components(p: &[u8]) -> impl Iterator<Item = &[u8]> {
    p.split(|&b| b == SEP)
}

/// `posixpath.isabs(p)`: starts with `/`.
#[must_use]
pub fn isabs(p: &[u8]) -> bool {
    p.first() == Some(&SEP)
}

/// `posixpath.normpath(p)` (`prefix.py:9-10` `Prefix.path`, `parse_shebang.py:25`): `''` → `'.'`;
/// exactly two leading slashes are kept, one or three and more collapse to one; `''`/`.` components
/// are dropped; `..` pops a previous non-`..` component, is dropped at the root and kept when it
/// leads a relative path; no trailing slash.
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

/// `posixpath.join(a, *rest)`: an absolute later part replaces everything; otherwise a `/` is added
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

/// `posixpath.split(p)`: the tail follows the last `/`; the head loses its trailing slashes unless
/// it consists of slashes only.
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

/// `posixpath.dirname(p)` (`split(p)[0]`).
#[must_use]
pub fn dirname(p: &[u8]) -> &[u8] {
    split(p).0
}

/// `posixpath.basename(p)` (`split(p)[1]`; `basename('a/b/') == ''`, unlike `Path::file_name`).
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

/// `posixpath.splitext(p)` (`genericpath._splitext`): `'...md'` has no extension, and
/// `'foo.'` → `('foo', '.')`.
#[must_use]
pub fn splitext(p: &[u8]) -> (&[u8], &[u8]) {
    splitext_generic(p, b"/")
}

/// `posixpath.abspath(p)` (`main.py:178-186`) with an explicit cwd (`os.getcwd()`; snp never
/// chdirs): lexical, i.e. `normpath(join(cwd, p))` — not `std::path::absolute`, which keeps `..` on
/// Unix.
#[must_use]
pub fn abspath(p: &[u8], cwd: &[u8]) -> Vec<u8> {
    if isabs(p) {
        normpath(p)
    } else {
        normpath(&join(cwd, &[p]))
    }
}

/// `posixpath.relpath(path, start)` (`main.py:191-199`); `start = None` means the cwd; `''` is
/// `ValueError: no path specified`. Compares the `abspath` component lists (empty components
/// dropped) and joins `..` for every unmatched start component with the rest of the path.
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

/// `posixpath.commonpath(paths)` (`lang_base.py:70` `exe_exists`): component-wise, ignores `''`/`.`
/// components, does not resolve `..` (`['/a/../b', '/a/b']` → `'/a'`).
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

/// `posixpath.expanduser(p)` (`store.py:33`, `lang_base.py:68`): only a leading `~` or `~user` up
/// to the first `/`; the home loses trailing slashes, and an empty result is `/`.
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

/// The filesystem queries of `_joinrealpath`, on paths already joined to the cwd.
#[cfg(any(unix, test))]
trait LinkFs {
    /// `stat.S_ISLNK(os.lstat(p).st_mode)`; any `OSError` counts as "not a link".
    fn is_link(&self, p: &[u8]) -> bool;
    /// `os.readlink(p)`.
    fn readlink(&self, p: &[u8]) -> std::io::Result<Vec<u8>>;
}

/// One suspended `_joinrealpath` call: the rest of its path and the link it is resolving.
#[cfg(any(unix, test))]
#[derive(Debug)]
struct Frame {
    rest: Vec<u8>,
    pos: usize,
    link: Vec<u8>,
}

/// CPython 3.12 `posixpath._joinrealpath(path='', rest=p, strict=False, seen={})` without
/// recursion: each symlink resolution pushes a [`Frame`] instead of a Python stack frame, so an
/// arbitrarily long link chain needs no thread stack (Python would raise `RecursionError` near
/// 1000 levels). Returns the path before the final `abspath`.
#[cfg(any(unix, test))]
fn join_realpath(p: &[u8], cwd: &[u8], fs: &dyn LinkFs) -> std::io::Result<Vec<u8>> {
    use std::collections::HashMap;

    let on_disk = |q: &[u8]| {
        if isabs(q) {
            q.to_vec()
        } else {
            join(cwd, &[q])
        }
    };
    let mut seen: HashMap<Vec<u8>, Option<Vec<u8>>> = HashMap::new();
    let mut stack: Vec<Frame> = Vec::new();
    let mut path: Vec<u8> = Vec::new();
    let mut rest: Vec<u8> = p.to_vec();
    let mut pos = 0_usize;
    let mut fresh_call = true;
    loop {
        if fresh_call {
            fresh_call = false;
            if isabs(&rest) {
                pos = 1;
                path = vec![SEP];
            }
        }
        let mut descended = false;
        while let Some(tail) = rest.get(pos..).filter(|t| !t.is_empty()) {
            // name, _, rest = rest.partition(sep)
            let (name, consumed) = match tail.iter().position(|&b| b == SEP) {
                Some(i) => (tail.get(..i).unwrap_or_default(), i.saturating_add(1)),
                None => (tail, tail.len()),
            };
            let name = name.to_vec();
            pos = pos.saturating_add(consumed);
            if name.is_empty() || name == b"." {
                continue;
            }
            if name == b".." {
                if path.is_empty() {
                    path = b"..".to_vec();
                } else {
                    let (head, last) = split(&path);
                    path = if last == b".." {
                        join(head, &[b"..", b".."])
                    } else {
                        head.to_vec()
                    };
                }
                continue;
            }
            let newpath = join(&path, &[&name]);
            if newpath.contains(&0) {
                // `os.lstat(newpath)` raises `ValueError: embedded null byte` (not an OSError,
                // so non-strict realpath does not swallow it); components after a symlink loop
                // are never lstat'ed and keep their NUL.
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "embedded null byte",
                ));
            }
            if !fs.is_link(&on_disk(&newpath)) {
                path = newpath;
                continue;
            }
            match seen.get(&newpath) {
                Some(Some(resolved)) => {
                    path.clone_from(resolved);
                    continue;
                }
                Some(None) => {
                    // a symlink loop: the resolved part + the rest unchanged, through every caller
                    let mut out = join(&newpath, &[rest.get(pos..).unwrap_or_default()]);
                    while let Some(frame) = stack.pop() {
                        out = join(&out, &[frame.rest.get(frame.pos..).unwrap_or_default()]);
                    }
                    return Ok(out);
                }
                None => {}
            }
            seen.insert(newpath.clone(), None);
            let target = fs.readlink(&on_disk(&newpath))?;
            stack.push(Frame {
                rest: std::mem::replace(&mut rest, target),
                pos,
                link: newpath,
            });
            pos = 0;
            fresh_call = true;
            descended = true;
            break;
        }
        if descended {
            continue;
        }
        // this call resolved its whole rest
        let Some(frame) = stack.pop() else {
            return Ok(path);
        };
        seen.insert(frame.link, Some(path.clone()));
        rest = frame.rest;
        pos = frame.pos;
    }
}

/// The real filesystem through `lstat`/`readlink` of raw byte paths.
#[cfg(unix)]
#[derive(Debug)]
struct RealFs;

#[cfg(unix)]
impl LinkFs for RealFs {
    fn is_link(&self, p: &[u8]) -> bool {
        use std::os::unix::ffi::OsStrExt;
        std::fs::symlink_metadata(std::ffi::OsStr::from_bytes(p))
            .is_ok_and(|m| m.file_type().is_symlink())
    }

    fn readlink(&self, p: &[u8]) -> std::io::Result<Vec<u8>> {
        use std::os::unix::ffi::{OsStrExt, OsStringExt};
        std::fs::read_link(std::ffi::OsStr::from_bytes(p)).map(|t| t.into_os_string().into_vec())
    }
}

/// Non-strict `os.path.realpath(p)` (3.12 `posixpath.realpath` / `_joinrealpath`, 15 §A4.9) with
/// an explicit `cwd`: resolves symlinks component by component, applies `..` after resolution
/// (`missing/../link_a` resolves `link_a`), follows dangling links to their target, leaves symlink
/// loops unresolved and never fails for missing paths; the result goes through [`abspath`]. An
/// error of `readlink` after `lstat` reported a link propagates, and a component holding a NUL
/// byte that would be `lstat`ed is `InvalidInput` `embedded null byte` (CPython 3.12.3's
/// `ValueError` from `os.lstat`). Used for the store directory
/// (`store.py:36`) and `init-templatedir` (`init_templatedir.py:34`).
#[cfg(unix)]
pub fn realpath(p: &[u8], cwd: &[u8]) -> std::io::Result<Vec<u8>> {
    let path = join_realpath(p, cwd, &RealFs)?;
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

    /// A filesystem of symlinks only (absolute link path → target); everything else is a plain
    /// entry.
    struct FakeFs(std::collections::HashMap<Vec<u8>, Vec<u8>>);

    impl LinkFs for FakeFs {
        fn is_link(&self, p: &[u8]) -> bool {
            self.0.contains_key(p)
        }
        fn readlink(&self, p: &[u8]) -> std::io::Result<Vec<u8>> {
            self.0
                .get(p)
                .cloned()
                .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::NotFound))
        }
    }

    fn fake(links: &[(&str, &str)]) -> FakeFs {
        FakeFs(
            links
                .iter()
                .map(|(l, t)| (l.as_bytes().to_vec(), t.as_bytes().to_vec()))
                .collect(),
        )
    }

    fn rp(fs: &FakeFs, p: &str) -> String {
        let path = join_realpath(p.as_bytes(), b"/r", fs).unwrap();
        String::from_utf8(abspath(&path, b"/r")).unwrap()
    }

    #[test]
    fn realpath_algorithm_on_a_fake_fs() {
        let fs = fake(&[
            ("/r/link_a", "a"),
            ("/r/link_abs", "/r/a/b"),
            ("/r/loop1", "loop2"),
            ("/r/loop2", "loop1"),
            ("/r/self", "self"),
            ("/r/dangling", "nonexistent"),
            ("/r/chain", "link_a"),
            ("/r/a/b/up", "../.."),
        ]);
        assert_eq!(rp(&fs, "/r/link_a/b"), "/r/a/b");
        assert_eq!(rp(&fs, "link_a/.."), "/r");
        assert_eq!(rp(&fs, "/r/link_abs/.."), "/r/a");
        assert_eq!(rp(&fs, "/r/loop1/x"), "/r/loop1/x");
        assert_eq!(rp(&fs, "/r/self"), "/r/self");
        assert_eq!(rp(&fs, "/r/dangling/x"), "/r/nonexistent/x");
        assert_eq!(rp(&fs, "/r/chain/b"), "/r/a/b");
        assert_eq!(rp(&fs, "/r/a/b/up/a"), "/r/a");
        assert_eq!(rp(&fs, "/r/missing/../link_a"), "/r/a");
        assert_eq!(rp(&fs, ""), "/r");
        // the loop is reported through a resolved outer link with its own rest
        let fs = fake(&[("/r/outer", "loop1/y"), ("/r/loop1", "loop1")]);
        assert_eq!(rp(&fs, "/r/outer/z"), "/r/loop1/y/z");
    }

    #[test]
    fn realpath_long_link_chains_need_no_stack() {
        // l0 -> l1 -> … -> l{n} -> target: 100 000 nested resolutions (Python: RecursionError)
        let n = 100_000_usize;
        let mut links = std::collections::HashMap::new();
        for i in 0..n {
            links.insert(
                format!("/r/l{i}").into_bytes(),
                format!("l{}", i + 1).into_bytes(),
            );
        }
        links.insert(format!("/r/l{n}").into_bytes(), b"/target".to_vec());
        let fs = FakeFs(links);
        let out = std::thread::Builder::new()
            .stack_size(256 * 1024)
            .spawn(move || rp(&fs, "/r/l0/x"))
            .unwrap()
            .join()
            .unwrap();
        assert_eq!(out, "/target/x");
    }

    #[test]
    fn realpath_embedded_nul_is_lstats_value_error() {
        let fs = fake(&[("/r/loop1", "loop1"), ("/r/lnk", "a")]);
        let err = |p: &[u8]| {
            join_realpath(p, b"/r", &fs)
                .map_err(|e| (e.kind(), e.to_string()))
                .unwrap_err()
        };
        let want = (
            std::io::ErrorKind::InvalidInput,
            "embedded null byte".to_owned(),
        );
        assert_eq!(err(b"a\0b"), want);
        assert_eq!(err(b"lnk\0/x"), want);
        assert_eq!(err(b"/\0"), want);
        assert_eq!(err(b"lnk/x\0"), want);
        // `.`/`..` are never lstat'ed and neither is the rest after a symlink loop
        assert_eq!(rp(&fs, "/r/loop1/a\0b").as_bytes(), b"/r/loop1/a\0b");
    }

    #[cfg(unix)]
    #[test]
    fn real_realpath_rejects_embedded_nul() {
        let e = realpath(b"a\0b", b"/").unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::InvalidInput);
        assert_eq!(e.to_string(), "embedded null byte");
    }

    #[test]
    fn realpath_readlink_errors_propagate() {
        struct Broken;
        impl LinkFs for Broken {
            fn is_link(&self, _: &[u8]) -> bool {
                true
            }
            fn readlink(&self, _: &[u8]) -> std::io::Result<Vec<u8>> {
                Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied))
            }
        }
        assert_eq!(
            join_realpath(b"/x", b"/", &Broken).map_err(|e| e.kind()),
            Err(std::io::ErrorKind::PermissionDenied)
        );
    }
}
