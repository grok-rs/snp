//! Python `re` compile errors and warnings, and match-time errors.
//!
//! Owner: WP-13.

use std::fmt;

use crate::messages;

/// Why a pattern does not compile, as CPython 3.12 reports it (`re.compile`, 01 Addendum §A.4).
///
/// `Display` is Python's `str(exc)`; [`PyReError::py_last_line`] is the last line of the traceback that
/// `python -m pre_commit.languages.pygrep` prints (§A.4.5).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum PyReError {
    /// `re.error` (`re/_constants.py:23-52`).
    ///
    /// `Display` = `str(e)`: `"{e.msg} at position {e.pos}"`, plus `" (line L, column C)"` when the pattern
    /// contains a newline, or the bare message for compile-phase errors (`pos` is `None`).
    Re {
        /// `e.msg`. When the pattern contains a newline and `pos` is set, the text also carries the
        /// `" (line L, column C)"` suffix that Python appends to `str(e)` after the position (the variant has
        /// no other field for it); `Display` moves it behind `at position N`. Bytes patterns carry the
        /// `backslashreplace` form Python uses (`re/_parser.py:301`).
        msg: Box<str>,
        /// `e.pos`: characters for str patterns, bytes for bytes patterns; `None` for compile-phase errors.
        pos: Option<usize>,
    },
    /// `OverflowError: the repetition number is too large` (a repeat bound ≥ 4294967295, `re/_parser.py:671`).
    Overflow,
    /// `ValueError` from `fix_flags` (`re/_parser.py:953-967`): "ASCII and UNICODE flags are incompatible", ….
    Value(Box<str>),
    /// `RecursionError: maximum recursion depth exceeded` (nesting ≥ 485 groups; pre-commit accepts ≤ 484).
    Recursion,
    /// Valid Python, beyond snp's engines (01 Addendum §A.6.13 D3/D5/D6); the text says which limit.
    Unsupported(Box<str>),
}

impl PyReError {
    /// The Python class: `re.error` | `OverflowError` | `ValueError` | `RecursionError`; `snp` for
    /// [`PyReError::Unsupported`], which has no Python counterpart.
    pub fn py_type(&self) -> &'static str {
        match self {
            Self::Re { .. } => "re.error",
            Self::Overflow => "OverflowError",
            Self::Value(_) => "ValueError",
            Self::Recursion => "RecursionError",
            Self::Unsupported(_) => "snp",
        }
    }

    /// The last traceback line pygrep prints for an invalid entry (01 Addendum §A.4.5, D-MISC-05):
    /// `re.error: missing ), unterminated subpattern at position 0`, `OverflowError: the repetition number is
    /// too large`, ….
    pub fn py_last_line(&self) -> String {
        format!("{}: {self}", self.py_type())
    }

    /// A `re.error` at `pos` of `pattern` (units: chars for str, bytes for bytes), with the line/column
    /// suffix of `re/_constants.py:44-50` folded into `msg` when the pattern has a newline.
    pub(crate) fn at(msg: String, pos: usize, units: &[u32]) -> Self {
        let newline = u32::from('\n');
        if !units.contains(&newline) {
            return Self::Re {
                msg: msg.into_boxed_str(),
                pos: Some(pos),
            };
        }
        let before = units.get(..pos).unwrap_or(units);
        let line = before
            .iter()
            .filter(|&&u| u == newline)
            .count()
            .saturating_add(1);
        let col = before
            .iter()
            .rposition(|&u| u == newline)
            .map_or_else(|| pos.saturating_add(1), |nl| pos.saturating_sub(nl));
        Self::Re {
            msg: format!("{msg} (line {line}, column {col})").into_boxed_str(),
            pos: Some(pos),
        }
    }

    /// A compile-phase `re.error` (no position).
    pub(crate) fn bare(msg: &str) -> Self {
        Self::Re {
            msg: msg.into(),
            pos: None,
        }
    }
}

/// Splits `msg` into Python's `e.msg` and the `(line, column)` suffix folded into it by [`PyReError::at`].
fn split_line_col(msg: &str) -> (&str, Option<(&str, &str)>) {
    let Some(head) = msg.strip_suffix(')') else {
        return (msg, None);
    };
    let Some(open) = head.rfind(" (line ") else {
        return (msg, None);
    };
    let (base, rest) = (head.get(..open), head.get(open.saturating_add(7)..));
    let (Some(base), Some(rest)) = (base, rest) else {
        return (msg, None);
    };
    let Some((line, col)) = rest.split_once(", column ") else {
        return (msg, None);
    };
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    if digits(line) && digits(col) {
        (base, Some((line, col)))
    } else {
        (msg, None)
    }
}

impl fmt::Display for PyReError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Re {
                msg,
                pos: Some(pos),
            } => {
                let (base, lc) = split_line_col(msg);
                write!(f, "{base} at position {pos}")?;
                if let Some((line, col)) = lc {
                    write!(f, " (line {line}, column {col})")?;
                }
                Ok(())
            }
            Self::Re { msg, pos: None } | Self::Value(msg) | Self::Unsupported(msg) => {
                f.write_str(msg)
            }
            Self::Overflow => f.write_str(messages::OVERFLOW),
            Self::Recursion => f.write_str(messages::RECURSION),
        }
    }
}

impl std::error::Error for PyReError {}

/// A `FutureWarning` emitted while compiling (`re/_parser.py:556-606`): "Possible nested set at position 1".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PyWarning {
    /// The warning text (positions in characters for str patterns, bytes for bytes patterns).
    pub msg: Box<str>,
}

/// How a compiled pattern is matched (ARCHITECTURE §3.3 item 4, 01 Addendum §A.6.9, §A.6.12).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum MatchKind {
    /// `''` (or a pattern that parses to nothing): matches everything.
    All,
    /// `^$`: Python's truth is `h == "" or h == "\n"`.
    EmptyOrNewline,
    /// `^lit`: `h.startswith(lit)`.
    Prefix,
    /// `lit$` (`h.endswith(lit) or h.endswith(lit + "\n")`) or `lit\Z` (`h.endswith(lit)`).
    Suffix,
    /// A plain literal: `memchr::memmem`.
    Contains,
    /// The `regex` engine (linear time).
    Tier1,
    /// The `fancy-regex` engine behind the relaxed tier-1 prefilter.
    Tier2,
}

/// Match-time failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum MatchError {
    /// fancy-regex's `backtrack_limit` (10⁸) was exceeded (D-SEC-10). Also reported when a lazily built
    /// engine variant exceeds the engine limits, which compile-time checks make unreachable in practice.
    Backtrack,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn units(s: &str) -> Vec<u32> {
        s.chars().map(u32::from).collect()
    }

    #[test]
    fn display_is_python_str() {
        let e = PyReError::at("missing ), unterminated subpattern".into(), 0, &units("("));
        assert_eq!(
            e.to_string(),
            "missing ), unterminated subpattern at position 0"
        );
        assert_eq!(
            e.py_last_line(),
            "re.error: missing ), unterminated subpattern at position 0"
        );
        let e = PyReError::at(
            "missing ), unterminated subpattern".into(),
            10,
            &units("(?x)\n(a\n  (b"),
        );
        assert_eq!(
            e.to_string(),
            "missing ), unterminated subpattern at position 10 (line 3, column 3)"
        );
        assert_eq!(
            PyReError::Overflow.py_last_line(),
            "OverflowError: the repetition number is too large"
        );
        assert_eq!(
            PyReError::Recursion.py_last_line(),
            "RecursionError: maximum recursion depth exceeded"
        );
        assert_eq!(
            PyReError::Value("ASCII and UNICODE flags are incompatible".into()).py_last_line(),
            "ValueError: ASCII and UNICODE flags are incompatible"
        );
        assert_eq!(
            PyReError::bare(messages::LOOKBEHIND_FIXED_WIDTH).to_string(),
            messages::LOOKBEHIND_FIXED_WIDTH
        );
        assert_eq!(PyReError::Unsupported("x".into()).py_last_line(), "snp: x");
    }

    #[test]
    fn line_col_edge_cases() {
        // pos at the very start of a later line; a newline right before pos.
        let e = PyReError::at("m".into(), 2, &units("a\nb"));
        assert_eq!(e.to_string(), "m at position 2 (line 2, column 1)");
        let e = PyReError::at("m".into(), 1, &units("a\nb"));
        assert_eq!(e.to_string(), "m at position 1 (line 1, column 2)");
        // A message that merely looks like a suffix is never split without digits.
        assert_eq!(
            split_line_col("x (line a, column 2)"),
            ("x (line a, column 2)", None)
        );
    }
}
