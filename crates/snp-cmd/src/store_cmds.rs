//! `gc`, `clean` (`commands/gc.py`, `clean.py`; §5.6) and `snp cache`.
//!
//! Owner: WP-33.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-33")]
#![expect(unused_variables, reason = "W0 stub parameters, removed by WP-33")]

use snp_cli::CacheArgs;
use snp_core::failure::Failure;

use crate::CmdCtx;

/// `pre-commit gc`: `_mark_used_repos` over the tracked configs, then sweep; `{n} repo(s) removed.`.
pub fn gc(cx: &CmdCtx<'_>) -> Result<i32, Failure> {
    todo!()
}

/// `pre-commit clean`: `Cleaned {store}.`.
pub fn clean(cx: &CmdCtx<'_>) -> Result<i32, Failure> {
    todo!()
}

/// `snp cache {dir,size,gc,clean}`.
pub fn cache(a: &CacheArgs, cx: &CmdCtx<'_>) -> Result<i32, Failure> {
    todo!()
}
