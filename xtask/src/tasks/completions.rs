//! `cargo xtask completions`: shell completion scripts generated from `snp_cli::SNP_SPEC`.
//!
//! Owner: WP-12.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-12")]
#![expect(unused_variables, reason = "W0 stub parameters, removed by WP-12")]

use std::ffi::OsString;

use clap as _;
use clap_complete as _;
use clap_mangen as _;
use snp_argparse as _;

/// Runs the task with the arguments that follow its name.
pub(crate) fn run(args: &[OsString]) -> anyhow::Result<()> {
    todo!()
}
