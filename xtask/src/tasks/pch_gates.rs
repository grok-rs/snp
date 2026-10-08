//! `cargo xtask pch-gates`: CI parity gates of the NATIVE-WITH-CAVEATS hooks (09 §9.9).
//!
//! Owner: WP-27.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-27")]
#![expect(unused_variables, reason = "W0 stub parameters, removed by WP-27")]

use std::ffi::OsString;

/// Runs the task with the arguments that follow its name.
pub(crate) fn run(args: &[OsString]) -> anyhow::Result<()> {
    todo!()
}
