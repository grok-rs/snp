//! `install-hooks` (`commands/install_uninstall.py:install_hooks`).
//!
//! Owner: WP-32.

use snp_cli::InstallHooksArgs;
use snp_core::failure::Failure;

use crate::RunCtx;
use crate::common::{self, Buffered, ResolveParts};

/// `install_hook_envs(all_hooks(load_config(config), store), store)`; `--with-fallback` also provisions delegate
/// environments.
///
/// pre-commit's `install_hooks` (`install_uninstall.py:158-159`) after `_adjust_args_and_chdir`: every hook of the
/// config, whatever its stage, with the shared store lock held while remote repos are in use; returns 0.
#[expect(
    clippy::needless_pass_by_value,
    reason = "frozen signature (ARCHITECTURE §1.4.21)"
)]
pub fn install_hooks(a: InstallHooksArgs, cx: &RunCtx) -> Result<i32, Failure> {
    let (git, info) = common::discover(cx)?;
    let root = info.root;
    let config = common::resolve_config(cx, &a.config, &root, true)?;
    let src = common::config_source(&config, &root);
    common::mark_config_used(&cx.store, &src.path, &root);
    let buffered = Buffered::default();
    let loaded = common::load_config_with(cx, &src, &buffered.warn, &buffered.diag);
    buffered.replay(&cx.printer);
    let mut cfg = loaded?;
    let overlay = common::apply_overlay(cx, &mut cfg, &git, false)?;
    let _guard = if common::uses_store(&cfg) {
        Some(common::store_lock(cx, &cx.store)?)
    } else {
        None
    };
    let parts = ResolveParts::new(cx.env.clone(), root, &cx.store);
    let policy = common::fast_path_policy(&cfg, &cx.env, false, a.with_fallback);
    let rcx = parts.ctx(
        cx,
        &cfg,
        &cx.printer,
        &*cx.printer,
        policy,
        overlay.as_ref(),
        &cx.cancel,
    );
    let hooks = cx.resolver.all_hooks(&rcx)?;
    let refs: Vec<&snp_repo::ResolvedHook> = hooks.iter().collect();
    cx.resolver.install_envs(&refs, &rcx)?;
    Ok(0)
}
