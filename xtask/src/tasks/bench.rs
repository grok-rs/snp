//! `cargo xtask bench`: `bench setup|run|gate [--micro]`: hyperfine scenarios B1–B12, history and gates (ARCHITECTURE §12.3).
//!
//! Owner: WP-36.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-36")]
#![expect(unused_variables, reason = "W0 stub parameters, removed by WP-36")]

use std::ffi::OsString;

/// Runs the task with the arguments that follow its name.
pub(crate) fn run(args: &[OsString]) -> anyhow::Result<()> {
    todo!()
}
