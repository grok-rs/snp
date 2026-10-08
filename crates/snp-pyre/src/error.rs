//! Python `re` compile errors and warnings, and match-time errors.
//!
//! Owner: WP-13.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-13")]
#![expect(unused_variables, reason = "W0 stub parameters, removed by WP-13")]

use std::fmt;

/// Why a pattern does not compile, as Python reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum PyReError {
    /// == Python `re.error`; Display = `str(e)` incl. " (line L, column C)".
    Re {
        /// `e.msg`.
        msg: Box<str>,
        /// `e.pos` (chars for str patterns, bytes for bytes patterns).
        pos: Option<usize>,
    },
    /// `OverflowError`: "the repetition number is too large".
    Overflow,
    /// `ValueError`: "ASCII and UNICODE flags are incompatible", ….
    Value(Box<str>),
    /// `RecursionError`: nesting ≥ 485 groups (pre-commit accepts ≤ 484).
    Recursion,
    /// Valid Python, beyond snp's engines (§A.6.13 D3/D5/D6).
    Unsupported(Box<str>),
}

impl PyReError {
    /// The Python class: `re.error` | `OverflowError` | `ValueError` | `RecursionError`.
    pub fn py_type(&self) -> &'static str {
        todo!()
    }

    /// "re.error: missing ), unterminated subpattern at position 0" (pygrep, §A.4.5).
    pub fn py_last_line(&self) -> String {
        todo!()
    }
}

impl fmt::Display for PyReError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        todo!()
    }
}

/// A `FutureWarning` emitted while compiling ("Possible nested set at position 1").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PyWarning {
    /// The warning text.
    pub msg: Box<str>,
}

/// How a compiled pattern is matched (ARCHITECTURE §3.3 item 4, §A.6.9).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum MatchKind {
    /// `''`: matches everything.
    All,
    /// `^$`: the empty string or `"\n"`.
    EmptyOrNewline,
    /// Literal prefix.
    Prefix,
    /// Literal suffix with Python `$` semantics.
    Suffix,
    /// memmem literal.
    Contains,
    /// `regex` engine.
    Tier1,
    /// `fancy-regex` engine.
    Tier2,
}

/// Match-time failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum MatchError {
    /// fancy `backtrack_limit` (1e8) exceeded.
    Backtrack,
}
