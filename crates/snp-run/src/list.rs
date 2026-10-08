//! `snp list` (snp brand, §9.3).
//!
//! Owner: WP-32.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-32")]
#![expect(
    unused_variables,
    clippy::needless_pass_by_value,
    reason = "W0 stub parameters, removed by WP-32"
)]

use snp_cli::ListArgs;
use snp_core::failure::Failure;

use crate::RunCtx;

/// Lists the configured hooks (text or `--json`).
pub fn list(a: ListArgs, cx: &RunCtx) -> Result<i32, Failure> {
    todo!()
}
