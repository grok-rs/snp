//! `hook-impl` (`commands/hook_impl.py`; §3.10). `hook_impl()` contains no `?` and no early `return` (§4.4,
//! `xtask lint-guards`).
//!
//! Owner: WP-32.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-32")]
#![expect(
    unused_variables,
    clippy::needless_pass_by_value,
    reason = "W0 stub parameters, removed by WP-32"
)]

use std::io;

use snp_cli::HookImplArgs;
use snp_core::failure::Failure;

use crate::RunCtx;

/// `pre-commit hook-impl`: the git hook's entry (legacy hook, then `run`).
pub fn hook_impl(a: HookImplArgs, stdin: &mut dyn io::Read, cx: &RunCtx) -> Result<i32, Failure> {
    todo!()
}
