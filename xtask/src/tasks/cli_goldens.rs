//! `cargo xtask cli-goldens`: argparse outcome and help goldens from the oracles (12 §A.16).
//!
//! Owner: WP-12.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-12")]
#![expect(unused_variables, reason = "W0 stub parameters, removed by WP-12")]

use std::ffi::OsString;

use snp_argparse as _;
use snp_cli as _;

/// Runs the task with the arguments that follow its name.
pub(crate) fn run(args: &[OsString]) -> anyhow::Result<()> {
    todo!()
}
