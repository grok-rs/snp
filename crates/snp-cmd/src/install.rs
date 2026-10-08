//! `install`, `uninstall`, `init-templatedir` (`commands/install_uninstall.py`, `init_templatedir.py`; 11 §A.5).
//!
//! Owner: WP-33.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-33")]
#![expect(unused_variables, reason = "W0 stub parameters, removed by WP-33")]

use snp_cli::{InitTemplatedirArgs, InstallArgs, UninstallArgs};
use snp_core::failure::Failure;

use crate::CmdCtx;

/// Writes the scripts only. `install --install-hooks` is sequenced by the binary: `snp_cmd::install(..)?` then
/// `snp_run::install_hooks(InstallHooksArgs { color, config, with_fallback: false }, &run_cx)` — pre-commit's
/// order; the first failure is returned (ADR-070).
pub fn install(a: &InstallArgs, cx: &CmdCtx<'_>) -> Result<i32, Failure> {
    todo!()
}

/// `pre-commit uninstall`.
pub fn uninstall(a: &UninstallArgs, cx: &CmdCtx<'_>) -> Result<i32, Failure> {
    todo!()
}

/// `pre-commit init-templatedir`.
pub fn init_templatedir(a: &InitTemplatedirArgs, cx: &CmdCtx<'_>) -> Result<i32, Failure> {
    todo!()
}
