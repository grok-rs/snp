//! `cargo xtask divergence-book`: the DIVERGENCES book generated from `docs/divergences/*.md`.
//!
//! Owner: WP-35.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-35")]
#![expect(unused_variables, reason = "W0 stub parameters, removed by WP-35")]

use std::ffi::OsString;

/// Runs the task with the arguments that follow its name.
pub(crate) fn run(args: &[OsString]) -> anyhow::Result<()> {
    todo!()
}
