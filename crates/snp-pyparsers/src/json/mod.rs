//! CPython `json` decoder/encoder with byte-identical `json.JSONDecodeError` texts (09 §4.C, §9.4;
//! `check_json.py`, `pretty_format_json.py`).
//!
//! The decoder is a port of CPython's C scanner (`Modules/_json.c`: `scan_once_unicode`, `_parse_object_unicode`,
//! `_parse_array_unicode`, `scanstring_unicode`, `_match_number_unicode`) plus `json.loads`' `detect_encoding` and
//! `surrogatepass` decoding, run as an explicit-stack state machine. Positions in messages are code-point indices
//! into the decoded text, as in Python. CPython's recursion limits are reproduced per emulated version and per
//! event (opening a container, raising an error, calling the `object_pairs_hook`) from thresholds measured on the
//! real console scripts (`tests/vectors/json_31x.json` `limits`); past them the result is the `RecursionError`
//! crash.
//!
//! Owner: WP-17.

mod encode;
mod limits;
mod scan;
mod value;

use std::borrow::Cow;

use crate::CheckError;
use crate::pystr;

/// The CPython minor version whose `json` module is reproduced (3.12 and 3.13 differ in messages).
#[derive(Copy, Clone, Debug)]
#[non_exhaustive]
pub enum PyMinor {
    /// CPython 3.12.
    P312,
    /// CPython 3.13.
    P313,
}

/// A decoded JSON document (`json.loads(…, object_pairs_hook=…)`).
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum JsonValue {
    /// `null`.
    Null,
    /// `true` / `false`.
    Bool(bool),
    /// An integer: the decimal `repr` of the Python `int` (the literal, with `-0` normalised to `0`; unbounded).
    Int(Box<str>),
    /// A float (`NaN`, `Infinity` and out-of-range literals included).
    Float(f64),
    /// A string. A lone surrogate (`"\ud800"`, which Python keeps) cannot live in a Rust `String` and becomes
    /// U+FFFD; [`pretty_format`] keeps them.
    Str(String),
    /// An array.
    Array(Vec<Self>),
    /// An object: order kept; duplicate keys per `object_pairs_hook`.
    Object(Vec<(String, Self)>),
}

/// `check-json`: `json.loads(bytes)`; `Err` is the exact `json.JSONDecodeError` text.
///
/// Reproduces `check_json.py` (v4.0.0+): `json.load(f, object_pairs_hook=raise_duplicate_keys)` on the file's
/// bytes; `Err` is what the hook prints inside `{filename}: Failed to json decode ({…})` (a decode error, the
/// 4300-digit limit, `Duplicate key: {key}` or a `JSONDecodeError`), or, for the crash path, the traceback's last
/// line (see [`check_ex`]).
pub fn check(bytes: &[u8], v: PyMinor) -> Result<(), String> {
    check_ex(bytes, v).map_err(CheckError::into_message)
}

/// [`check`] keeping a crash apart: `Crash` for `RecursionError` (nesting beyond CPython's limits) and for a
/// duplicate key holding a lone surrogate (printing it raises `UnicodeEncodeError`; positions are relative to the
/// message). The v3.x hook (no duplicate check) is [`loads`]' acceptance.
pub fn check_ex(bytes: &[u8], v: PyMinor) -> Result<(), CheckError> {
    let text = decode_bytes(bytes)?;
    scan::check(&text, v)
}

/// `json.loads(bytes, object_pairs_hook=…)`: iterative, CPython's recursion thresholds (those of `check-json`'s
/// `json.load` with a Python `object_pairs_hook`). `Err` as for [`check`]; duplicates are kept in order.
pub fn loads(bytes: &[u8], v: PyMinor) -> Result<JsonValue, String> {
    let text = decode_bytes(bytes).map_err(CheckError::into_message)?;
    scan::loads(&text, v).map_err(CheckError::into_message)
}

/// `pretty-format-json` encoder options.
#[derive(Clone, Debug)]
pub struct DumpOpts {
    /// `--indent` (a number of spaces or a literal string).
    pub indent: String,
    /// `ensure_ascii` (`--no-ensure-ascii` clears it).
    pub ensure_ascii: bool,
    /// `--no-sort-keys` clears it.
    pub sort_keys: bool,
    /// `--top-keys`.
    pub top_keys: Vec<String>,
}

/// `pretty_format_json._get_pretty_format`: `json.dumps(…)` with the hook's options.
///
/// Applies `pairs_first` (top keys first in `top_keys` order, the rest sorted when `sort_keys`, then `dict()`:
/// first position, last value) to every object bottom-up and encodes with `indent` (`parse_num_to_int`: a Python
/// `int` gives that many spaces, ≤ 0 none; anything else is used literally), separators `(',', ': ')` and
/// `ensure_ascii`, plus the trailing `\n`. Sorting duplicate keys compares their values with Python's `<`; where
/// Python raises `TypeError` this infallible variant keeps the original order of those pairs. [`pretty_format`]
/// is the exact hook path (crashes, lone surrogates, recursion limits).
pub fn dumps_pretty(v: &JsonValue, o: &DumpOpts) -> String {
    let indent = match encode::Indent::parse(&o.indent) {
        Ok(i) => i,
        Err(_) => encode::Indent::Str(Cow::Borrowed("")),
    };
    let val = value::Val::from_json(v);
    let val = value::pairs_first_tree(val, o, None);
    let mut out = Vec::new();
    // The tree is finite and the encoder iterative; a byte budget cannot be exceeded by a valid tree here, so the
    // error case (impossible indent sizes) degrades to the compact rendering.
    if encode::dumps(&val, &indent, o.ensure_ascii, &mut out).is_err() {
        out.clear();
        let _ignored = encode::dumps(
            &val,
            &encode::Indent::Str(Cow::Borrowed("")),
            o.ensure_ascii,
            &mut out,
        );
    }
    out.push(b'\n');
    pystr::to_string_lossy(&out)
}

/// `pretty_format_json._get_pretty_format(contents, indent, ensure_ascii, sort_keys, top_keys)` exactly
/// (ARCHITECTURE §1.4.11 addition): `contents` is the file as the hook reads it (`open(f, encoding='UTF-8')`,
/// universal newlines). `Ok` is the pretty text (with its `\n`); `Err(Reported)` is the `ValueError` the hook turns
/// into `Input File … is not a valid JSON` (its `str()` is not printed); `Err(Crash)` is `TypeError` (duplicate keys
/// whose values Python cannot order), `RecursionError` (parse or encoder limits of `v`), `MemoryError` (an absurd
/// `--indent`), or `UnicodeEncodeError` for a lone surrogate kept by `ensure_ascii=False` (the hook writes or
/// diffs the text later, after printing `Fixing file …` with `--autofix`; the position is the index in the
/// pretty text).
pub fn pretty_format(contents: &str, o: &DumpOpts, v: PyMinor) -> Result<String, CheckError> {
    let indent = encode::Indent::parse(&o.indent)?;
    let tree = scan::pretty(contents.as_bytes(), o, v)?;
    let mut out = Vec::new();
    encode::dumps(&tree, &indent, o.ensure_ascii, &mut out)?;
    out.push(b'\n');
    match String::from_utf8(out) {
        Ok(s) => Ok(s),
        Err(e) => {
            let bytes = e.into_bytes();
            Err(CheckError::crash(
                "UnicodeEncodeError",
                pystr::encode_error(&bytes, 0).unwrap_or_default(),
            ))
        }
    }
}

/// `json.detect_encoding(b)` and `b.decode(encoding, 'surrogatepass')`: the decoded text in generalized UTF-8; a
/// `UnicodeDecodeError` (a `ValueError`, so the hook reports it).
fn decode_bytes(b: &[u8]) -> Result<Cow<'_, [u8]>, CheckError> {
    let reported = |e: pystr::DecodeError| CheckError::Reported(e.to_string());
    let starts = |p: &[u8]| b.starts_with(p);
    if starts(b"\x00\x00\xfe\xff") {
        return pystr::decode_utf32_surrogatepass(b, 4, true)
            .map(Cow::Owned)
            .map_err(reported);
    }
    if starts(b"\xff\xfe\x00\x00") {
        return pystr::decode_utf32_surrogatepass(b, 4, false)
            .map(Cow::Owned)
            .map_err(reported);
    }
    if starts(b"\xfe\xff") {
        return pystr::decode_utf16_surrogatepass(b, 2, true)
            .map(Cow::Owned)
            .map_err(reported);
    }
    if starts(b"\xff\xfe") {
        return pystr::decode_utf16_surrogatepass(b, 2, false)
            .map(Cow::Owned)
            .map_err(reported);
    }
    if let Some(rest) = b.strip_prefix(b"\xef\xbb\xbf") {
        // utf-8-sig decodes the rest on its own: error positions are relative to it, the codec name is 'utf-8'.
        return pystr::decode_utf8_surrogatepass(rest).map_err(reported);
    }
    let enc = match b {
        [0, b1, _, _, ..] => {
            if *b1 != 0 {
                Enc::Utf16Be
            } else {
                Enc::Utf32Be
            }
        }
        [_, 0, b2, b3, ..] => {
            if *b2 != 0 || *b3 != 0 {
                Enc::Utf16Le
            } else {
                Enc::Utf32Le
            }
        }
        [0, _] => Enc::Utf16Be,
        [_, 0] => Enc::Utf16Le,
        _ => Enc::Utf8,
    };
    match enc {
        Enc::Utf8 => pystr::decode_utf8_surrogatepass(b).map_err(reported),
        Enc::Utf16Be => pystr::decode_utf16_surrogatepass(b, 0, true)
            .map(Cow::Owned)
            .map_err(reported),
        Enc::Utf16Le => pystr::decode_utf16_surrogatepass(b, 0, false)
            .map(Cow::Owned)
            .map_err(reported),
        Enc::Utf32Be => pystr::decode_utf32_surrogatepass(b, 0, true)
            .map(Cow::Owned)
            .map_err(reported),
        Enc::Utf32Le => pystr::decode_utf32_surrogatepass(b, 0, false)
            .map(Cow::Owned)
            .map_err(reported),
    }
}

/// `detect_encoding`'s BOM-less answers.
#[derive(Copy, Clone, Debug)]
enum Enc {
    Utf8,
    Utf16Be,
    Utf16Le,
    Utf32Be,
    Utf32Le,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalogue_09() {
        let v = PyMinor::P312;
        for (input, want) in [
            ("", "Expecting value: line 1 column 1 (char 0)"),
            (" \n ", "Expecting value: line 2 column 2 (char 3)"),
            ("{\"a\": 1} {}", "Extra data: line 1 column 10 (char 9)"),
            (
                "{\"a\"  1}",
                "Expecting ':' delimiter: line 1 column 7 (char 6)",
            ),
            (
                "{\"a\": 1,}",
                "Expecting property name enclosed in double quotes: line 1 column 9 (char 8)",
            ),
            ("[1,]", "Expecting value: line 1 column 4 (char 3)"),
            (
                "\"\\u12\"",
                "Invalid \\uXXXX escape: line 1 column 3 (char 2)",
            ),
            (
                "{\"a\": {\"b\": 1, \"b\": 2}, \"a\": 3}",
                "Duplicate key: b",
            ),
        ] {
            assert_eq!(check(input.as_bytes(), v).unwrap_err(), want, "{input:?}");
        }
        assert_eq!(
            check(b"{\"a\": 1,}", PyMinor::P313).unwrap_err(),
            "Illegal trailing comma before end of object: line 1 column 8 (char 7)"
        );
        assert!(check(b"[NaN, Infinity, -Infinity, 1e999]", v).is_ok());
    }
}
