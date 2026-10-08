//! `cargo xtask check-unsafe`: every crate forbids `unsafe_code` (§11.6).
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
