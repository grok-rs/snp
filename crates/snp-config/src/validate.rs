//! `validate-config` / `validate-manifest` (`clientlib.validate_*_main`): per-file `print(e)` blocks, no git.
//!
//! Owner: WP-21.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-21")]
#![expect(unused_variables, reason = "W0 stub parameters, removed by WP-21")]

use std::ffi::OsStr;

use crate::LoadOpts;

/// Which schema a file is validated against.
#[derive(Copy, Clone, Debug)]
#[non_exhaustive]
pub enum FileKind {
    /// `validate-config`.
    Config,
    /// `validate-manifest`.
    Manifest,
}

/// Validates one file; `Some(text)` = the `print(e)` block of a failure.
pub fn validate_file(
    kind: FileKind,
    path: &OsStr,
    o: &LoadOpts<'_>,
    strict: bool,
) -> Option<String> {
    todo!()
}
