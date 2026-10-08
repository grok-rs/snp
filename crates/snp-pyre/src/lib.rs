//! CPython-3.12 `re` front end (exact grammar and error texts) lowered to `regex` (tier 1) and `fancy-regex`
//! (tier 2) with a relaxed prefilter, fast match kinds and a compile cache (ARCHITECTURE §1.4.5, 01 Addendum §A.6).
//!
//! Owner: WP-13.

mod cache;
mod compile;
mod error;
mod options;

pub use cache::RegexCache;
pub use compile::{PyRegex, cfgv_is_valid, validate};
pub use error::{MatchError, MatchKind, PyReError, PyWarning};
pub use options::{Haystack, Mode, Options};
