//! CPython 3.12 `ntpath`, generic over path code units (`u8` for tests and UTF-8 data, `u16` for
//! `OsStr::encode_wide` on Windows); every lexical function runs on every OS (15 §A4.10).
//!
//! Separators, `:` and `.` are ASCII, so the algorithms work unit by unit; lone surrogates pass
//! through untouched. Case folding (`normcase`, drive comparison, `commonpath`) is ASCII-only — the
//! accepted drift for non-ASCII letters (15 Q5).

use std::any::{Any, TypeId};
use std::borrow::Cow;
use std::hash::Hash;

use crate::PathError;

/// A path code unit: `u8` or `u16`.
pub trait Unit: Copy + Eq + Ord + Hash + std::fmt::Debug + Send + Sync + 'static {
    /// The unit encoding the ASCII byte `b`.
    fn from_ascii(b: u8) -> Self;
    /// `Some(byte)` for ASCII units, `None` otherwise.
    fn to_ascii(self) -> Option<u8>;
}

impl Unit for u8 {
    fn from_ascii(b: u8) -> Self {
        b
    }
    fn to_ascii(self) -> Option<u8> {
        self.is_ascii().then_some(self)
    }
}

impl Unit for u16 {
    fn from_ascii(b: u8) -> Self {
        Self::from(b)
    }
    fn to_ascii(self) -> Option<u8> {
        u8::try_from(self).ok().filter(u8::is_ascii)
    }
}

/// The raw value of a unit (`u8`/`u16` are the only implementations used by snp).
fn raw<U: Unit>(u: U) -> u32 {
    let any: &dyn Any = &u;
    if let Some(b) = any.downcast_ref::<u8>() {
        u32::from(*b)
    } else if let Some(w) = any.downcast_ref::<u16>() {
        u32::from(*w)
    } else {
        u.to_ascii().map_or(0xfffd, u32::from)
    }
}

/// Number of units of the first Python character of `p` (UTF-8 sequence or UTF-16 pair).
fn first_char_len<U: Unit>(p: &[U]) -> usize {
    let Some(&first) = p.first() else { return 0 };
    if first.to_ascii().is_some() {
        return 1;
    }
    if is_u8::<U>() {
        let len: usize = match raw(first) {
            0xc2..=0xdf => 2,
            0xe0..=0xef => 3,
            0xf0..=0xf4 => 4,
            _ => 1,
        };
        let cont = p
            .iter()
            .skip(1)
            .take(len.saturating_sub(1))
            .take_while(|u| matches!(raw(**u), 0x80..=0xbf))
            .count();
        if cont.saturating_add(1) == len {
            len
        } else {
            1
        }
    } else {
        let hi = matches!(raw(first), 0xd800..=0xdbff);
        let lo = p.get(1).is_some_and(|u| matches!(raw(*u), 0xdc00..=0xdfff));
        if hi && lo { 2 } else { 1 }
    }
}

fn is_sep<U: Unit>(u: U) -> bool {
    matches!(u.to_ascii(), Some(b'\\' | b'/'))
}

fn is<U: Unit>(u: U, b: u8) -> bool {
    u.to_ascii() == Some(b)
}

fn lit<U: Unit>(s: &[u8]) -> Vec<U> {
    s.iter().map(|&b| U::from_ascii(b)).collect()
}

fn ascii_lower<U: Unit>(u: U) -> U {
    match u.to_ascii() {
        Some(b) if b.is_ascii_uppercase() => U::from_ascii(b.to_ascii_lowercase()),
        _ => u,
    }
}

fn eq_ignore_ascii_case<U: Unit>(a: &[U], b: &[U]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b)
            .all(|(x, y)| ascii_lower(*x) == ascii_lower(*y))
}

fn backslash<U: Unit>() -> U {
    U::from_ascii(b'\\')
}

/// `p.replace('/', '\\')`.
fn replace_altsep<U: Unit>(p: &[U]) -> Vec<U> {
    p.iter()
        .map(|&u| if is(u, b'/') { backslash() } else { u })
        .collect()
}

fn position_from<U: Unit>(p: &[U], from: usize, pred: impl Fn(U) -> bool) -> Option<usize> {
    p.get(from..)?
        .iter()
        .position(|&u| pred(u))
        .map(|i| i.saturating_add(from))
}

fn is_u8<U: Unit>() -> bool {
    TypeId::of::<U>() == TypeId::of::<u8>()
}

fn to_string_lossy<U: Unit>(p: &[U]) -> String {
    if is_u8::<U>() {
        let bytes: Vec<u8> = p
            .iter()
            .filter_map(|&u| u8::try_from(raw(u)).ok())
            .collect();
        String::from_utf8_lossy(&bytes).into_owned()
    } else {
        let units: Vec<u16> = p
            .iter()
            .filter_map(|&u| u16::try_from(raw(u)).ok())
            .collect();
        String::from_utf16_lossy(&units)
    }
}

/// `ntpath.splitroot(p)` (3.12): `(drive, root, tail)` with `/` treated as `\`.
#[must_use]
pub fn splitroot<U: Unit>(p: &[U]) -> (&[U], &[U], &[U]) {
    let empty: &[U] = &[];
    let sep_at = |i: usize| p.get(i).is_some_and(|&u| is_sep(u));
    if sep_at(0) {
        if sep_at(1) {
            // UNC drives (\\server\share, \\?\UNC\server\share) and device drives (\\.\dev, \\?\X:)
            let unc: Vec<U> = lit(b"\\\\?\\UNC\\");
            let start = if p.get(..8).is_some_and(|h| {
                h.iter().zip(&unc).all(|(&x, &y)| {
                    let x = if is(x, b'/') { backslash() } else { x };
                    let upper = x
                        .to_ascii()
                        .map_or(x, |b| U::from_ascii(b.to_ascii_uppercase()));
                    upper == y
                })
            }) {
                8
            } else {
                2
            };
            let Some(index) = position_from(p, start, is_sep) else {
                return (p, empty, empty);
            };
            let Some(index2) = position_from(p, index.saturating_add(1), is_sep) else {
                return (p, empty, empty);
            };
            let (drive, rest) = p.split_at_checked(index2).unwrap_or((p, empty));
            let (root, tail) = rest.split_at_checked(1).unwrap_or((rest, empty));
            return (drive, root, tail);
        }
        let (root, tail) = p.split_at_checked(1).unwrap_or((p, empty));
        return (empty, root, tail);
    }
    let c1 = first_char_len(p);
    if p.get(c1).is_some_and(|&u| is(u, b':')) {
        let d_end = c1.saturating_add(1);
        let (drive, rest) = p.split_at_checked(d_end).unwrap_or((p, empty));
        if rest.first().is_some_and(|&u| is_sep(u)) {
            let (root, tail) = rest.split_at_checked(1).unwrap_or((rest, empty));
            return (drive, root, tail);
        }
        return (drive, empty, rest);
    }
    (empty, empty, p)
}

/// `ntpath.splitdrive(p)` = `(drive, root + tail)`.
#[must_use]
pub fn splitdrive<U: Unit>(p: &[U]) -> (&[U], &[U]) {
    let (d, _, _) = splitroot(p);
    p.split_at_checked(d.len()).unwrap_or((p, &[]))
}

/// The first three characters are `X:\`-like (drive letter, colon, separator).
fn has_colon_sep<U: Unit>(p: &[U]) -> bool {
    let c1 = first_char_len(p);
    p.get(c1).is_some_and(|&u| is(u, b':'))
        && p.get(c1.saturating_add(1)).is_some_and(|&u| is_sep(u))
}

/// `ntpath.isabs(p)` with CPython 3.12 semantics: `\x` and `/x` are absolute (legacy behaviour).
#[must_use]
pub fn isabs_312<U: Unit>(p: &[U]) -> bool {
    p.first().is_some_and(|&u| is_sep(u)) || has_colon_sep(p)
}

/// `ntpath.isabs(p)` with CPython 3.13+ semantics: only drive + root or UNC/device paths.
#[must_use]
pub fn isabs_313<U: Unit>(p: &[U]) -> bool {
    (p.first().is_some_and(|&u| is_sep(u)) && p.get(1).is_some_and(|&u| is_sep(u)))
        || has_colon_sep(p)
}

/// `ntpath.normpath(p)` (3.12).
#[must_use]
pub fn normpath<U: Unit>(p: &[U]) -> Vec<U> {
    let path = replace_altsep(p);
    let (drive, root, rest) = splitroot(&path);
    let dot = [U::from_ascii(b'.')];
    let dotdot = [U::from_ascii(b'.'), U::from_ascii(b'.')];
    let mut comps: Vec<&[U]> = Vec::new();
    for c in rest.split(|&u| is(u, b'\\')) {
        if c.is_empty() || c == dot.as_slice() {
            continue;
        }
        if c == dotdot.as_slice() {
            match comps.last() {
                Some(&last) if last != dotdot.as_slice() => {
                    comps.pop();
                }
                None if !root.is_empty() => {}
                _ => comps.push(c),
            }
        } else {
            comps.push(c);
        }
    }
    let mut out: Vec<U> = Vec::new();
    out.extend_from_slice(drive);
    out.extend_from_slice(root);
    if drive.is_empty() && root.is_empty() && comps.is_empty() {
        out.push(U::from_ascii(b'.'));
    }
    for (i, c) in comps.iter().enumerate() {
        if i > 0 {
            out.push(backslash());
        }
        out.extend_from_slice(c);
    }
    out
}

/// `ntpath.join(path, *paths)` (3.12).
#[must_use]
pub fn join<U: Unit>(a: &[U], rest: &[&[U]]) -> Vec<U> {
    let (d, r, t) = splitroot(a);
    let (mut rd, mut rr, mut rp) = (d.to_vec(), r.to_vec(), t.to_vec());
    for p in rest {
        let (pd, pr, pp) = splitroot(p);
        if !pr.is_empty() {
            // the second path is absolute
            if !pd.is_empty() || rd.is_empty() {
                rd = pd.to_vec();
            }
            rr = pr.to_vec();
            rp = pp.to_vec();
            continue;
        } else if !pd.is_empty() && pd != rd.as_slice() {
            if !eq_ignore_ascii_case(pd, &rd) {
                // different drives: ignore the first path entirely
                rd = pd.to_vec();
                rr = pr.to_vec();
                rp = pp.to_vec();
                continue;
            }
            // same drive in a different case
            rd = pd.to_vec();
        }
        // the second path is relative to the first
        if rp.last().is_some_and(|&u| !is_sep(u)) {
            rp.push(backslash());
        }
        rp.extend_from_slice(pp);
    }
    // add a separator between a UNC drive and a non-absolute path
    if !rp.is_empty()
        && rr.is_empty()
        && rd
            .last()
            .is_some_and(|&u| !matches!(u.to_ascii(), Some(b':' | b'\\' | b'/')))
    {
        let mut out = rd;
        out.push(backslash());
        out.extend_from_slice(&rp);
        return out;
    }
    let mut out = rd;
    out.extend_from_slice(&rr);
    out.extend_from_slice(&rp);
    out
}

/// `ntpath.split(p)`: the head is drive + root + the dirname without trailing separators.
#[must_use]
pub fn split<U: Unit>(p: &[U]) -> (Vec<U>, &[U]) {
    let (drive, root, rest) = splitroot(p);
    let cut = rest
        .iter()
        .rposition(|&u| is_sep(u))
        .map_or(0, |i| i.saturating_add(1));
    let (head, tail) = rest.split_at_checked(cut).unwrap_or((rest, &[]));
    let trailing = head.iter().rev().take_while(|&&u| is_sep(u)).count();
    let head = head
        .get(..head.len().saturating_sub(trailing))
        .unwrap_or_default();
    let mut out = drive.to_vec();
    out.extend_from_slice(root);
    out.extend_from_slice(head);
    (out, tail)
}

/// `ntpath.dirname(p)`.
#[must_use]
pub fn dirname<U: Unit>(p: &[U]) -> Vec<U> {
    split(p).0
}

/// `ntpath.basename(p)`.
#[must_use]
pub fn basename<U: Unit>(p: &[U]) -> &[U] {
    split(p).1
}

/// `ntpath.splitext(p)` (both separators).
#[must_use]
pub fn splitext<U: Unit>(p: &[U]) -> (&[U], &[U]) {
    let sep_idx = p.iter().rposition(|&u| is_sep(u));
    let Some(dot) = p.iter().rposition(|&u| is(u, b'.')) else {
        return (p, &[]);
    };
    if sep_idx.is_none_or(|s| dot > s) {
        let fname_start = sep_idx.map_or(0, |s| s.saturating_add(1));
        if p.get(fname_start..dot)
            .is_some_and(|lead| lead.iter().any(|&u| !is(u, b'.')))
        {
            return p.split_at_checked(dot).unwrap_or((p, &[]));
        }
    }
    (p, &[])
}

/// `ntpath.normcase(p)`: `/` → `\` and ASCII lowercase (CPython lowercases non-ASCII letters too;
/// accepted drift, 15 Q5).
#[must_use]
pub fn normcase<U: Unit>(p: &[U]) -> Vec<U> {
    p.iter()
        .map(|&u| {
            if is(u, b'/') {
                backslash()
            } else {
                ascii_lower(u)
            }
        })
        .collect()
}

/// `ntpath.commonpath(paths)` (3.12 check order: empty, abs/rel mix, drives). Comparison is
/// case-insensitive; the result keeps drive, root and case of `paths[0]`.
pub fn commonpath<U: Unit>(ps: &[&[U]]) -> Result<Vec<U>, PathError> {
    let Some(first) = ps.first() else {
        return Err(PathError::EmptySequence);
    };
    let lowered: Vec<Vec<U>> = ps.iter().map(|p| normcase(p)).collect();
    let splits: Vec<(&[U], &[U], &[U])> = lowered.iter().map(|p| splitroot(p)).collect();
    let mut roots = splits.iter().map(|s| s.1);
    let r0 = roots.next().unwrap_or_default();
    if roots.any(|r| r != r0) {
        return Err(PathError::MixAbsRel);
    }
    let mut drives = splits.iter().map(|s| s.0);
    let d0 = drives.next().unwrap_or_default();
    if drives.any(|d| d != d0) {
        return Err(PathError::DifferentDrives);
    }
    let dot = [U::from_ascii(b'.')];
    let keep = |c: &&[U]| !c.is_empty() && *c != dot.as_slice();
    let first_norm = replace_altsep(first);
    let (drive, root, path) = splitroot(&first_norm);
    let common: Vec<&[U]> = path.split(|&u| is(u, b'\\')).filter(keep).collect();
    let sp: Vec<Vec<&[U]>> = splits
        .iter()
        .map(|s| s.2.split(|&u| is(u, b'\\')).filter(keep).collect())
        .collect();
    let (Some(s1), Some(s2)) = (sp.iter().min(), sp.iter().max()) else {
        return Err(PathError::EmptySequence);
    };
    let n = s1.iter().zip(s2).take_while(|(a, b)| a == b).count();
    let mut out = drive.to_vec();
    out.extend_from_slice(root);
    for (i, c) in common.iter().take(n).enumerate() {
        if i > 0 {
            out.push(backslash());
        }
        out.extend_from_slice(c);
    }
    Ok(out)
}

/// `ntpath.abspath` as CPython computes it without the `nt` module (`_abspath_fallback`):
/// `normpath(join(cwd, p))` unless `p` is absolute by the 3.12 rule. Real Windows uses
/// `GetFullPathNameW`; see `ospath::abspath`.
#[must_use]
pub fn abspath_lexical<U: Unit>(p: &[U], cwd: &[U]) -> Vec<U> {
    if isabs_312(p) {
        normpath(p)
    } else {
        normpath(&join(cwd, &[p]))
    }
}

/// `ntpath.relpath(path, start)` over [`abspath_lexical`].
pub fn relpath<U: Unit>(p: &[U], start: Option<&[U]>, cwd: &[U]) -> Result<Vec<U>, PathError> {
    if p.is_empty() {
        return Err(PathError::NoPathSpecified);
    }
    let dot: Vec<U> = lit(b".");
    let start_abs = abspath_lexical(&normpath(start.unwrap_or(&dot)), cwd);
    let path_abs = abspath_lexical(&normpath(p), cwd);
    let (sd, _, sr) = splitroot(&start_abs);
    let (pd, _, pr) = splitroot(&path_abs);
    if normcase(sd) != normcase(pd) {
        return Err(PathError::DifferentMounts {
            path_drive: to_string_lossy(pd),
            start_drive: to_string_lossy(sd),
        });
    }
    let sl: Vec<&[U]> = sr
        .split(|&u| is(u, b'\\'))
        .filter(|c| !c.is_empty())
        .collect();
    let pl: Vec<&[U]> = pr
        .split(|&u| is(u, b'\\'))
        .filter(|c| !c.is_empty())
        .collect();
    let i = sl
        .iter()
        .zip(&pl)
        .take_while(|(a, b)| normcase(a) == normcase(b))
        .count();
    let dotdot: Vec<U> = lit(b"..");
    let mut rel: Vec<&[U]> = vec![dotdot.as_slice(); sl.len().saturating_sub(i)];
    rel.extend(pl.get(i..).unwrap_or_default());
    match rel.split_first() {
        None => Ok(dot),
        Some((first, rest)) => Ok(join(first, rest)),
    }
}

/// `ntpath.expanduser(p)` (3.12): home = `USERPROFILE` if set (even empty), else
/// `join(HOMEDRIVE or '', HOMEPATH)` if `HOMEPATH` is set, else `p` unchanged; `HOME` is ignored.
/// `~user` replaces the last component of the own home when `USERNAME` names its basename.
pub fn expanduser<'a, U: Unit>(p: &'a [U], env: &dyn Fn(&str) -> Option<Vec<U>>) -> Cow<'a, [U]> {
    if !p.first().is_some_and(|&u| is(u, b'~')) {
        return Cow::Borrowed(p);
    }
    let i = position_from(p, 1, is_sep).unwrap_or(p.len());
    let mut userhome = if let Some(up) = env("USERPROFILE") {
        up
    } else if let Some(hp) = env("HOMEPATH") {
        join(&env("HOMEDRIVE").unwrap_or_default(), &[&hp])
    } else {
        return Cow::Borrowed(p);
    };
    if i != 1 {
        let target = p.get(1..i).unwrap_or_default();
        let current = env("USERNAME");
        if current.as_deref() != Some(target) {
            if current.as_deref() != Some(basename(&userhome)) {
                return Cow::Borrowed(p);
            }
            userhome = join(&dirname(&userhome), &[target]);
        }
    }
    userhome.extend_from_slice(p.get(i..).unwrap_or_default());
    Cow::Owned(userhome)
}

/// Non-strict `ntpath.realpath` (15 §A4.10): `normpath` first (`..` is lexical), `nul` →
/// `\\.\NUL`, relative paths joined to `cwd`, then the final path of the longest existing prefix
/// (`GetFinalPathNameByHandleW` through std) with the unresolved tail re-joined, and the `\\?\`
/// prefix stripped when the stripped form resolves to the same file.
#[cfg(windows)]
pub fn realpath(p: &std::ffi::OsStr, cwd: &std::path::Path) -> std::io::Result<std::path::PathBuf> {
    use std::ffi::OsString;
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use std::path::PathBuf;

    // ERROR_INVALID_FUNCTION … ERROR_CANT_RESOLVE_FILENAME: stop resolving, keep the tail.
    const ALLOWED: &[i32] = &[1, 2, 3, 5, 21, 32, 50, 53, 65, 67, 87, 123, 161, 1920, 1921];

    #[expect(
        clippy::disallowed_methods,
        reason = "GetFinalPathNameByHandleW is ntpath's _getfinalpathname"
    )]
    fn final_path(units: &[u16]) -> std::io::Result<Vec<u16>> {
        std::fs::canonicalize(OsString::from_wide(units))
            .map(|p| p.as_os_str().encode_wide().collect())
    }

    let units: Vec<u16> = p.encode_wide().collect();
    let mut path = normpath(&units);
    if normcase(&path) == lit::<u16>(b"nul") {
        return Ok(PathBuf::from("\\\\.\\NUL"));
    }
    let prefix: Vec<u16> = lit(b"\\\\?\\");
    let unc_prefix: Vec<u16> = lit(b"\\\\?\\UNC\\");
    let had_prefix = path.starts_with(&prefix);
    if !had_prefix && !isabs_312(&path) {
        let cwd_units: Vec<u16> = cwd.as_os_str().encode_wide().collect();
        path = join(&cwd_units, &[&path]);
    }
    let (resolved, initial_error) = match final_path(&path) {
        Ok(r) => (r, None),
        Err(e) => {
            let code = e.raw_os_error();
            if !code.is_some_and(|c| ALLOWED.contains(&c)) {
                return Err(e);
            }
            // walk up until a prefix resolves, then re-join the unresolved tail
            let mut head = path.clone();
            let mut tail: Vec<u16> = Vec::new();
            let resolved = loop {
                if head.is_empty() {
                    break tail;
                }
                if let Ok(r) = final_path(&head) {
                    break if tail.is_empty() {
                        r
                    } else {
                        join(&r, &[&tail])
                    };
                }
                let (parent, name) = split(&head);
                let name = name.to_vec();
                if !parent.is_empty() && name.is_empty() {
                    let mut out = parent;
                    out.extend_from_slice(&tail);
                    break out;
                }
                tail = if tail.is_empty() {
                    name
                } else {
                    join(&name, &[&tail])
                };
                head = parent;
            };
            (resolved, code)
        }
    };
    let mut out = resolved;
    if !had_prefix && out.starts_with(&prefix) {
        let spath: Vec<u16> = if out.starts_with(&unc_prefix) {
            let mut s: Vec<u16> = lit(b"\\\\");
            s.extend_from_slice(out.get(unc_prefix.len()..).unwrap_or_default());
            s
        } else {
            out.get(prefix.len()..).unwrap_or_default().to_vec()
        };
        let same = match final_path(&spath) {
            Ok(r) => r == out,
            // the path did not exist before either: strip the prefix anyway
            Err(e) => e.raw_os_error() == initial_error,
        };
        if same {
            out = spath;
        }
    }
    Ok(PathBuf::from(OsString::from_wide(&out)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[u8]) -> String {
        String::from_utf8(v.to_vec()).unwrap()
    }

    #[test]
    fn splitroot_cases() {
        let t = |p: &str| {
            let (d, r, x) = splitroot(p.as_bytes());
            (s(d), s(r), s(x))
        };
        assert_eq!(
            t("C:\\Windows"),
            ("C:".into(), "\\".into(), "Windows".into())
        );
        assert_eq!(
            t("C:Windows"),
            ("C:".into(), String::new(), "Windows".into())
        );
        assert_eq!(
            t("\\\\srv\\share\\x"),
            ("\\\\srv\\share".into(), "\\".into(), "x".into())
        );
        assert_eq!(
            t("//srv/share/x"),
            ("//srv/share".into(), "/".into(), "x".into())
        );
        assert_eq!(
            t("\\\\?\\UNC\\srv\\share\\x"),
            ("\\\\?\\UNC\\srv\\share".into(), "\\".into(), "x".into())
        );
        assert_eq!(
            t("\\\\srv"),
            ("\\\\srv".into(), String::new(), String::new())
        );
        assert_eq!(t("\\x"), (String::new(), "\\".into(), "x".into()));
        assert_eq!(t("é:x"), ("é:".into(), String::new(), "x".into()));
        assert_eq!(t("x"), (String::new(), String::new(), "x".into()));
    }

    #[test]
    fn normpath_join_split() {
        assert_eq!(s(&normpath(b"C:/a/../b/./c/")), "C:\\b\\c");
        assert_eq!(s(&normpath(b"\\..\\a")), "\\a");
        assert_eq!(s(&normpath(b"C:..\\a")), "C:..\\a");
        assert_eq!(s(&normpath(b"")), ".");
        assert_eq!(s(&join(b"C:\\a", &[b"b"])), "C:\\a\\b");
        assert_eq!(s(&join(b"C:\\a", &[b"D:b"])), "D:b");
        assert_eq!(s(&join(b"C:\\a", &[b"c:b"])), "c:\\a\\b");
        assert_eq!(s(&join(b"C:\\a", &[b"\\b"])), "C:\\b");
        assert_eq!(s(&join(b"\\\\srv\\share", &[b"x"])), "\\\\srv\\share\\x");
        assert_eq!(s(&dirname(b"//srv/share/x")), "//srv/share/");
        assert_eq!(s(basename(b"C:\\a/b")), "b");
        assert_eq!(splitext(b"C:\\a.b\\c"), (&b"C:\\a.b\\c"[..], &b""[..]));
        assert_eq!(s(&normcase(b"C:/Users/U")), "c:\\users\\u");
        assert!(
            isabs_312(b"/x") && !isabs_313(b"/x") && isabs_313(b"C:\\x") && isabs_313(b"\\\\srv")
        );
    }

    #[test]
    fn common_rel() {
        assert_eq!(
            commonpath(&[b"C:\\Users\\u\\x".as_slice(), b"c:\\users\\U"]).map(|v| s(&v)),
            Ok("C:\\Users\\u".into())
        );
        assert_eq!(
            commonpath(&[b"C:\\a".as_slice(), b"D:\\a"]),
            Err(PathError::DifferentDrives)
        );
        assert_eq!(
            commonpath(&[b"C:\\a".as_slice(), b"a"]),
            Err(PathError::MixAbsRel)
        );
        let cwd = b"C:\\r\\sub";
        assert_eq!(
            relpath(b"C:\\r\\sub\\a\\b.py", None, b"C:\\r").map(|v| s(&v)),
            Ok("sub\\a\\b.py".into())
        );
        assert_eq!(
            relpath(b"c:\\R\\SUB\\x.py", None, b"C:\\r").map(|v| s(&v)),
            Ok("SUB\\x.py".into())
        );
        assert_eq!(
            relpath(b"D:\\x.py", None, cwd).map_err(|e| e.to_string()),
            Err("path is on mount 'D:', start on mount 'C:'".into())
        );
        assert_eq!(s(&abspath_lexical(b"..\\c.py", cwd)), "C:\\r\\c.py");
    }

    #[test]
    fn expanduser_cases() {
        let env = |k: &str| match k {
            "USERPROFILE" => Some(b"C:\\Users\\u".to_vec()),
            "USERNAME" => Some(b"u".to_vec()),
            _ => None,
        };
        assert_eq!(s(&expanduser(b"~/.cache", &env)), "C:\\Users\\u/.cache");
        assert_eq!(s(&expanduser(b"~other\\x", &env)), "C:\\Users\\other\\x");
        let env2 = |k: &str| match k {
            "HOMEDRIVE" => Some(b"D:".to_vec()),
            "HOMEPATH" => Some(b"\\home".to_vec()),
            _ => None,
        };
        assert_eq!(s(&expanduser(b"~", &env2)), "D:\\home");
        let none = |_: &str| None;
        assert_eq!(s(&expanduser(b"~", &none)), "~");
    }

    #[test]
    fn utf16_units() {
        let w: Vec<u16> = "C:/Users/u".encode_utf16().collect();
        let n = normpath(&w);
        assert_eq!(String::from_utf16(&n).unwrap(), "C:\\Users\\u");
        let astral: Vec<u16> = "😀:x".encode_utf16().collect();
        let (d, _, _) = splitroot(&astral);
        assert_eq!(d.len(), 3);
    }
}
