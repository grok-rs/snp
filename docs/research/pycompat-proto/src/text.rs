//! `str` / `bytes` splitting and stripping with CPython semantics.
use crate::unicode::{is_ascii_space, is_linebreak, is_space};

/// `s.splitlines(keepends)`.
#[must_use]
pub fn splitlines(s: &str, keepends: bool) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut it = s.char_indices().peekable();
    while let Some((i, c)) = it.next() {
        if !is_linebreak(c) {
            continue;
        }
        let mut end = i + c.len_utf8();
        if c == '\r' && it.next_if(|&(_, n)| n == '\n').is_some() {
            end += 1;
        }
        out.push(s.get(start..if keepends { end } else { i }).unwrap_or_default());
        start = end;
    }
    if start < s.len() {
        out.push(s.get(start..).unwrap_or_default());
    }
    out
}

/// `b.splitlines(keepends)`: boundaries are `\r\n`, `\r`, `\n` only.
#[must_use]
pub fn bytes_splitlines(b: &[u8], keepends: bool) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while let Some(&c) = b.get(i) {
        if c == b'\n' || c == b'\r' {
            let mut end = i + 1;
            if c == b'\r' && b.get(i + 1) == Some(&b'\n') {
                end += 1;
            }
            out.push(b.get(start..if keepends { end } else { i }).unwrap_or_default());
            start = end;
            i = end;
        } else {
            i += 1;
        }
    }
    if start < b.len() {
        out.push(b.get(start..).unwrap_or_default());
    }
    out
}

/// `s.strip()` (Unicode whitespace as in `str.isspace`).
#[must_use]
pub fn strip(s: &str) -> &str {
    s.trim_matches(is_space)
}
/// `s.lstrip()`.
#[must_use]
pub fn lstrip(s: &str) -> &str {
    s.trim_start_matches(is_space)
}
/// `s.rstrip()`.
#[must_use]
pub fn rstrip(s: &str) -> &str {
    s.trim_end_matches(is_space)
}
/// `s.strip(chars)` — `chars` is a *set* of characters, not a suffix.
#[must_use]
pub fn strip_chars<'a>(s: &'a str, chars: &str) -> &'a str {
    s.trim_matches(|c| chars.contains(c))
}
/// `s.lstrip(chars)`.
#[must_use]
pub fn lstrip_chars<'a>(s: &'a str, chars: &str) -> &'a str {
    s.trim_start_matches(|c| chars.contains(c))
}
/// `s.rstrip(chars)`.
#[must_use]
pub fn rstrip_chars<'a>(s: &'a str, chars: &str) -> &'a str {
    s.trim_end_matches(|c| chars.contains(c))
}

/// `b.strip()` (ASCII set `b' \t\n\r\x0b\x0c'`).
#[must_use]
pub fn bytes_strip(b: &[u8]) -> &[u8] {
    bytes_rstrip(bytes_lstrip(b))
}
/// `b.lstrip()`.
#[must_use]
pub fn bytes_lstrip(b: &[u8]) -> &[u8] {
    let n = b.iter().take_while(|&&c| is_ascii_space(c)).count();
    b.get(n..).unwrap_or_default()
}
/// `b.rstrip()`.
#[must_use]
pub fn bytes_rstrip(b: &[u8]) -> &[u8] {
    let n = b.iter().rev().take_while(|&&c| is_ascii_space(c)).count();
    b.get(..b.len() - n).unwrap_or_default()
}
/// `b.strip(set)`.
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
    b.get(..b.len() - n).unwrap_or_default()
}

/// `s.split(None, maxsplit)`; `None` means unlimited (`-1`).
///
/// Runs of whitespace are one separator, leading whitespace is skipped; when
/// `maxsplit` is reached the remainder keeps its *trailing* whitespace.
#[must_use]
pub fn split_whitespace(s: &str, maxsplit: Option<usize>) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = s;
    let mut remaining = maxsplit.unwrap_or(usize::MAX);
    loop {
        let t = lstrip(rest);
        if t.is_empty() {
            return out;
        }
        if remaining == 0 {
            out.push(t);
            return out;
        }
        let end = t.char_indices().find(|&(_, c)| is_space(c)).map_or(t.len(), |(i, _)| i);
        let (tok, tail) = t.split_at(end);
        out.push(tok);
        rest = tail;
        remaining -= 1;
    }
}

/// `s.rsplit(None, maxsplit)`, returned left-to-right like Python.
///
/// When `maxsplit` is reached the remainder keeps its *leading* whitespace:
/// `'  a b c d'.rsplit(maxsplit=3) == ['  a', 'b', 'c', 'd']`.
#[must_use]
pub fn rsplit_whitespace(s: &str, maxsplit: Option<usize>) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = s;
    let mut remaining = maxsplit.unwrap_or(usize::MAX);
    loop {
        let t = rstrip(rest);
        if t.is_empty() {
            break;
        }
        if remaining == 0 {
            out.push(t);
            break;
        }
        let start = t
            .char_indices()
            .rev()
            .find(|&(_, c)| is_space(c))
            .map_or(0, |(i, c)| i + c.len_utf8());
        let (head, tok) = t.split_at(start);
        out.push(tok);
        rest = head;
        remaining -= 1;
    }
    out.reverse();
    out
}

/// `s.partition(sep)` (`sep` non-empty).
#[must_use]
pub fn partition<'a>(s: &'a str, sep: &str) -> (&'a str, &'a str, &'a str) {
    match s.find(sep) {
        Some(i) => {
            let (a, rest) = s.split_at(i);
            let (m, b) = rest.split_at(sep.len());
            (a, m, b)
        }
        None => (s, "", ""),
    }
}
