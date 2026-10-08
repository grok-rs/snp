//! Parsing, post-parse checks, lowering and the compiled [`PyRegex`] (01 Addendum §A.6.2–§A.6.12).
//!
//! Owner: WP-13.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-13")]
#![expect(unused_variables, reason = "W0 stub parameters, removed by WP-13")]

use crate::{MatchError, MatchKind, Mode, Options, PyReError, PyWarning};

/// Parse + post-parse checks only; never builds an engine (validate-config, config load).
pub fn validate(src: &str, mode: Mode) -> Result<Vec<PyWarning>, PyReError> {
    todo!()
}

/// For cfgv 0.3 `CheckFn::check_regex_with` (bool callback, ADR-003): `validate(src, Mode::Str).is_ok()`. Every
/// rejection — including Overflow/Value/Recursion — becomes cfgv's "is not a valid python regex" leaf (D-CRASH-04);
/// `check_regex_try_with` (which could reproduce the exit-3 crash) is deliberately unused (§1.5).
pub fn cfgv_is_valid(src: &str) -> bool {
    todo!()
}

/// A compiled Python pattern; engines are built lazily on first match.
#[derive(Debug)]
pub struct PyRegex {
    /// W0 placeholder; WP-13 stores src, opts, kind, fast: `Option<regex>`, exact: `OnceLock<fancy>`, relaxed, cond.
    _priv: (),
}

impl PyRegex {
    /// `re.compile(src, flags)`; engines built lazily on first match.
    pub fn compile(src: &str, opts: Options) -> Result<Self, PyReError> {
        todo!()
    }

    /// The pattern source.
    pub fn source(&self) -> &str {
        todo!()
    }

    /// The selected match kind.
    pub fn kind(&self) -> MatchKind {
        todo!()
    }

    /// `FutureWarning`s raised by the compile, in order.
    pub fn warnings(&self) -> &[PyWarning] {
        todo!()
    }

    /// `re.search(...) is not None`.
    pub fn search(&self, h: &str) -> Result<bool, MatchError> {
        todo!()
    }

    /// `re.match` → `\A(?:P)`.
    pub fn match_prefix(&self, h: &str) -> Result<bool, MatchError> {
        todo!()
    }

    /// `re.fullmatch` → `\A(?:P)\z`.
    pub fn fullmatch(&self, h: &str) -> Result<bool, MatchError> {
        todo!()
    }

    /// `re.search` over bytes with the match span (pygrep needs the span).
    pub fn search_bytes(&self, h: &[u8]) -> Result<Option<(usize, usize)>, MatchError> {
        todo!()
    }
}
