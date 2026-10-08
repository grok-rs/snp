//! CPython's recursion thresholds for the `json` paths the hooks take (09 §4.C, Q9), measured on the real
//! console scripts by `xtask/oracle/pch_vectors.py` (`limits` in `tests/vectors/json_31x.json`; a test keeps these
//! constants equal to the measurement).
//!
//! A threshold is the largest nesting depth (number of open arrays/objects) at which the event still succeeds:
//! opening a container (`_Py_EnterRecursiveCall` in `scan_once_unicode`), raising `StopIteration` / the int-limit
//! `ValueError` / calling `parse_constant` (one C call), constructing a `JSONDecodeError` (a Python `__init__`),
//! calling the Python `object_pairs_hook`, the hook raising `ValueError`, and, for 3.12's pure-Python indenting
//! encoder, the generator depth of `json.dumps(indent=…)`.
//!
//! Owner: WP-17.

use super::PyMinor;

/// Which hook's call stack the thresholds belong to.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum Path {
    /// `check-json`: `json.load(f, object_pairs_hook=raise_duplicate_keys)`.
    Check,
    /// `pretty-format-json`: `json.loads(contents, object_pairs_hook=pairs_first)` then `json.dumps(indent=…)`.
    Pretty,
}

/// The thresholds of one (path, version).
#[derive(Copy, Clone, Debug)]
pub(super) struct Limits {
    /// Opening a container at this depth still works.
    pub(super) enter: usize,
    /// `StopIteration`, the 4300-digit `ValueError`, `parse_constant`.
    pub(super) stop: usize,
    /// Constructing a `JSONDecodeError`.
    pub(super) errmsg: usize,
    /// Calling the `object_pairs_hook` for an object at this depth.
    pub(super) hook: usize,
    /// `raise_duplicate_keys` raising its `ValueError` (check-json).
    pub(super) hook_raise: usize,
    /// How many depths past `hook` still fail inside the call machinery ("while calling a Python object").
    pub(super) hook_calling: usize,
    /// 3.12's Python encoder: the deepest container (a float needs one more level, `floatstr`).
    pub(super) dump: Option<usize>,
}

impl Limits {
    pub(super) const fn of(path: Path, v: PyMinor) -> Self {
        match (path, v) {
            (Path::Check, PyMinor::P312) => Self {
                enter: 9997,
                stop: 9996,
                errmsg: 9993,
                hook: 9995,
                hook_raise: 9994,
                hook_calling: 0,
                dump: None,
            },
            (Path::Pretty, PyMinor::P312) => Self {
                enter: 9997,
                stop: 9996,
                errmsg: 9993,
                hook: 9993,
                hook_raise: 9993,
                hook_calling: 2,
                dump: Some(994),
            },
            (Path::Check, PyMinor::P313) => Self {
                enter: 9998,
                stop: 9997,
                errmsg: 9994,
                hook: 9996,
                hook_raise: 9995,
                hook_calling: 0,
                dump: None,
            },
            (Path::Pretty, PyMinor::P313) => Self {
                enter: 9998,
                stop: 9997,
                errmsg: 9994,
                hook: 9994,
                hook_raise: 9994,
                hook_calling: 2,
                dump: None,
            },
        }
    }
}

/// The `RecursionError` texts.
pub(super) const CALLING: &str = "maximum recursion depth exceeded while calling a Python object";
/// A plain Python-frame overflow.
pub(super) const PLAIN: &str = "maximum recursion depth exceeded";

/// Text of a failed container entry.
pub(super) const fn entering(object: bool) -> &'static str {
    if object {
        "maximum recursion depth exceeded while decoding a JSON object from a unicode string"
    } else {
        "maximum recursion depth exceeded while decoding a JSON array from a unicode string"
    }
}
