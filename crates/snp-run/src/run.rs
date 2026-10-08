//! `run` (`commands/run.py`; §3.1). `run()` contains no `?` and no early `return` (§4.4, `xtask lint-guards`).
//!
//! Owner: WP-32.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-32")]
#![expect(
    unused_variables,
    clippy::needless_pass_by_value,
    reason = "W0 stub parameters, removed by WP-32"
)]

use snp_cli::RunArgs;
use snp_core::failure::Failure;

use crate::RunCtx;

/// `pre-commit run`: single exit (§4.4).
pub fn run(a: RunArgs, cx: &RunCtx) -> Result<i32, Failure> {
    todo!()
}
