//! Compile cache keyed by (source, [`Options`]).
//!
//! Owner: WP-13.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-13")]
#![expect(unused_variables, reason = "W0 stub parameters, removed by WP-13")]

use std::sync::Arc;

use snp_core as _;
use snp_pycompat as _;

use crate::{Options, PyReError, PyRegex};

/// Process-wide compile cache (`re._cache` equivalent, unbounded).
#[derive(Debug, Default)]
pub struct RegexCache {
    /// W0 placeholder; WP-13 stores `Mutex<IndexMap<(Box<str>, Options), Arc<PyRegex>>>`.
    _priv: (),
}

impl RegexCache {
    /// An empty cache.
    pub fn new() -> Self {
        todo!()
    }

    /// The cached compile of `src` under `opts`, compiling on a miss.
    pub fn get(&self, src: &str, opts: Options) -> Result<Arc<PyRegex>, PyReError> {
        todo!()
    }
}
