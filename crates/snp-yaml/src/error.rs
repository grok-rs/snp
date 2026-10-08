//! `MarkedYAMLError` / `ReaderError` texts of PyYAML's C loader.
//!
//! Owner: WP-14.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-14")]
#![expect(unused_variables, reason = "W0 stub parameters, removed by WP-14")]

use std::fmt;

/// A load error; Display == `str(MarkedYAMLError)` of the C loader, marks in `"<unicode string>"`.
#[derive(Debug, Clone)]
pub struct YamlError {
    /// W0 placeholder; WP-14 stores context, `context_mark`, problem, `problem_mark`, note | reader position.
    _priv: (),
}

impl fmt::Display for YamlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        todo!()
    }
}

/// A position in the text (PyYAML `Mark`; 0-based line and column).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Mark {
    /// Character index.
    pub index: usize,
    /// 0-based line.
    pub line: usize,
    /// 0-based column.
    pub column: usize,
}
