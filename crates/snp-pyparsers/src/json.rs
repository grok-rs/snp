//! CPython `json` decoder/encoder with byte-identical `json.JSONDecodeError` texts (09 §4.C, §9.4;
//! `check_json.py`, `pretty_format_json.py`).
//!
//! Owner: WP-17.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-17")]
#![expect(unused_variables, reason = "W0 stub parameters, removed by WP-17")]

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
    /// An integer literal, kept verbatim (Python `int` is unbounded).
    Int(Box<str>),
    /// A float.
    Float(f64),
    /// A string.
    Str(String),
    /// An array.
    Array(Vec<Self>),
    /// An object: order kept; duplicate keys per `object_pairs_hook`.
    Object(Vec<(String, Self)>),
}

/// `check-json`: `json.loads(bytes)`; `Err` is the exact `json.JSONDecodeError` text.
pub fn check(bytes: &[u8], v: PyMinor) -> Result<(), String> {
    todo!()
}

/// `json.loads(bytes, object_pairs_hook=…)`: iterative, CPython's recursion thresholds.
pub fn loads(bytes: &[u8], v: PyMinor) -> Result<JsonValue, String> {
    todo!()
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
pub fn dumps_pretty(v: &JsonValue, o: &DumpOpts) -> String {
    todo!()
}
