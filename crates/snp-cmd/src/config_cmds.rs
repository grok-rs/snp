//! `validate-config`, `validate-manifest`, `migrate-config`, `sample-config` and `snp config dump|schema`.
//!
//! Owner: WP-33.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-33")]
#![expect(unused_variables, reason = "W0 stub parameters, removed by WP-33")]

use snp_cli::{ConfigArgs, ConfigCmd, ValidateArgs};
use snp_core::failure::Failure;

use crate::CmdCtx;

/// `pre-commit validate-config` (`clientlib.validate_config_main`).
pub fn validate_config(a: &ValidateArgs, cx: &CmdCtx<'_>) -> Result<i32, Failure> {
    todo!()
}

/// `pre-commit validate-manifest` (`clientlib.validate_manifest_main`).
pub fn validate_manifest(a: &ValidateArgs, cx: &CmdCtx<'_>) -> Result<i32, Failure> {
    todo!()
}

/// `pre-commit migrate-config`.
pub fn migrate_config(a: &ConfigArgs, cx: &CmdCtx<'_>) -> Result<i32, Failure> {
    todo!()
}

/// `pre-commit sample-config`.
pub fn sample_config(cx: &CmdCtx<'_>) -> Result<i32, Failure> {
    todo!()
}

/// `snp config dump|schema`.
pub fn config(a: &ConfigCmd, cx: &CmdCtx<'_>) -> Result<i32, Failure> {
    todo!()
}
