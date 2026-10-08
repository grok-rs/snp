//! `lang_base.hook_cmd` with the hazmat rewrite (02 §11.3, 03 H.8.1).
//!
//! Owner: WP-15.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-15")]
#![expect(unused_variables, reason = "W0 stub parameters, removed by WP-15")]

use std::ffi::OsString;
use std::path::Path;
use std::sync::Arc;

use snp_pycompat::shlex::ShlexError;

/// Why an `entry` cannot become an argv.
#[derive(Debug)]
#[non_exhaustive]
pub enum HookCmdError {
    /// `shlex.split` raised.
    Shlex(ShlexError),
    /// The entry splits to nothing.
    Empty,
}

/// `shlex.split(entry)`; "pre-commit hazmat …"/"snp hazmat …" (first two tokens, any quoting) → [`self_exe`, "__hazmat",
/// …]; then args (02 §11.3, 03 H.8.1). Other `pre-commit …` entries are not rewritten (parity).
pub fn hook_cmd(
    entry: &str,
    args: &[Arc<str>],
    self_exe: &Path,
) -> Result<Vec<OsString>, HookCmdError> {
    todo!()
}
