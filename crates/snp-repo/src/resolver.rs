//! The real resolver and the `doctor --fast-path` decision table.
//!
//! Owner: WP-31.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-31")]
#![expect(unused_variables, reason = "W0 stub parameters, removed by WP-31")]

use std::sync::Arc;

use snp_config::Config;
use snp_core::env::Env;
use snp_core::failure::Failure;
use snp_lang::RunPlan;

use crate::{Resolve, ResolveCtx, ResolvedHook};

/// The store-backed [`Resolve`] implementation.
#[derive(Debug, Default)]
pub struct StoreResolver {
    /// W0 placeholder; WP-31 stores the env cache and the decision log.
    _priv: (),
}

impl Resolve for StoreResolver {
    fn all_hooks(&self, cx: &ResolveCtx<'_>) -> Result<Vec<ResolvedHook>, Failure> {
        todo!()
    }

    fn install_envs(&self, hooks: &[&ResolvedHook], cx: &ResolveCtx<'_>) -> Result<(), Failure> {
        todo!()
    }

    fn hook_env(&self, h: &ResolvedHook, cx: &ResolveCtx<'_>) -> Result<Env, Failure> {
        todo!()
    }

    fn plan(&self, h: &ResolvedHook, cx: &ResolveCtx<'_>, color: bool) -> Result<RunPlan, Failure> {
        todo!()
    }
}

/// `(hook id, decision, reason)` rows; wrapped by `snp_run::doctor_fast_path`.
pub fn decision_table(cfg: &Config, cx: &ResolveCtx<'_>) -> Vec<(Arc<str>, Arc<str>, String)> {
    todo!()
}
