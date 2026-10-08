//! `install-hooks` (`commands/install_uninstall.py:install_hooks`).
//!
//! Owner: WP-32.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-32")]
#![expect(
    unused_variables,
    clippy::needless_pass_by_value,
    reason = "W0 stub parameters, removed by WP-32"
)]

use snp_cli::InstallHooksArgs;
use snp_core::failure::Failure;

use crate::RunCtx;

/// `install_hook_envs(all_hooks(config), store)`; `--with-fallback` also provisions delegate environments.
pub fn install_hooks(a: InstallHooksArgs, cx: &RunCtx) -> Result<i32, Failure> {
    todo!()
}
