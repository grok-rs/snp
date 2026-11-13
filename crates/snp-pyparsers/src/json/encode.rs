//! `json.dumps(obj, indent=…, ensure_ascii=…)` with the default separators for an indent (`(',', ': ')`), as
//! `json/encoder.py` `_make_iterencode` (3.12) and `c_make_encoder` (3.13) render it — byte-identical — plus
//! `pretty_format_json.parse_num_to_int` for the indent. Iterative; output in generalized UTF-8 (a lone surrogate
//! stays raw with `ensure_ascii=False`, which the caller turns into Python's `UnicodeEncodeError`).
//!
//! Owner: WP-17.

use std::borrow::Cow;

use snp_pycompat::chars::{decimal_value, is_space};

use super::value::Val;
use crate::CheckError;
use crate::pystr;

/// The largest output (and `--indent` string) this encoder produces before reporting `MemoryError`: a resource
/// cap (1 GiB), where Python goes on as long as memory lasts. It is a SEC-class divergence (unbounded resources),
/// reachable only with an absurd `--indent` or a multi-GiB document.
pub(super) const MAX_OUTPUT: usize = 1 << 30;

fn memory_error() -> CheckError {
    CheckError::crash("MemoryError", "")
}

/// The per-level indent string (`' ' * n` for an integer `--indent`, else the literal).
#[derive(Debug, Clone)]
pub(super) enum Indent<'a> {
    Str(Cow<'a, str>),
}

impl<'a> Indent<'a> {
    /// `parse_num_to_int(s)` then `json.encoder`'s `' ' * indent` for an `int`: `int(s)` if Python accepts it, else
    /// the string itself. An integer beyond `index` range is Python's `OverflowError`; an indent longer than
    /// [`MAX_OUTPUT`], or one that cannot be allocated, is `MemoryError`.
    pub(super) fn parse(s: &'a str) -> Result<Self, CheckError> {
        if !is_int_syntax(s) {
            return Ok(Self::Str(Cow::Borrowed(s)));
        }
        // `int(s)` raises ValueError past 4300 digits (underscores not counted): the string is used literally.
        if s.chars().filter(|&c| decimal_value(c).is_some()).count() > 4300 {
            return Ok(Self::Str(Cow::Borrowed(s)));
        }
        let overflow = || {
            CheckError::crash(
                "OverflowError",
                "cannot fit 'int' into an index-sized integer",
            )
        };
        // `' ' * n` needs `n` as a `Py_ssize_t`.
        let n = snp_pycompat::int::parse_int(s)
            .ok()
            .and_then(|n| i64::try_from(n).ok())
            .ok_or_else(overflow)?;
        if n <= 0 {
            return Ok(Self::Str(Cow::Borrowed("")));
        }
        let n = usize::try_from(n).unwrap_or(usize::MAX);
        if n > MAX_OUTPUT {
            return Err(memory_error());
        }
        let mut ind = Vec::new();
        ind.try_reserve_exact(n).map_err(|_| memory_error())?;
        ind.resize(n, b' ');
        Ok(Self::Str(Cow::Owned(
            String::from_utf8(ind).unwrap_or_default(),
        )))
    }

    fn as_bytes(&self) -> &[u8] {
        match self {
            Self::Str(s) => s.as_bytes(),
        }
    }
}

/// Whether Python's `int(s)` accepts `s` (whitespace, sign, Unicode decimal digits, single `_` between digits).
fn is_int_syntax(s: &str) -> bool {
    let t = s.trim_matches(is_space);
    let t = t.trim_prefix(['-', '+']);
    let mut prev_digit = false;
    let mut any = false;
    for c in t.chars() {
        if c == '_' {
            if !prev_digit {
                return false;
            }
            prev_digit = false;
        } else if decimal_value(c).is_some() {
            prev_digit = true;
            any = true;
        } else {
            return false;
        }
    }
    any && prev_digit
}

/// One open container while encoding.
enum Open<'v> {
    List(std::slice::Iter<'v, Val>, bool),
    Dict(std::slice::Iter<'v, (Box<[u8]>, Val)>, bool),
}

/// Appends `json.dumps(v, indent=indent, ensure_ascii=ensure_ascii)` to `out`.
pub(super) fn dumps(
    v: &Val,
    indent: &Indent<'_>,
    ensure_ascii: bool,
    out: &mut Vec<u8>,
) -> Result<(), CheckError> {
    let mut lead = Lead {
        ind: indent.as_bytes(),
        buf: vec![b'\n'],
    };
    let mut stack: Vec<Open<'_>> = Vec::new();
    let mut pending: Option<&Val> = Some(v);
    loop {
        if out.len() > MAX_OUTPUT {
            return Err(memory_error());
        }
        if let Some(val) = pending.take() {
            match val {
                Val::List(items) if items.is_empty() => out.extend_from_slice(b"[]"),
                Val::Dict(pairs) if pairs.is_empty() => out.extend_from_slice(b"{}"),
                Val::List(items) => {
                    out.push(b'[');
                    stack.push(Open::List(items.iter(), true));
                }
                Val::Dict(pairs) => {
                    out.push(b'{');
                    stack.push(Open::Dict(pairs.iter(), true));
                }
                scalar => scalar_repr(scalar, ensure_ascii, out),
            }
            continue;
        }
        let level = stack.len();
        let Some(top) = stack.last_mut() else {
            return Ok(());
        };
        let next = match top {
            Open::List(it, first) => it
                .next()
                .map(|v| (None, v, std::mem::replace(first, false))),
            Open::Dict(it, first) => it
                .next()
                .map(|(k, v)| (Some(k), v, std::mem::replace(first, false))),
        };
        if let Some((key, val, first)) = next {
            if !first {
                out.push(b',');
            }
            lead.write(out, level)?;
            if let Some(k) = key {
                encode_str(k, ensure_ascii, out);
                out.extend_from_slice(b": ");
            }
            pending = Some(val);
        } else {
            let close = if matches!(top, Open::List(..)) {
                b']'
            } else {
                b'}'
            };
            stack.pop();
            lead.write(out, stack.len())?;
            out.push(close);
        }
    }
}

/// `'\n' + indent * level`, built once per depth and copied with one `memcpy` per line.
struct Lead<'i> {
    ind: &'i [u8],
    buf: Vec<u8>,
}

impl Lead<'_> {
    /// Appends the line break and indent of `level`; `MemoryError` before anything grows past [`MAX_OUTPUT`] (a
    /// huge indent times a deep level would otherwise abort on allocation failure).
    fn write(&mut self, out: &mut Vec<u8>, level: usize) -> Result<(), CheckError> {
        let want = level.saturating_mul(self.ind.len()).saturating_add(1);
        if want > MAX_OUTPUT || out.len().saturating_add(want) > MAX_OUTPUT {
            return Err(memory_error());
        }
        if self.buf.len() < want {
            self.buf
                .try_reserve(want.saturating_sub(self.buf.len()))
                .map_err(|_| memory_error())?;
        }
        while self.buf.len() < want {
            self.buf.extend_from_slice(self.ind);
        }
        out.try_reserve(want).map_err(|_| memory_error())?;
        out.extend_from_slice(self.buf.get(..want).unwrap_or(&self.buf));
        Ok(())
    }
}

/// `None`/`True`/`False`, `int.__repr__`, `floatstr`, or the encoded string.
fn scalar_repr(v: &Val, ensure_ascii: bool, out: &mut Vec<u8>) {
    match v {
        Val::Null => out.extend_from_slice(b"null"),
        Val::Bool(true) => out.extend_from_slice(b"true"),
        Val::Bool(false) => out.extend_from_slice(b"false"),
        Val::Int(i) => out.extend_from_slice(i.as_bytes()),
        Val::Float(f) => {
            if f.is_nan() {
                out.extend_from_slice(b"NaN");
            } else if f.is_infinite() {
                out.extend_from_slice(if *f > 0.0 {
                    b"Infinity".as_slice()
                } else {
                    b"-Infinity".as_slice()
                });
            } else {
                out.extend_from_slice(snp_pycompat::float::repr(*f).as_bytes());
            }
        }
        Val::Str(s) => encode_str(s, ensure_ascii, out),
        Val::List(_) | Val::Dict(_) => {}
    }
}

/// `encode_basestring_ascii` / `encode_basestring`.
fn encode_str(s: &[u8], ensure_ascii: bool, out: &mut Vec<u8>) {
    out.push(b'"');
    let push_u = |out: &mut Vec<u8>, u: u32| {
        out.extend_from_slice(format!("\\u{u:04x}").as_bytes());
    };
    for cp in pystr::code_points(s) {
        match cp {
            0x22 => out.extend_from_slice(b"\\\""),
            0x5C => out.extend_from_slice(b"\\\\"),
            0x08 => out.extend_from_slice(b"\\b"),
            0x0C => out.extend_from_slice(b"\\f"),
            0x0A => out.extend_from_slice(b"\\n"),
            0x0D => out.extend_from_slice(b"\\r"),
            0x09 => out.extend_from_slice(b"\\t"),
            0x00..=0x1F => push_u(out, cp),
            0x20..=0x7E => out.push(u8::try_from(cp).unwrap_or(b'?')),
            _ if !ensure_ascii => pystr::push_cp(out, cp),
            0x1_0000.. => {
                let v = cp.saturating_sub(0x1_0000);
                push_u(out, 0xD800 | ((v >> 10) & 0x3FF));
                push_u(out, 0xDC00 | (v & 0x3FF));
            }
            _ => push_u(out, cp),
        }
    }
    out.push(b'"');
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dump(v: &Val, indent: &str, ascii: bool) -> String {
        let mut out = Vec::new();
        dumps(v, &Indent::parse(indent).unwrap(), ascii, &mut out).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn shapes() {
        let v = Val::Dict(vec![
            (
                (*b"a").into(),
                Val::List(vec![
                    Val::Int("1".into()),
                    Val::Float(1.5),
                    Val::List(Vec::new()),
                ]),
            ),
            ((*b"b").into(), Val::Dict(Vec::new())),
        ]);
        assert_eq!(
            dump(&v, "2", true),
            "{\n  \"a\": [\n    1,\n    1.5,\n    []\n  ],\n  \"b\": {}\n}"
        );
        assert_eq!(
            dump(&v, "0", true),
            "{\n\"a\": [\n1,\n1.5,\n[]\n],\n\"b\": {}\n}"
        );
        assert_eq!(
            dump(&v, "\t", true),
            "{\n\t\"a\": [\n\t\t1,\n\t\t1.5,\n\t\t[]\n\t],\n\t\"b\": {}\n}"
        );
        assert_eq!(dump(&Val::Float(1e16), "2", true), "1e+16");
        assert_eq!(dump(&Val::Float(f64::NAN), "2", true), "NaN");
    }

    #[test]
    fn strings() {
        let mut s = Vec::new();
        for cp in [
            0x00, 0x1F, 0x7F, 0x20, 0xD800, 0xE9, 0x1F600, 0x22, 0x5C, 0x2F, 0x08, 0x0C, 0x0A,
            0x0D, 0x09, 0x2028,
        ] {
            pystr::push_cp(&mut s, cp);
        }
        let mut out = Vec::new();
        encode_str(&s, true, &mut out);
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "\"\\u0000\\u001f\\u007f \\ud800\\u00e9\\ud83d\\ude00\\\"\\\\/\\b\\f\\n\\r\\t\\u2028\""
        );
        let mut out = Vec::new();
        encode_str("\u{7f}é\u{2028}\u{1}".as_bytes(), false, &mut out);
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "\"\u{7f}é\u{2028}\\u0001\""
        );
    }

    #[test]
    fn indent_parsing() {
        assert!(matches!(Indent::parse(" 3 ").unwrap(), Indent::Str(s) if s == "   "));
        assert!(matches!(Indent::parse("1_0").unwrap(), Indent::Str(s) if s.len() == 10));
        assert!(matches!(Indent::parse("-5").unwrap(), Indent::Str(s) if s.is_empty()));
        assert!(matches!(Indent::parse("ab").unwrap(), Indent::Str(s) if s == "ab"));
        assert!(matches!(Indent::parse("1__0").unwrap(), Indent::Str(s) if s == "1__0"));
        assert!(matches!(Indent::parse("٣").unwrap(), Indent::Str(s) if s == "   "));
        assert!(Indent::parse("99999999999999999999999999999999999999999").is_err());
    }
}
