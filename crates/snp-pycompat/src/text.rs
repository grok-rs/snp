//! `str` / `bytes` splitting and stripping with CPython semantics (15 §A4.1–§A4.2;
//! `Objects/stringlib/split.h`, `Objects/unicodeobject.c`, `Objects/bytes_methods.c`).
//!
//! Rust's `str::lines`, `split_whitespace`, `trim` and `<[u8]>::trim_ascii` use different character
//! sets (15 §A8); every pre-commit call site goes through these functions instead. The line
//! splitters scan bytes, never decode to `char`s, and allocate nothing.

use std::fmt;
use std::iter::FusedIterator;

use crate::chars::{is_bytes_space, is_space};
use crate::messages;

/// Whether `b` can start a PY-LB boundary in UTF-8 text: the ASCII breaks `\n \x0b \x0c \r`,
/// `\x1c..\x1e`, or the lead bytes of U+0085 (`C2 85`) and U+2028/U+2029 (`E2 80 A8`/`A9`).
const fn may_start_linebreak(b: u8) -> bool {
    matches!(b, b'\n'..=b'\r' | 0x1c..=0x1e | 0xc2 | 0xe2)
}

/// Whether `b` can end a PY-LB boundary in UTF-8 text (the ASCII breaks, or the last byte of
/// U+0085, U+2028, U+2029).
const fn may_end_linebreak(b: u8) -> bool {
    matches!(b, b'\n'..=b'\r' | 0x1c..=0x1e | 0x85 | 0xa8 | 0xa9)
}

/// The first PY-LB boundary of `s` as `(start, len)` in bytes; `"\r\n"` is one boundary.
fn find_linebreak(s: &str) -> Option<(usize, usize)> {
    let b = s.as_bytes();
    let mut from = 0_usize;
    loop {
        let off = b
            .get(from..)?
            .iter()
            .position(|&c| may_start_linebreak(c))?;
        let i = from.checked_add(off)?;
        let at = |k: usize| b.get(i.checked_add(k)?).copied();
        let len = match at(0) {
            Some(b'\r') if at(1) == Some(b'\n') => 2,
            Some(0xc2) => {
                if at(1) == Some(0x85) {
                    2
                } else {
                    0
                }
            }
            Some(0xe2) => {
                if at(1) == Some(0x80) && matches!(at(2), Some(0xa8 | 0xa9)) {
                    3
                } else {
                    0
                }
            }
            _ => 1,
        };
        if len > 0 {
            return Some((i, len));
        }
        from = i.checked_add(1)?;
    }
}

/// Length in bytes of the PY-LB boundary that ends `b` (0 when `b` does not end with one).
const fn trailing_linebreak_len(b: &[u8]) -> usize {
    match b {
        [.., b'\r', b'\n'] | [.., 0xc2, 0x85] => 2,
        [.., b'\n'..=b'\r' | 0x1c..=0x1e] => 1,
        [.., 0xe2, 0x80, 0xa8 | 0xa9] => 3,
        _ => 0,
    }
}

/// The offset just after the last PY-LB boundary inside `b` (0 when there is none).
fn last_linebreak_end(b: &[u8]) -> usize {
    let mut end = b.len();
    while let Some(j) = b
        .get(..end)
        .and_then(|h| h.iter().rposition(|&c| may_end_linebreak(c)))
    {
        let candidate = j.saturating_add(1);
        if trailing_linebreak_len(b.get(..candidate).unwrap_or_default()) > 0 {
            return candidate;
        }
        end = j;
    }
    0
}

/// `str.splitlines()` / `str.splitlines(True)`: zero-copy, Unicode line boundaries (PY-LB, with
/// `"\r\n"` as one boundary; `Objects/stringlib/split.h` `STRINGLIB(splitlines)`). The text after
/// the last boundary is a line only when it is non-empty (`'a\n'` → `['a']`, `''` → `[]`).
#[derive(Clone, Debug)]
pub struct SplitLines<'a> {
    rest: &'a str,
    keepends: bool,
}

impl<'a> Iterator for SplitLines<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<&'a str> {
        if self.rest.is_empty() {
            return None;
        }
        let s = self.rest;
        let Some((i, n)) = find_linebreak(s) else {
            self.rest = "";
            return Some(s);
        };
        let (with_end, rest) = s.split_at_checked(i.checked_add(n)?)?;
        self.rest = rest;
        if self.keepends {
            Some(with_end)
        } else {
            with_end.get(..i)
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        if self.rest.is_empty() {
            (0, Some(0))
        } else {
            (1, Some(self.rest.len()))
        }
    }
}

impl DoubleEndedIterator for SplitLines<'_> {
    fn next_back(&mut self) -> Option<Self::Item> {
        let s = self.rest;
        if s.is_empty() {
            return None;
        }
        let content_end = s.len().saturating_sub(trailing_linebreak_len(s.as_bytes()));
        let start = last_linebreak_end(s.as_bytes().get(..content_end)?);
        let line = if self.keepends {
            s.get(start..)?
        } else {
            s.get(start..content_end)?
        };
        self.rest = s.get(..start)?;
        Some(line)
    }
}

impl FusedIterator for SplitLines<'_> {}

/// `bytes.splitlines()` / `bytes.splitlines(True)`: boundaries are `\n`, `\r` and `\r\n` only
/// (B-LB).
#[derive(Clone, Debug)]
pub struct BytesSplitLines<'a> {
    rest: &'a [u8],
    keepends: bool,
}

impl<'a> Iterator for BytesSplitLines<'a> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<&'a [u8]> {
        if self.rest.is_empty() {
            return None;
        }
        let b = self.rest;
        let Some(i) = b.iter().position(|&c| c == b'\n' || c == b'\r') else {
            self.rest = &[];
            return Some(b);
        };
        let crlf = b.get(i) == Some(&b'\r') && b.get(i.saturating_add(1)) == Some(&b'\n');
        let end = i.saturating_add(if crlf { 2 } else { 1 });
        let (with_end, rest) = b.split_at_checked(end)?;
        self.rest = rest;
        if self.keepends {
            Some(with_end)
        } else {
            with_end.get(..i)
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        if self.rest.is_empty() {
            (0, Some(0))
        } else {
            (1, Some(self.rest.len()))
        }
    }
}

impl DoubleEndedIterator for BytesSplitLines<'_> {
    fn next_back(&mut self) -> Option<Self::Item> {
        let b = self.rest;
        if b.is_empty() {
            return None;
        }
        let terminator = match b {
            [.., b'\r', b'\n'] => 2,
            [.., b'\n' | b'\r'] => 1,
            _ => 0,
        };
        let content_end = b.len().saturating_sub(terminator);
        let start = b
            .get(..content_end)?
            .iter()
            .rposition(|&c| c == b'\n' || c == b'\r')
            .map_or(0, |j| j.saturating_add(1));
        let line = if self.keepends {
            b.get(start..)?
        } else {
            b.get(start..content_end)?
        };
        self.rest = b.get(..start)?;
        Some(line)
    }
}

impl FusedIterator for BytesSplitLines<'_> {}

/// `str.split(maxsplit=n)` (whitespace split, `sep=None`; `STRINGLIB(split_whitespace)`).
#[derive(Clone, Debug)]
pub struct SplitWs<'a> {
    rest: &'a str,
    remaining: Option<usize>,
}

impl<'a> Iterator for SplitWs<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<&'a str> {
        let t = lstrip(self.rest);
        if t.is_empty() {
            self.rest = "";
            return None;
        }
        if self.remaining == Some(0) {
            // maxsplit reached: the remainder keeps its trailing whitespace
            self.rest = "";
            return Some(t);
        }
        let end = t
            .char_indices()
            .find(|&(_, c)| is_space(c))
            .map_or(t.len(), |(i, _)| i);
        let (tok, tail) = t.split_at_checked(end)?;
        self.rest = tail;
        if let Some(r) = self.remaining.as_mut() {
            *r = r.saturating_sub(1);
        }
        Some(tok)
    }
}

impl FusedIterator for SplitWs<'_> {}

/// `s.splitlines()`.
#[must_use]
pub fn splitlines(s: &str) -> SplitLines<'_> {
    SplitLines {
        rest: s,
        keepends: false,
    }
}

/// `s.splitlines(True)`.
#[must_use]
pub fn splitlines_keepends(s: &str) -> SplitLines<'_> {
    SplitLines {
        rest: s,
        keepends: true,
    }
}

/// `b.splitlines()`.
#[must_use]
pub fn bytes_splitlines(b: &[u8]) -> BytesSplitLines<'_> {
    BytesSplitLines {
        rest: b,
        keepends: false,
    }
}

/// `b.splitlines(True)`.
#[must_use]
pub fn bytes_splitlines_keepends(b: &[u8]) -> BytesSplitLines<'_> {
    BytesSplitLines {
        rest: b,
        keepends: true,
    }
}

/// `s.split(None, maxsplit)`; `None` means unlimited (`-1`). Runs of PY-WS are one separator,
/// leading whitespace is skipped, and once `maxsplit` is reached the remainder keeps its
/// *trailing* whitespace (`'  a b  c  '.split(None, 2)` → `['a', 'b', 'c  ']`).
#[must_use]
pub fn split_whitespace(s: &str, maxsplit: Option<usize>) -> SplitWs<'_> {
    SplitWs {
        rest: s,
        remaining: maxsplit,
    }
}

/// `s.rsplit(None, maxsplit)`, returned left-to-right like Python. Once `maxsplit` is reached the
/// remainder keeps its *leading* whitespace: `'  a b c d'.rsplit(maxsplit=3) == ['  a', 'b', 'c', 'd']`.
#[must_use]
pub fn rsplit_whitespace(s: &str, maxsplit: Option<usize>) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = s;
    let mut remaining = maxsplit;
    loop {
        let t = rstrip(rest);
        if t.is_empty() {
            break;
        }
        if remaining == Some(0) {
            out.push(t);
            break;
        }
        let start = t
            .char_indices()
            .rev()
            .find(|&(_, c)| is_space(c))
            .map_or(0, |(i, c)| i.saturating_add(c.len_utf8()));
        let Some((head, tok)) = t.split_at_checked(start) else {
            break;
        };
        out.push(tok);
        rest = head;
        if let Some(r) = remaining.as_mut() {
            *r = r.saturating_sub(1);
        }
    }
    out.reverse();
    out
}

/// `b.rsplit(None, maxsplit)` (B-WS separators), returned left-to-right.
#[must_use]
pub fn bytes_rsplit_whitespace(b: &[u8], maxsplit: Option<usize>) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let mut rest = b;
    let mut remaining = maxsplit;
    loop {
        let t = bytes_rstrip(rest);
        if t.is_empty() {
            break;
        }
        if remaining == Some(0) {
            out.push(t);
            break;
        }
        let start = t
            .iter()
            .rposition(|&c| is_bytes_space(c))
            .map_or(0, |i| i.saturating_add(1));
        let Some((head, tok)) = t.split_at_checked(start) else {
            break;
        };
        out.push(tok);
        rest = head;
        if let Some(r) = remaining.as_mut() {
            *r = r.saturating_sub(1);
        }
    }
    out.reverse();
    out
}

/// `ValueError` of a tuple unpack, e.g. `a, b, c, d = s.rsplit(maxsplit=3)` (`hook_impl.py:130`;
/// `Python/ceval.c` `unpack_iterable`, 3.12 wording).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct UnpackError {
    /// Number of targets.
    pub expected: usize,
    /// Number of values produced.
    pub got: usize,
}

impl UnpackError {
    /// The Python exception class: always `ValueError`.
    #[must_use]
    pub const fn py_type(&self) -> &'static str {
        messages::VALUE_ERROR
    }
}

impl fmt::Display for UnpackError {
    /// `not enough values to unpack (expected 4, got 3)` / `too many values to unpack (expected 4)`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.got > self.expected {
            messages::unpack_too_many(f, self.expected)
        } else {
            messages::unpack_not_enough(f, self.expected, self.got)
        }
    }
}

impl std::error::Error for UnpackError {}

/// `a, b, c, d = s.rsplit(maxsplit=3)` (pre-push lines, `hook_impl.py:129-130`; no allocation).
pub fn rsplit4(s: &str) -> Result<[&str; 4], UnpackError> {
    let mut parts: [&str; 4] = [""; 4];
    let mut rest = s;
    let mut n = 0_usize;
    for slot in parts.iter_mut().rev() {
        let t = rstrip(rest);
        if t.is_empty() {
            break;
        }
        n = n.saturating_add(1);
        if n == 4 {
            *slot = t;
            break;
        }
        let start = t
            .char_indices()
            .rev()
            .find(|&(_, c)| is_space(c))
            .map_or(0, |(i, c)| i.saturating_add(c.len_utf8()));
        let Some((head, tok)) = t.split_at_checked(start) else {
            break;
        };
        *slot = tok;
        rest = head;
    }
    if n == 4 {
        Ok(parts)
    } else {
        Err(UnpackError {
            expected: 4,
            got: n,
        })
    }
}

/// `a, b, c, d = b.rsplit(maxsplit=3)` (B-WS; the non-UTF-8 pre-push path, D-CRASH-11).
pub fn bytes_rsplit4(b: &[u8]) -> Result<[&[u8]; 4], UnpackError> {
    let mut parts: [&[u8]; 4] = [&[]; 4];
    let mut rest = b;
    let mut n = 0_usize;
    for slot in parts.iter_mut().rev() {
        let t = bytes_rstrip(rest);
        if t.is_empty() {
            break;
        }
        n = n.saturating_add(1);
        if n == 4 {
            *slot = t;
            break;
        }
        let start = t
            .iter()
            .rposition(|&c| is_bytes_space(c))
            .map_or(0, |i| i.saturating_add(1));
        let Some((head, tok)) = t.split_at_checked(start) else {
            break;
        };
        *slot = tok;
        rest = head;
    }
    if n == 4 {
        Ok(parts)
    } else {
        Err(UnpackError {
            expected: 4,
            got: n,
        })
    }
}

/// `s.strip()` (PY-WS).
#[must_use]
pub fn strip(s: &str) -> &str {
    s.trim_matches(is_space)
}

/// `s.lstrip()` (PY-WS).
#[must_use]
pub fn lstrip(s: &str) -> &str {
    s.trim_start_matches(is_space)
}

/// `s.rstrip()` (PY-WS).
#[must_use]
pub fn rstrip(s: &str) -> &str {
    s.trim_end_matches(is_space)
}

/// `s.strip(chars)`: `chars` is a *set* of characters, not a prefix/suffix (`strip('')` strips
/// nothing).
#[must_use]
pub fn strip_chars<'a>(s: &'a str, set: &str) -> &'a str {
    s.trim_matches(|c| set.contains(c))
}

/// `s.lstrip(chars)`.
#[must_use]
pub fn lstrip_chars<'a>(s: &'a str, set: &str) -> &'a str {
    s.trim_start_matches(|c| set.contains(c))
}

/// `s.rstrip(chars)` (`'a\r\n'.rstrip('\n')` → `'a\r'`).
#[must_use]
pub fn rstrip_chars<'a>(s: &'a str, set: &str) -> &'a str {
    s.trim_end_matches(|c| set.contains(c))
}

/// `b.strip()` (B-WS: `b' \t\n\r\x0b\x0c'`; not `<[u8]>::trim_ascii`, which keeps `\x0b`).
#[must_use]
pub fn bytes_strip(b: &[u8]) -> &[u8] {
    bytes_rstrip(bytes_lstrip(b))
}

/// `b.lstrip()`.
#[must_use]
pub fn bytes_lstrip(b: &[u8]) -> &[u8] {
    let n = b.iter().take_while(|&&c| is_bytes_space(c)).count();
    b.get(n..).unwrap_or_default()
}

/// `b.rstrip()`.
#[must_use]
pub fn bytes_rstrip(b: &[u8]) -> &[u8] {
    let n = b.iter().rev().take_while(|&&c| is_bytes_space(c)).count();
    b.get(..b.len().saturating_sub(n)).unwrap_or_default()
}

/// `b.strip(set)` (a set of bytes: `b'foo\r\r\n'.strip(b'\r\n')` → `b'foo'`).
#[must_use]
pub fn bytes_strip_set<'a>(b: &'a [u8], set: &[u8]) -> &'a [u8] {
    bytes_rstrip_set(bytes_lstrip_set(b, set), set)
}

/// `b.lstrip(set)`.
#[must_use]
pub fn bytes_lstrip_set<'a>(b: &'a [u8], set: &[u8]) -> &'a [u8] {
    let n = b.iter().take_while(|c| set.contains(c)).count();
    b.get(n..).unwrap_or_default()
}

/// `b.rstrip(set)`.
#[must_use]
pub fn bytes_rstrip_set<'a>(b: &'a [u8], set: &[u8]) -> &'a [u8] {
    let n = b.iter().rev().take_while(|c| set.contains(c)).count();
    b.get(..b.len().saturating_sub(n)).unwrap_or_default()
}

/// Length in bytes of the PY-WS character that starts `b`, if `b` starts with one.
fn leading_space_len(b: &[u8]) -> Option<usize> {
    let lead = *b.first()?;
    let len = match lead {
        0x00..=0x7f => 1,
        0xc0..=0xdf => 2,
        0xe0..=0xef => 3,
        _ => 4,
    };
    let s = core::str::from_utf8(b.get(..len)?).ok()?;
    let c = s.chars().next()?;
    is_space(c).then_some(len)
}

/// Length in bytes of the PY-WS character that ends `b`, if `b` ends with one. UTF-8 is
/// self-synchronising, so a valid sequence at the end decodes the same in the whole string.
fn trailing_space_len(b: &[u8]) -> Option<usize> {
    (1..=4_usize).find_map(|k| {
        let tail = b.get(b.len().checked_sub(k)?..)?;
        let s = core::str::from_utf8(tail).ok()?;
        let mut cs = s.chars();
        let c = cs.next()?;
        (cs.next().is_none() && is_space(c)).then_some(k)
    })
}

/// `os.fsdecode(b).strip()` as bytes (POSIX `surrogateescape`; `run.py:129-131` `SKIP`): PY-WS
/// characters decoded as UTF-8 are stripped; undecodable bytes are never whitespace.
#[must_use]
pub fn strip_fsbytes(b: &[u8]) -> &[u8] {
    let mut start = 0_usize;
    while let Some(n) = b.get(start..).and_then(leading_space_len) {
        start = start.saturating_add(n);
    }
    let mut end = b.len();
    while end > start {
        let Some(n) = b.get(start..end).and_then(trailing_space_len) else {
            break;
        };
        end = end.saturating_sub(n);
    }
    b.get(start..end).unwrap_or_default()
}

/// `s.partition(sep)`: `(head, sep, tail)`, or `(s, "", "")` when `sep` does not occur
/// (`languages/rust.py:76`). Python rejects an empty `sep` (`ValueError: empty separator`); here an
/// empty `sep` splits before the first character.
#[must_use]
pub fn partition<'a>(s: &'a str, sep: &str) -> (&'a str, &'a str, &'a str) {
    match s.find(sep) {
        Some(at) => {
            let end = at.saturating_add(sep.len());
            match (s.get(..at), s.get(at..end), s.get(end..)) {
                (Some(head), Some(mid), Some(tail)) => (head, mid, tail),
                _ => (s, "", ""),
            }
        }
        None => (s, "", ""),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sl(s: &str) -> Vec<&str> {
        splitlines(s).collect()
    }
    fn slk(s: &str) -> Vec<&str> {
        splitlines_keepends(s).collect()
    }

    const SAMPLES: &[&str] = &[
        "",
        "\n",
        "a",
        "a\n",
        "a\nb",
        "a\nb\n",
        "a\r\nb\r\n",
        "a\r\rb",
        "a\r\r\n",
        "\r\n\r\n",
        "x\u{2028}y\u{85}z\x0b\x0c\x1c\x1d\x1e",
        "a\n\nb\n\n",
        "\r",
        "\n\r",
        "é\r\nü",
        "\u{145}\u{2029}\u{2028}\u{85}x\u{85}",
        "\u{c5}\u{85}\u{e2}\u{2080}\u{a8}",
        "\u{2018}\u{20a8}\u{2828}",
    ];

    #[test]
    fn splitlines_matches_python() {
        assert_eq!(sl(""), Vec::<&str>::new());
        assert_eq!(sl("\n"), vec![""]);
        assert_eq!(sl("a\nb\n"), vec!["a", "b"]);
        assert_eq!(sl("a\r\nb"), vec!["a", "b"]);
        assert_eq!(sl("a\rb\x0bc"), vec!["a", "b", "c"]);
        assert_eq!(sl("a\x1fb"), vec!["a\x1fb"]);
        assert_eq!(slk("a\r\nb\rc\n"), vec!["a\r\n", "b\r", "c\n"]);
        assert_eq!(sl("x\u{2028}y\u{85}z"), vec!["x", "y", "z"]);
        // C2/E2 lead bytes of other characters are not boundaries
        assert_eq!(sl("\u{a0}\u{2018}\u{20a8}"), vec!["\u{a0}\u{2018}\u{20a8}"]);
        // continuation bytes 0x85/0xa8 of other characters are not boundaries either
        assert_eq!(sl("\u{145}\u{2828}"), vec!["\u{145}\u{2828}"]);
    }

    #[test]
    fn splitlines_back_equals_forward() {
        for s in SAMPLES {
            for keep in [false, true] {
                let fwd: Vec<&str> = SplitLines {
                    rest: s,
                    keepends: keep,
                }
                .collect();
                let mut back: Vec<&str> = SplitLines {
                    rest: s,
                    keepends: keep,
                }
                .rev()
                .collect();
                back.reverse();
                assert_eq!(fwd, back, "{s:?} keepends={keep}");
                // interleaved
                let mut it = SplitLines {
                    rest: s,
                    keepends: keep,
                };
                let mut front = Vec::new();
                let mut tail = Vec::new();
                while let Some(x) = it.next() {
                    front.push(x);
                    let Some(y) = it.next_back() else { break };
                    tail.push(y);
                }
                tail.reverse();
                front.extend(tail);
                assert_eq!(front, fwd, "{s:?} interleaved");
            }
        }
    }

    #[test]
    fn bytes_splitlines_only_cr_lf() {
        let v: Vec<&[u8]> = bytes_splitlines(b"a\x0bb\rc\r\nd\n").collect();
        assert_eq!(v, vec![&b"a\x0bb"[..], b"c", b"d"]);
        let v: Vec<&[u8]> = bytes_splitlines_keepends(b"a\r\n\nb").collect();
        assert_eq!(v, vec![&b"a\r\n"[..], b"\n", b"b"]);
        assert_eq!(bytes_splitlines(b"").count(), 0);
        for s in SAMPLES {
            for keep in [false, true] {
                let it = BytesSplitLines {
                    rest: s.as_bytes(),
                    keepends: keep,
                };
                let fwd: Vec<&[u8]> = it.clone().collect();
                let mut back: Vec<&[u8]> = it.rev().collect();
                back.reverse();
                assert_eq!(fwd, back, "{s:?} keepends={keep}");
            }
        }
    }

    #[test]
    fn whitespace_split() {
        let v: Vec<&str> = split_whitespace("  a  b\x1fc ", None).collect();
        assert_eq!(v, vec!["a", "b", "c"]);
        let v: Vec<&str> = split_whitespace("  a  b c  ", Some(1)).collect();
        assert_eq!(v, vec!["a", "b c  "]);
        assert_eq!(split_whitespace("   ", Some(0)).count(), 0);
        assert_eq!(
            rsplit_whitespace("  a b c d", Some(3)),
            vec!["  a", "b", "c", "d"]
        );
        assert_eq!(rsplit_whitespace("a b ", None), vec!["a", "b"]);
        assert_eq!(
            bytes_rsplit_whitespace(b" a\x0bb c ", Some(1)),
            vec![&b" a\x0bb"[..], b"c"]
        );
    }

    #[test]
    fn rsplit4_values_and_errors() {
        assert_eq!(
            rsplit4("  refs/heads/x o1 refs/heads/x z\n"),
            Ok(["  refs/heads/x", "o1", "refs/heads/x", "z"])
        );
        assert_eq!(
            rsplit4("a b c"),
            Err(UnpackError {
                expected: 4,
                got: 3
            })
        );
        assert_eq!(
            rsplit4(""),
            Err(UnpackError {
                expected: 4,
                got: 0
            })
        );
        assert_eq!(rsplit4("a\u{a0}b o1 x y"), Ok(["a\u{a0}b", "o1", "x", "y"]));
        assert_eq!(
            UnpackError {
                expected: 4,
                got: 1
            }
            .to_string(),
            "not enough values to unpack (expected 4, got 1)"
        );
        assert_eq!(
            UnpackError {
                expected: 2,
                got: 3
            }
            .to_string(),
            "too many values to unpack (expected 2)"
        );
        assert_eq!(
            bytes_rsplit4(b"a\xffb o1 x y\r"),
            Ok([&b"a\xffb"[..], b"o1", b"x", b"y"])
        );
        assert_eq!(
            bytes_rsplit4(b"a\xa0b o1 x"),
            Err(UnpackError {
                expected: 4,
                got: 3
            })
        );
    }

    #[test]
    fn strips() {
        assert_eq!(strip("\x1f a \u{3000}"), "a");
        assert_eq!(strip("\u{200b}a"), "\u{200b}a");
        assert_eq!(strip_chars("xxaxyx", "xy"), "a");
        assert_eq!(lstrip_chars("##\tx#", "#"), "\tx#");
        assert_eq!(rstrip_chars("a\r\n\n", "\r\n"), "a");
        assert_eq!(bytes_strip(b"\x0b a \x0c"), b"a");
        assert_eq!(bytes_strip(b"\xa0a"), b"\xa0a");
        assert_eq!(bytes_strip_set(b"##a#", b"#"), b"a");
    }

    #[test]
    fn strip_fsbytes_is_utf8_aware() {
        assert_eq!(strip_fsbytes(b" \xc2\xa0flake8\xe3\x80\x80 "), b"flake8");
        assert_eq!(strip_fsbytes(b"\xa0x\xa0"), b"\xa0x\xa0");
        assert_eq!(strip_fsbytes(b" \xff "), b"\xff");
        assert_eq!(strip_fsbytes(b"\x1c\x1f"), b"");
        assert_eq!(strip_fsbytes(b"\xe2\xc2\xa0"), b"\xe2");
    }

    #[test]
    fn partition_cases() {
        assert_eq!(partition("a=b=c", "="), ("a", "=", "b=c"));
        assert_eq!(partition("abc", "="), ("abc", "", ""));
        assert_eq!(partition("abc", ""), ("", "", "abc"));
    }
}
