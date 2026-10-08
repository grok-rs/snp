//! `main.py` argument handling: argparse over the brand's spec, then the typed [`crate::Cli`] facade (12 §A.5).
//!
//! Owner: WP-12.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-12")]
#![expect(unused_variables, reason = "W0 stub parameters, removed by WP-12")]

use std::ffi::OsString;

use snp_core::{brand::Brand, env::Env};

use crate::CliOutcome;

/// argv WITHOUT `argv[0]`; `[]` ⇒ `["run"]`; `help [cmd]` re-parses `[cmd, --help]` (12 §A.5). Never touches disk.
pub fn parse(
    brand: Brand,
    argv: &[OsString],
    env: &Env,
    stdout_tty_cols: Option<usize>,
) -> CliOutcome {
    todo!()
}
