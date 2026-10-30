//! The CPython 3.12/3.13 `_json` C scanner as an explicit-stack state machine over generalized UTF-8 text
//! (`Modules/_json.c`: `scan_once_unicode`, `_parse_object_unicode`, `_parse_array_unicode`, `scanstring_unicode`,
//! `_match_number_unicode`; `json/decoder.py`: `JSONDecodeError`, `JSONDecoder.decode`).
//!
//! Positions are byte offsets internally; messages convert them to code-point indices exactly as Python reports
//! them (every syntax character is ASCII, so byte arithmetic on them equals code-point arithmetic, except inside
//! `\uXXXX` escapes where the scanner counts code points like the C code).
//!
//! Owner: WP-17.

use super::limits::{self, Limits, Path};
use super::value::{self, Val};
use super::{DumpOpts, JsonValue, PyMinor};
use crate::CheckError;
use crate::pystr;

/// A scalar the scanner produced.
enum Scalar {
    Null,
    Bool(bool),
    Int(Box<str>),
    Float(f64),
    Str(Vec<u8>),
}

/// What the scanner builds; the object hook is the `object_pairs_hook` of the emulated call.
trait Build {
    type V;
    /// Whether string values must be decoded (keys always are).
    const STRINGS: bool;
    fn scalar(&mut self, s: Scalar) -> Self::V;
    fn array(&mut self, items: Vec<Self::V>) -> Self::V;
    /// The hook for an object at `depth` (open containers including itself).
    fn object(
        &mut self,
        cx: &Ctx<'_>,
        pairs: Vec<(Box<[u8]>, Self::V)>,
        depth: usize,
    ) -> Result<Self::V, CheckError>;
}

/// Text, version and thresholds, for error construction.
struct Ctx<'t> {
    s: &'t [u8],
    minor: PyMinor,
    lim: Limits,
}

impl Ctx<'_> {
    /// `JSONDecodeError(msg, s, pos)` text: `{msg}: line {lineno} column {colno} (char {pos})`.
    fn decode_error_text(&self, msg: &str, at: usize) -> String {
        let prefix = self.s.get(..at).unwrap_or(self.s);
        let pos = pystr::char_count(prefix);
        let (lineno, colno) = match prefix.iter().rposition(|&b| b == b'\n') {
            Some(nl) => {
                let nl_char = pystr::char_count(prefix.get(..nl).unwrap_or_default());
                let lines = prefix.split(|&b| b == b'\n').count().saturating_sub(1);
                (lines.saturating_add(1), pos.saturating_sub(nl_char))
            }
            None => (1, pos.saturating_add(1)),
        };
        format!("{msg}: line {lineno} column {colno} (char {pos})")
    }

    /// `raise_errmsg(msg, s, at)` inside `depth` open containers.
    fn errmsg(&self, msg: &str, at: usize, depth: usize) -> CheckError {
        if depth > self.lim.errmsg {
            // Just past the threshold, and at the deepest enterable level, the C call itself overflows.
            let calling = depth == self.lim.errmsg.saturating_add(1) || depth > self.lim.stop;
            let text = if calling {
                limits::CALLING
            } else {
                limits::PLAIN
            };
            return CheckError::crash("RecursionError", text);
        }
        CheckError::Reported(self.decode_error_text(msg, at))
    }

    /// `raise_stop_iteration(at)` inside `depth` containers, turned into `Expecting value` by `raw_decode`.
    fn stop(&self, at: usize, depth: usize) -> CheckError {
        if depth > self.lim.stop {
            return CheckError::crash("RecursionError", limits::CALLING);
        }
        CheckError::Reported(self.decode_error_text("Expecting value", at))
    }

    /// A one-C-call event (`parse_constant`, the int-limit `ValueError`) inside `depth` containers.
    fn call(&self, depth: usize) -> Result<(), CheckError> {
        if depth > self.lim.stop {
            return Err(CheckError::crash("RecursionError", limits::CALLING));
        }
        Ok(())
    }

    /// Calling the Python `object_pairs_hook` for an object at `depth`.
    fn hook(&self, depth: usize) -> Result<(), CheckError> {
        if depth > self.lim.hook {
            let over = depth.saturating_sub(self.lim.hook);
            let text = if over <= self.lim.hook_calling {
                limits::CALLING
            } else {
                limits::PLAIN
            };
            return Err(CheckError::crash("RecursionError", text));
        }
        Ok(())
    }
}

/// One open container.
enum Frame<V> {
    Array(Vec<V>),
    Object {
        pairs: Vec<(Box<[u8]>, V)>,
        key: Option<Box<[u8]>>,
    },
}

/// What scanning one value at a position gave.
enum Step<V> {
    /// A complete value and the index after it.
    Value(V, usize),
    /// A container was opened; scan its first value at this index.
    Opened(usize),
}

struct Scanner<'t, B: Build> {
    cx: Ctx<'t>,
    b: B,
    stack: Vec<Frame<B::V>>,
    max_depth: usize,
    max_float_depth: Option<usize>,
}

const fn is_ws(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r')
}

fn hex(b: u8) -> Option<u32> {
    char::from(b).to_digit(16)
}

impl<'t, B: Build> Scanner<'t, B> {
    fn new(s: &'t [u8], minor: PyMinor, path: Path, b: B) -> Self {
        Self {
            cx: Ctx {
                s,
                minor,
                lim: Limits::of(path, minor),
            },
            b,
            stack: Vec::new(),
            max_depth: 0,
            max_float_depth: None,
        }
    }

    fn at(&self, i: usize) -> Option<u8> {
        self.cx.s.get(i).copied()
    }

    fn len(&self) -> usize {
        self.cx.s.len()
    }

    fn skip_ws(&self, mut i: usize) -> usize {
        while self.at(i).is_some_and(is_ws) {
            i = i.saturating_add(1);
        }
        i
    }

    fn depth(&self) -> usize {
        self.stack.len()
    }

    /// `JSONDecoder.decode`: whitespace, one value, whitespace, `Extra data`.
    fn parse(mut self) -> Result<(B::V, Self), CheckError> {
        let mut idx = self.skip_ws(0);
        loop {
            let (mut val, mut next) = match self.scan_value(idx)? {
                Step::Value(v, n) => (v, n),
                Step::Opened(n) => {
                    idx = n;
                    continue;
                }
            };
            // Deliver `val` to the innermost container; closing containers delivers again.
            loop {
                let minor = self.cx.minor;
                let depth = self.depth();
                match self.stack.last_mut() {
                    None => {
                        let end = self.skip_ws(next);
                        if end != self.len() {
                            return Err(self.cx.errmsg("Extra data", end, 0));
                        }
                        return Ok((val, self));
                    }
                    Some(Frame::Array(items)) => {
                        items.push(val);
                        let i = self.skip_ws(next);
                        match self.at(i) {
                            Some(b']') => {
                                let Some(Frame::Array(items)) = self.stack.pop() else {
                                    return unreachable_frame();
                                };
                                val = self.b.array(items);
                                next = i.saturating_add(1);
                            }
                            Some(b',') => {
                                let j = self.skip_ws(i.saturating_add(1));
                                if matches!(minor, PyMinor::P313) && self.at(j) == Some(b']') {
                                    return Err(self.cx.errmsg(
                                        "Illegal trailing comma before end of array",
                                        i,
                                        depth,
                                    ));
                                }
                                idx = j;
                                break;
                            }
                            _ => return Err(self.cx.errmsg("Expecting ',' delimiter", i, depth)),
                        }
                    }
                    Some(Frame::Object { pairs, key }) => {
                        pairs.push((key.take().unwrap_or_default(), val));
                        let i = self.skip_ws(next);
                        match self.at(i) {
                            Some(b'}') => {
                                let Some(Frame::Object { pairs, .. }) = self.stack.pop() else {
                                    return unreachable_frame();
                                };
                                val = self.b.object(&self.cx, pairs, depth)?;
                                next = i.saturating_add(1);
                            }
                            Some(b',') => {
                                let j = self.skip_ws(i.saturating_add(1));
                                if matches!(minor, PyMinor::P313) && self.at(j) == Some(b'}') {
                                    return Err(self.cx.errmsg(
                                        "Illegal trailing comma before end of object",
                                        i,
                                        depth,
                                    ));
                                }
                                idx = self.read_key(j)?;
                                break;
                            }
                            _ => return Err(self.cx.errmsg("Expecting ',' delimiter", i, depth)),
                        }
                    }
                }
            }
        }
    }

    /// `scan_once_unicode(idx)`.
    fn scan_value(&mut self, idx: usize) -> Result<Step<B::V>, CheckError> {
        let depth = self.depth();
        let Some(c) = self.at(idx) else {
            return Err(self.cx.stop(idx, depth));
        };
        let s = self.cx.s;
        let rest = s.get(idx..).unwrap_or_default();
        match c {
            b'"' => {
                let mut buf = Vec::new();
                let next =
                    self.scanstring(idx.saturating_add(1), B::STRINGS.then_some(&mut buf))?;
                Ok(Step::Value(self.b.scalar(Scalar::Str(buf)), next))
            }
            b'{' | b'[' => {
                let object = c == b'{';
                let new_depth = depth.saturating_add(1);
                if new_depth > self.cx.lim.enter {
                    return Err(CheckError::crash(
                        "RecursionError",
                        limits::entering(object),
                    ));
                }
                self.max_depth = self.max_depth.max(new_depth);
                let i = self.skip_ws(idx.saturating_add(1));
                if object {
                    if self.at(i) == Some(b'}') {
                        let v = self.b.object(&self.cx, Vec::new(), new_depth)?;
                        return Ok(Step::Value(v, i.saturating_add(1)));
                    }
                    self.stack.push(Frame::Object {
                        pairs: Vec::new(),
                        key: None,
                    });
                    Ok(Step::Opened(self.read_key(i)?))
                } else {
                    if self.at(i) == Some(b']') {
                        return Ok(Step::Value(self.b.array(Vec::new()), i.saturating_add(1)));
                    }
                    self.stack.push(Frame::Array(Vec::new()));
                    Ok(Step::Opened(i))
                }
            }
            b'n' if rest.starts_with(b"null") => Ok(Step::Value(
                self.b.scalar(Scalar::Null),
                idx.saturating_add(4),
            )),
            b't' if rest.starts_with(b"true") => Ok(Step::Value(
                self.b.scalar(Scalar::Bool(true)),
                idx.saturating_add(4),
            )),
            b'f' if rest.starts_with(b"false") => Ok(Step::Value(
                self.b.scalar(Scalar::Bool(false)),
                idx.saturating_add(5),
            )),
            b'N' if rest.starts_with(b"NaN") => self.constant(f64::NAN, idx, 3),
            b'I' if rest.starts_with(b"Infinity") => self.constant(f64::INFINITY, idx, 8),
            b'-' if rest.starts_with(b"-Infinity") => self.constant(f64::NEG_INFINITY, idx, 9),
            _ => self.number(idx),
        }
    }

    /// `_parse_constant`: one C call to `parse_constant`.
    fn constant(&mut self, f: f64, idx: usize, len: usize) -> Result<Step<B::V>, CheckError> {
        let depth = self.depth();
        self.cx.call(depth)?;
        self.note_float(depth);
        Ok(Step::Value(
            self.b.scalar(Scalar::Float(f)),
            idx.saturating_add(len),
        ))
    }

    fn note_float(&mut self, depth: usize) {
        self.max_float_depth = Some(self.max_float_depth.map_or(depth, |d| d.max(depth)));
    }

    /// After `{` or `,` in an object: `"key"`, whitespace, `:`, whitespace; returns the index of the value.
    fn read_key(&mut self, i: usize) -> Result<usize, CheckError> {
        let depth = self.depth();
        if self.at(i) != Some(b'"') {
            return Err(self.cx.errmsg(
                "Expecting property name enclosed in double quotes",
                i,
                depth,
            ));
        }
        let mut key = Vec::new();
        let next = self.scanstring(i.saturating_add(1), Some(&mut key))?;
        let j = self.skip_ws(next);
        if self.at(j) != Some(b':') {
            return Err(self.cx.errmsg("Expecting ':' delimiter", j, depth));
        }
        if let Some(Frame::Object { key: slot, .. }) = self.stack.last_mut() {
            *slot = Some(key.into_boxed_slice());
        }
        Ok(self.skip_ws(j.saturating_add(1)))
    }

    /// The byte index `n` code points after `i`, if the text has that many.
    fn advance_chars(&self, i: usize, n: usize) -> Option<usize> {
        let mut pos = i;
        for _ in 0..n {
            let b = self.at(pos)?;
            let width = match b {
                0x00..=0x7F => 1,
                0xC0..=0xDF => 2,
                0xE0..=0xEF => 3,
                _ => 4,
            };
            pos = pos.saturating_add(width);
        }
        (pos <= self.len()).then_some(pos)
    }

    /// Four hex digits at byte `i` (all ASCII when valid).
    fn hex4(&self, i: usize) -> Option<u32> {
        let mut v = 0u32;
        for k in 0..4 {
            v = (v << 4) | hex(self.at(i.saturating_add(k))?)?;
        }
        Some(v)
    }

    /// `scanstring_unicode(end)`: `end` is just after the opening quote; returns the index after the closing one.
    fn scanstring(&self, end: usize, mut out: Option<&mut Vec<u8>>) -> Result<usize, CheckError> {
        let depth = self.depth();
        let begin = end.saturating_sub(1);
        let s = self.cx.s;
        let mut end = end;
        loop {
            let mut next = end;
            while let Some(d) = self.at(next) {
                if d == b'"' || d == b'\\' {
                    break;
                }
                if d <= 0x1F {
                    return Err(self.cx.errmsg("Invalid control character at", next, depth));
                }
                next = next.saturating_add(1);
            }
            let Some(c) = self.at(next) else {
                return Err(self
                    .cx
                    .errmsg("Unterminated string starting at", begin, depth));
            };
            if let Some(buf) = out.as_deref_mut() {
                buf.extend_from_slice(s.get(end..next).unwrap_or_default());
            }
            next = next.saturating_add(1);
            if c == b'"' {
                return Ok(next);
            }
            let Some(esc) = self.at(next) else {
                return Err(self
                    .cx
                    .errmsg("Unterminated string starting at", begin, depth));
            };
            let cp: u32 = if esc == b'u' {
                let first = next.saturating_add(1);
                // `end = next + 4; if (end >= len)`: at least 5 code points from the first digit.
                if self.advance_chars(first, 5).is_none() {
                    return Err(self.cx.errmsg("Invalid \\uXXXX escape", next, depth));
                }
                let Some(mut cp) = self.hex4(first) else {
                    return Err(self.cx.errmsg("Invalid \\uXXXX escape", next, depth));
                };
                end = first.saturating_add(4);
                // Surrogate pair: `end + 6 < len`, then `\u` and four more hex digits.
                if (0xD800..=0xDBFF).contains(&cp)
                    && self.advance_chars(end, 7).is_some()
                    && self.at(end) == Some(b'\\')
                    && self.at(end.saturating_add(1)) == Some(b'u')
                {
                    let second_u = end.saturating_add(1);
                    let Some(lo) = self.hex4(end.saturating_add(2)) else {
                        return Err(self.cx.errmsg("Invalid \\uXXXX escape", second_u, depth));
                    };
                    if (0xDC00..=0xDFFF).contains(&lo) {
                        cp = pystr::join_surrogates(cp, lo);
                        end = end.saturating_add(6);
                    }
                }
                cp
            } else {
                end = next.saturating_add(1);
                match esc {
                    b'"' => u32::from(b'"'),
                    b'\\' => u32::from(b'\\'),
                    b'/' => u32::from(b'/'),
                    b'b' => 0x08,
                    b'f' => 0x0C,
                    b'n' => u32::from(b'\n'),
                    b'r' => u32::from(b'\r'),
                    b't' => u32::from(b'\t'),
                    _ => {
                        return Err(self.cx.errmsg(
                            "Invalid \\escape",
                            end.saturating_sub(2),
                            depth,
                        ));
                    }
                }
            };
            if let Some(buf) = out.as_deref_mut() {
                pystr::push_cp(buf, cp);
            }
        }
    }

    /// `_match_number_unicode(start)`.
    fn number(&mut self, start: usize) -> Result<Step<B::V>, CheckError> {
        let depth = self.depth();
        let digit = |i: usize| self.at(i).is_some_and(|b| b.is_ascii_digit());
        let len = self.len();
        let mut idx = start;
        if self.at(idx) == Some(b'-') {
            idx = idx.saturating_add(1);
            if idx >= len {
                return Err(self.cx.stop(start, depth));
            }
        }
        match self.at(idx) {
            Some(b'1'..=b'9') => {
                idx = idx.saturating_add(1);
                while digit(idx) {
                    idx = idx.saturating_add(1);
                }
            }
            Some(b'0') => idx = idx.saturating_add(1),
            _ => return Err(self.cx.stop(start, depth)),
        }
        let mut is_float = false;
        // `idx < end_idx` = at least one more character after `idx`.
        let more = |i: usize| i.saturating_add(1) < len;
        if more(idx) && self.at(idx) == Some(b'.') && digit(idx.saturating_add(1)) {
            is_float = true;
            idx = idx.saturating_add(2);
            while digit(idx) {
                idx = idx.saturating_add(1);
            }
        }
        if more(idx) && matches!(self.at(idx), Some(b'e' | b'E')) {
            let e_start = idx;
            idx = idx.saturating_add(1);
            if more(idx) && matches!(self.at(idx), Some(b'-' | b'+')) {
                idx = idx.saturating_add(1);
            }
            while digit(idx) {
                idx = idx.saturating_add(1);
            }
            if digit(idx.saturating_sub(1)) {
                is_float = true;
            } else {
                idx = e_start;
            }
        }
        let lit = std::str::from_utf8(self.cx.s.get(start..idx).unwrap_or_default()).unwrap_or("0");
        if is_float {
            self.note_float(depth);
            let f: f64 = lit.parse().unwrap_or(f64::NAN);
            return Ok(Step::Value(self.b.scalar(Scalar::Float(f)), idx));
        }
        let digits = lit.strip_prefix('-').map_or(lit.len(), str::len);
        if digits > 4300 {
            self.cx.call(depth)?;
            return Err(CheckError::Reported(format!(
                "Exceeds the limit (4300 digits) for integer string conversion: value has {digits} digits; \
                 use sys.set_int_max_str_digits() to increase the limit"
            )));
        }
        let repr = if lit == "-0" { "0" } else { lit };
        Ok(Step::Value(self.b.scalar(Scalar::Int(repr.into())), idx))
    }
}

/// A popped frame is always the kind that was on top (checked one line earlier); never reached.
fn unreachable_frame<T>() -> Result<T, CheckError> {
    Err(CheckError::crash(
        "SystemError",
        "json scanner frame mismatch",
    ))
}

// --------------------------------------------------------------------------------------------------------------
// check-json
// --------------------------------------------------------------------------------------------------------------

/// `check_json.py`: no values; with `hook`, keys checked by `raise_duplicate_keys` at every object close (v4.0.0+),
/// without it the plain `json.load(f)` of v3.x (no Python call per object).
struct CheckBuild {
    hook: bool,
}

impl Build for CheckBuild {
    type V = ();
    const STRINGS: bool = false;

    fn scalar(&mut self, _s: Scalar) {}

    fn array(&mut self, _items: Vec<()>) {}

    fn object(
        &mut self,
        cx: &Ctx<'_>,
        pairs: Vec<(Box<[u8]>, ())>,
        depth: usize,
    ) -> Result<(), CheckError> {
        if !self.hook {
            return Ok(());
        }
        cx.hook(depth)?;
        let mut seen = std::collections::HashSet::new();
        for (key, ()) in &pairs {
            if !seen.insert(&**key) {
                if depth > cx.lim.hook_raise {
                    let text = if depth == cx.lim.hook_raise.saturating_add(1) {
                        limits::CALLING
                    } else {
                        limits::PLAIN
                    };
                    return Err(CheckError::crash("RecursionError", text));
                }
                let mut msg = b"Duplicate key: ".to_vec();
                msg.extend_from_slice(key);
                return Err(match String::from_utf8(msg) {
                    Ok(m) => CheckError::Reported(m),
                    Err(e) => CheckError::crash(
                        "UnicodeEncodeError",
                        pystr::encode_error(&e.into_bytes(), 0).unwrap_or_default(),
                    ),
                });
            }
        }
        Ok(())
    }
}

/// [`super::check_ex`] on decoded text.
pub(super) fn check(text: &[u8], v: PyMinor) -> Result<(), CheckError> {
    Scanner::new(text, v, Path::Check, CheckBuild { hook: true })
        .parse()
        .map(|_| ())
}

/// [`super::check_v3`] on decoded text.
pub(super) fn check_v3(text: &[u8], v: PyMinor) -> Result<(), CheckError> {
    Scanner::new(text, v, Path::CheckV3, CheckBuild { hook: false })
        .parse()
        .map(|_| ())
}

// --------------------------------------------------------------------------------------------------------------
// loads / pretty-format-json
// --------------------------------------------------------------------------------------------------------------

/// Builds Python objects; `pairs_first` set = pretty-format-json's hook.
struct ValBuild<'o> {
    pretty: Option<&'o DumpOpts>,
}

impl Build for ValBuild<'_> {
    type V = Val;
    const STRINGS: bool = true;

    fn scalar(&mut self, s: Scalar) -> Val {
        match s {
            Scalar::Null => Val::Null,
            Scalar::Bool(b) => Val::Bool(b),
            Scalar::Int(i) => Val::Int(i),
            Scalar::Float(f) => Val::Float(f),
            Scalar::Str(b) => Val::Str(b.into_boxed_slice()),
        }
    }

    fn array(&mut self, items: Vec<Val>) -> Val {
        Val::List(items)
    }

    fn object(
        &mut self,
        cx: &Ctx<'_>,
        pairs: Vec<(Box<[u8]>, Val)>,
        depth: usize,
    ) -> Result<Val, CheckError> {
        cx.hook(depth)?;
        match self.pretty {
            None => Ok(Val::Dict(pairs)),
            Some(o) => value::pairs_first(pairs, o, Some(cx.minor))
                .map(Val::Dict)
                .map_err(|e| CheckError::crash("TypeError", e.text())),
        }
    }
}

/// [`super::loads`] on decoded text.
pub(super) fn loads(text: &[u8], v: PyMinor) -> Result<JsonValue, CheckError> {
    let (val, _) = Scanner::new(text, v, Path::Check, ValBuild { pretty: None }).parse()?;
    Ok(val.into_json())
}

/// `json.loads(contents, object_pairs_hook=pairs_first)` on a `str`, then the encoder's recursion limit.
pub(super) fn pretty(contents: &[u8], o: &DumpOpts, v: PyMinor) -> Result<Val, CheckError> {
    if contents.starts_with("\u{feff}".as_bytes()) {
        return Err(CheckError::Reported(
            "Unexpected UTF-8 BOM (decode using utf-8-sig): line 1 column 1 (char 0)".to_owned(),
        ));
    }
    let (val, sc) =
        Scanner::new(contents, v, Path::Pretty, ValBuild { pretty: Some(o) }).parse()?;
    if let Some(limit) = sc.cx.lim.dump {
        let floats_too_deep = sc
            .max_float_depth
            .is_some_and(|d| d.saturating_add(1) > limit);
        if sc.max_depth > limit || floats_too_deep {
            return Err(CheckError::crash("RecursionError", limits::PLAIN));
        }
    }
    Ok(val)
}
