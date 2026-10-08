//! CPython 3.12 `ntpath` (`Lib/ntpath.py`, the pure-Python `normpath` and `_abspath_fallback` it
//! uses off Windows; `Lib/genericpath.py`), generic over path code units: `u8` for UTF-8 data and
//! the vectors, `u16` for `OsStr::encode_wide` on Windows. Every lexical function runs on every OS
//! (15 §A4.10).
//!
//! Separators, `:` and `.` are ASCII, so the algorithms work unit by unit; a character is one
//! UTF-8 sequence or UTF-16 pair where Python looks at one code point (`splitroot`'s drive letter),
//! and lone surrogates / undecodable bytes pass through untouched. Case folding (`normcase`, drive
//! comparison in `join`, `relpath`, `commonpath`) is Python's `str.lower()`: full Unicode lowercase
//! with the final-sigma rule (15 Q5; real Windows `normcase` uses `LCMapStringEx`, which agrees for
//! every cased letter except word-final `Σ` and U+0130).

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

/// The text encoding behind a unit type.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Encoding {
    /// `u8`: UTF-8, undecodable bytes viewed as `surrogateescape` surrogates.
    Utf8,
    /// `u16`: UTF-16 (WTF-16), lone surrogates kept.
    Utf16,
    /// A foreign `Unit` implementation: only ASCII is understood.
    Other,
}

fn encoding<U: Unit>() -> Encoding {
    if TypeId::of::<U>() == TypeId::of::<u8>() {
        Encoding::Utf8
    } else if TypeId::of::<U>() == TypeId::of::<u16>() {
        Encoding::Utf16
    } else {
        Encoding::Other
    }
}

/// The raw value of a unit (`u8`/`u16`; a foreign unit maps its ASCII units, else U+FFFD).
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

/// `p` as bytes when `U` is `u8`.
fn as_bytes<U: Unit>(p: &[U]) -> Vec<u8> {
    p.iter()
        .filter_map(|&u| u8::try_from(raw(u)).ok())
        .collect()
}

/// `p` as UTF-16 units when `U` is `u16`.
fn as_wide<U: Unit>(p: &[U]) -> Vec<u16> {
    p.iter()
        .filter_map(|&u| u16::try_from(raw(u)).ok())
        .collect()
}

/// `v` as units of type `U` when `U` is `u8`.
fn from_bytes<U: Unit>(v: Vec<u8>) -> Option<Vec<U>> {
    let any: Box<dyn Any> = Box::new(v);
    any.downcast::<Vec<U>>().ok().map(|b| *b)
}

/// `v` as units of type `U` when `U` is `u16`.
fn from_wide<U: Unit>(v: Vec<u16>) -> Option<Vec<U>> {
    let any: Box<dyn Any> = Box::new(v);
    any.downcast::<Vec<U>>().ok().map(|b| *b)
}

/// Number of units of the first Python character of `p` (one UTF-8 sequence or UTF-16 pair; an
/// undecodable unit or a lone surrogate is one character).
fn first_char_len<U: Unit>(p: &[U]) -> usize {
    let Some(&first) = p.first() else { return 0 };
    if first.to_ascii().is_some() {
        return 1;
    }
    match encoding::<U>() {
        Encoding::Utf8 => {
            let head = as_bytes(p.get(..4).unwrap_or(p));
            match core::str::from_utf8(&head) {
                Ok(s) => s.chars().next().map_or(1, char::len_utf8),
                Err(e) => head
                    .get(..e.valid_up_to())
                    .and_then(|v| core::str::from_utf8(v).ok())
                    .and_then(|s| s.chars().next())
                    .map_or(1, char::len_utf8),
            }
        }
        Encoding::Utf16 => {
            let hi = matches!(raw(first), 0xd800..=0xdbff);
            let lo = p.get(1).is_some_and(|u| matches!(raw(*u), 0xdc00..=0xdfff));
            if hi && lo { 2 } else { 1 }
        }
        Encoding::Other => 1,
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

/// Python's `str.lower()` of the path as CPython holds it: `str::to_lowercase` on every run of
/// valid text (full mappings, final sigma), undecodable bytes and lone surrogates unchanged — they
/// are neither cased nor case-ignorable, so they end a final-sigma context like the string end.
fn lower<U: Unit>(p: &[U]) -> Vec<U> {
    if p.iter().all(|u| u.to_ascii().is_some()) {
        return p.iter().map(|&u| ascii_lower(u)).collect();
    }
    let unicode = match encoding::<U>() {
        Encoding::Utf8 => {
            let bytes = as_bytes(p);
            let mut out = Vec::new();
            for chunk in bytes.utf8_chunks() {
                out.extend_from_slice(chunk.valid().to_lowercase().as_bytes());
                out.extend_from_slice(chunk.invalid());
            }
            from_bytes(out)
        }
        Encoding::Utf16 => {
            let mut out: Vec<u16> = Vec::new();
            let mut run = String::new();
            for r in char::decode_utf16(as_wide(p)) {
                match r {
                    Ok(c) => run.push(c),
                    Err(e) => {
                        out.extend(run.to_lowercase().encode_utf16());
                        run.clear();
                        out.push(e.unpaired_surrogate());
                    }
                }
            }
            out.extend(run.to_lowercase().encode_utf16());
            from_wide(out)
        }
        Encoding::Other => None,
    };
    unicode.unwrap_or_else(|| p.iter().map(|&u| ascii_lower(u)).collect())
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

/// The drive as Python text (for `%r` in `path is on mount …`; lone surrogates become U+FFFD).
fn to_string_lossy<U: Unit>(p: &[U]) -> String {
    match encoding::<U>() {
        Encoding::Utf16 => String::from_utf16_lossy(&as_wide(p)),
        Encoding::Utf8 | Encoding::Other => String::from_utf8_lossy(&as_bytes(p)).into_owned(),
    }
}

/// `ntpath.splitroot(p)` (3.12): `(drive, root, tail)` with `/` treated as `\`. UNC
/// (`\\server\share`, `\\?\UNC\server\share`) and device (`\\.\dev`, `\\?\X:`) drives end before
/// the separator after the share name (no such separator: the whole input is the drive); `\x` has
/// a root only; `X:` (any first character, colon second) is a drive with an optional root.
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

/// The first three characters are `X:\`-like (any character, colon, separator).
fn has_colon_sep<U: Unit>(p: &[U]) -> bool {
    let c1 = first_char_len(p);
    p.get(c1).is_some_and(|&u| is(u, b':'))
        && p.get(c1.saturating_add(1)).is_some_and(|&u| is_sep(u))
}

/// `ntpath.isabs(p)` with CPython 3.12 semantics: `\x` and `/x` are absolute ("LEGACY BUG"), as
/// are drive + root and UNC paths.
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

/// `ntpath.normpath(p)` (3.12, the pure-Python version): `/` → `\`; `''`/`.` components dropped;
/// `..` pops a previous non-`..` component, is dropped right after a root and kept otherwise
/// (`C:..` stays); drive and root are kept verbatim (`\\?\` paths are normalised like any other);
/// an empty result without drive or root is `.`.
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

/// `ntpath.join(path, *paths)` (3.12): a later rooted part replaces drive + root (keeping the old
/// drive when it has none); a later different drive (`str.lower()` comparison) replaces
/// everything; the same drive in another case adopts the later spelling; a `\` is added unless the
/// path ends in a separator; a UNC drive with a relative tail gets a `\`.
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
            if lower(pd) != lower(&rd) {
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

/// `ntpath.split(p)`: the head is drive + root + the dirname without trailing separators
/// (`'//srv/share/x'` → `('//srv/share/', 'x')`).
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

/// `ntpath.dirname(p)` (`split(p)[0]`).
#[must_use]
pub fn dirname<U: Unit>(p: &[U]) -> Vec<U> {
    split(p).0
}

/// `ntpath.basename(p)` (`split(p)[1]`).
#[must_use]
pub fn basename<U: Unit>(p: &[U]) -> &[U] {
    split(p).1
}

/// `ntpath.splitext(p)` (`genericpath._splitext` over both separators; leading dots of the name
/// never start an extension).
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

/// `ntpath.normcase(p)`: `/` → `\` and Python's `str.lower()` (CPython's non-Windows
/// implementation, which generated the vectors; real Windows uses `LCMapStringEx`, see the module
/// documentation).
#[must_use]
pub fn normcase<U: Unit>(p: &[U]) -> Vec<U> {
    lower(&replace_altsep(p))
}

/// `ntpath.commonpath(paths)` (3.12 check order: empty, abs/rel mix, drives; `lang_base.py:70`).
/// Comparison is on `str.lower()` copies; the result keeps drive, root and case of `paths[0]`
/// with `\` separators.
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

/// `ntpath.relpath` over any `abspath` (3.12): both sides go through `abspath(normpath(x))`,
/// drives are compared with `normcase`, components with `normcase`, and the result is
/// `join(*rel_list)` (so a component that looks like a drive restarts the join, as in 3.12).
pub(crate) fn relpath_with<U: Unit>(
    p: &[U],
    start: Option<&[U]>,
    abspath: &dyn Fn(&[U]) -> Vec<U>,
) -> Result<Vec<U>, PathError> {
    if p.is_empty() {
        return Err(PathError::NoPathSpecified);
    }
    let dot: Vec<U> = lit(b".");
    let start_abs = abspath(&normpath(start.unwrap_or(&dot)));
    let path_abs = abspath(&normpath(p));
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

/// `ntpath.relpath(path, start)` (3.12; `main.py:191-199`) over [`abspath_lexical`] with an
/// explicit `cwd`; `start = None` means the cwd. Across drives:
/// `ValueError: path is on mount 'D:', start on mount 'C:'`.
pub fn relpath<U: Unit>(p: &[U], start: Option<&[U]>, cwd: &[U]) -> Result<Vec<U>, PathError> {
    relpath_with(p, start, &|x| abspath_lexical(x, cwd))
}

/// `ntpath.expanduser(p)` (3.12): home = `USERPROFILE` if set (even empty), else
/// `join(HOMEDRIVE or '', HOMEPATH)` if `HOMEPATH` is set, else `p` unchanged; `HOME` is ignored.
/// `~user` replaces the last component of the own home when `USERNAME` names its basename. No
/// separator normalisation (`'C:\\Users\\u/.cache'`).
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

/// Windows `GetFinalPathNameByHandleW` (`nt._getfinalpathname`) through std.
#[cfg(windows)]
#[expect(
    clippy::disallowed_methods,
    reason = "std::fs::canonicalize is GetFinalPathNameByHandleW, i.e. ntpath's _getfinalpathname"
)]
fn final_path(units: &[u16]) -> std::io::Result<Vec<u16>> {
    use std::ffi::OsString;
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    std::fs::canonicalize(OsString::from_wide(units)).map(|p| p.as_os_str().encode_wide().collect())
}

/// `ntpath._readlink_deep` (non-strict): follows links with `nt.readlink` until a non-link, a loop
/// (by `normcase`) or an allowed error; a relative target resolves against the link's directory
/// when the link is a symlink, else the walk stops.
#[cfg(windows)]
fn readlink_deep(start: Vec<u16>) -> std::io::Result<Vec<u16>> {
    use std::collections::HashSet;
    use std::ffi::OsString;
    use std::os::windows::ffi::{OsStrExt, OsStringExt};

    // ERROR_INVALID_FUNCTION … ERROR_REPARSE_TAG_INVALID
    const ALLOWED: &[i32] = &[1, 2, 3, 5, 21, 32, 50, 67, 87, 4390, 4392, 4393];
    let mut seen: HashSet<Vec<u16>> = HashSet::new();
    let mut path = start;
    while seen.insert(normcase(&path)) {
        let old = path.clone();
        match std::fs::read_link(OsString::from_wide(&path)) {
            Ok(target) => {
                let target: Vec<u16> = target.as_os_str().encode_wide().collect();
                if isabs_312(&target) {
                    path = target;
                } else {
                    let is_symlink = std::fs::symlink_metadata(OsString::from_wide(&old))
                        .is_ok_and(|m| m.file_type().is_symlink());
                    if !is_symlink {
                        path = old;
                        break;
                    }
                    path = normpath(&join(&dirname(&old), &[&target]));
                }
            }
            // an unsupported reparse point is Python's ValueError: stop there
            Err(e) if e.raw_os_error().is_none_or(|c| ALLOWED.contains(&c)) => break,
            Err(e) => return Err(e),
        }
    }
    Ok(path)
}

/// `ntpath._getfinalpathname_nonstrict`: the final path of the longest resolvable prefix (or of a
/// dangling link's target) with the unresolved tail re-joined.
#[cfg(windows)]
fn final_path_nonstrict(start: Vec<u16>) -> std::io::Result<Vec<u16>> {
    // ERROR_INVALID_FUNCTION … ERROR_CANT_RESOLVE_FILENAME
    const ALLOWED: &[i32] = &[1, 2, 3, 5, 21, 32, 50, 53, 65, 67, 87, 123, 161, 1920, 1921];
    let with_tail = |p: Vec<u16>, tail: &[u16]| {
        if tail.is_empty() {
            p
        } else {
            join(&p, &[tail])
        }
    };
    let mut path = start;
    let mut tail: Vec<u16> = Vec::new();
    while !path.is_empty() {
        match final_path(&path) {
            Ok(p) => return Ok(with_tail(p, &tail)),
            Err(e) => {
                if !e.raw_os_error().is_some_and(|c| ALLOWED.contains(&c)) {
                    return Err(e);
                }
                if let Ok(new_path) = readlink_deep(path.clone())
                    && new_path != path
                {
                    return Ok(with_tail(new_path, &tail));
                }
                let (parent, name) = split(&path);
                if !parent.is_empty() && name.is_empty() {
                    let mut out = parent;
                    out.extend_from_slice(&tail);
                    return Ok(out);
                }
                tail = if tail.is_empty() {
                    name.to_vec()
                } else {
                    join(name, &[&tail])
                };
                path = parent;
            }
        }
    }
    Ok(tail)
}

/// Non-strict `ntpath.realpath(p)` (3.12, 15 §A4.10): `normpath` first (`..` is lexical), `nul` →
/// `\\.\NUL`, relative paths joined to `cwd`, then `GetFinalPathNameByHandleW` (links, junctions,
/// real case, SUBST drives) with the non-strict walk-up of `_getfinalpathname_nonstrict`, and the
/// `\\?\` prefix stripped when the stripped form resolves to the same file (or failed the same
/// way). A path with an embedded NUL is returned normalised, like CPython's `ValueError` branch.
#[cfg(windows)]
pub fn realpath(p: &std::ffi::OsStr, cwd: &std::path::Path) -> std::io::Result<std::path::PathBuf> {
    use std::ffi::OsString;
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use std::path::PathBuf;

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
    if path.contains(&0) {
        return Ok(PathBuf::from(OsString::from_wide(&normpath(&path))));
    }
    let (mut out, initial_error) = match final_path(&path) {
        Ok(r) => (r, 0),
        Err(e) => {
            let code = e.raw_os_error().unwrap_or(0);
            (final_path_nonstrict(path)?, code)
        }
    };
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
            Err(e) => e.raw_os_error() == Some(initial_error),
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
    fn invalid_utf8_drive_letters_are_one_character_per_byte() {
        // b'\xe0\x80:x' is two surrogate-escaped characters before the colon: no drive
        let (d, _, _) = splitroot(b"\xe0\x80:x".as_slice());
        assert_eq!(d, b"");
        let (d, _, _) = splitroot(b"\xff:x".as_slice());
        assert_eq!(d, b"\xff:");
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
    fn unicode_case_folding_is_str_lower() {
        assert_eq!(
            s(&normcase("C:/Élodie/ΟΔΟΣ".as_bytes())),
            "c:\\élodie\\οδος"
        );
        assert_eq!(s(&normcase("ΣΑΣ x".as_bytes())), "σας x");
        assert_eq!(s(&normcase("İ".as_bytes())), "i\u{307}");
        assert_eq!(normcase(b"A\xffB".as_slice()), b"a\xffb");
        let w: Vec<u16> = "É\u{1}".encode_utf16().chain([0xd800]).collect();
        let lw = normcase(&w);
        assert_eq!(lw, [0xe9, 1, 0xd800]);
        assert_eq!(
            s(&join("é:\\a".as_bytes(), &["É:b".as_bytes()])),
            "É:\\a\\b"
        );
        assert_eq!(
            relpath(
                "C:\\Users\\Élodie\\repo\\x".as_bytes(),
                Some("c:\\users\\élodie\\repo".as_bytes()),
                b"C:\\"
            )
            .map(|v| s(&v)),
            Ok("x".into())
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
