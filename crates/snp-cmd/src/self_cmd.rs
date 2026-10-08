//! `snp self alias install|uninstall|status` and `snp self check` (11 §A.9).
//!
//! Owner: WP-33.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-33")]
#![expect(unused_variables, reason = "W0 stub parameters, removed by WP-33")]

use snp_cli::SelfArgs;
use snp_core::failure::Failure;

use crate::CmdCtx;

/// `snp self …`.
pub fn self_cmd(a: &SelfArgs, cx: &CmdCtx<'_>) -> Result<i32, Failure> {
    todo!()
}
