//! CPython-3.12 `re` front end (exact grammar and error texts) lowered to `regex` (tier 1) and `fancy-regex`
//! (tier 2) with a relaxed prefilter, fast match kinds and a compile cache (ARCHITECTURE §1.4.5, 01 Addendum §A.6).
//!
//! * [`validate`] / [`cfgv_is_valid`] parse only: they accept exactly what CPython 3.12's `re.compile` accepts
//!   (`re/_parser.py`, `re/_compiler.py`), report Python's exact message and position, and turn Python's
//!   `OverflowError`/`ValueError`/`RecursionError` into [`PyReError`] variants (D-CRASH-04).
//! * [`PyRegex::compile`] lowers the parse tree to explicit, engine-neutral syntax (Python's `\w`, `\s`, `$`,
//!   `\Z`, `\b`, IGNORECASE folding) and picks a cheap [`MatchKind`] or the two-tier engines.
//! * No input panics or exhausts the stack: the parser and the lowering are iterative, deep engines are built
//!   on a helper thread, and tier 2 stops at a backtrack limit ([`MatchError::Backtrack`], D-SEC-10).
//!
//! Owner: WP-13.
#![forbid(unsafe_code)]

mod ast;
mod cache;
mod compile;
mod engine;
mod error;
mod fold;
#[cfg(feature = "py-re-names")]
mod generated;
mod lower;
mod messages;
mod names;
mod options;
mod parse;
mod tok;
mod unicode;

pub use cache::RegexCache;
pub use compile::{PyRegex, cfgv_is_valid, validate};
pub use error::{MatchError, MatchKind, PyReError, PyWarning};
pub use options::{Haystack, Mode, Options};

#[cfg(test)]
mod props;
#[cfg(test)]
mod tests;
