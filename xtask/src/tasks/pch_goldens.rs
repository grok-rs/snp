//! `cargo xtask pch-goldens`: per-epoch pre-commit-hooks goldens and the Appendix A help goldens.
//!
//! Owner: WP-26.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-26")]
#![expect(unused_variables, reason = "W0 stub parameters, removed by WP-26")]

use std::ffi::OsString;

use snp_hooks as _;
/// Runs the task with the arguments that follow its name.
pub(crate) fn run(args: &[OsString]) -> anyhow::Result<()> {
    todo!()
}
