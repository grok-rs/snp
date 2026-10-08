//! `cargo xtask check-dag`: enforce the crate DAG of ARCHITECTURE §1.2 (normal, build and dev edges; xtask edges).
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
