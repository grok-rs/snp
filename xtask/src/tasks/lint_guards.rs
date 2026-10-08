//! `cargo xtask lint-guards`: no `?` and no `return` inside `run`/`hook_impl` of crates/snp-run (§4.4).
//!
//! Owner: WP-00.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-00 (xtask stage)")]
#![expect(
    unused_variables,
    reason = "W0 stub parameters, removed by WP-00 (xtask stage)"
)]

use std::ffi::OsString;

/// Runs the task with the arguments that follow its name.
pub(crate) fn run(args: &[OsString]) -> anyhow::Result<()> {
    todo!()
}
