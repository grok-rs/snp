//! `cargo xtask docs`: build the user documentation (`docs/user`).
//!
//! Owner: WP-37.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-37")]
#![expect(unused_variables, reason = "W0 stub parameters, removed by WP-37")]

use std::ffi::OsString;

/// Runs the task with the arguments that follow its name.
pub(crate) fn run(args: &[OsString]) -> anyhow::Result<()> {
    todo!()
}
