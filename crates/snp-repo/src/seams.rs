//! The seams snp-run uses (S12) and the G5 preflight seam (§7.2, ADR-070).
//!
//! Owner: WP-31.

use std::fmt::Debug;
use std::sync::Arc;

use snp_core::env::Env;
use snp_core::failure::Failure;
use snp_hooks_data::{Epoch, NativeSource};
use snp_lang::RunPlan;

use crate::{ResolveCtx, ResolvedHook};

/// G5 of §7.2 as a seam (snp-repo cannot depend on snp-hooks): snp-run implements it with
/// `snp_hooks::native(source, id).map(|h| h.preflight(epoch, args))`; `all_hooks` calls it after G1–G4/G6 so the
/// final Native/Delegate decision — including the clone of a Tier A repo whose hook delegates — is made inside
/// `all_hooks`.
pub trait Preflight: Send + Sync + Debug {
    /// `Err(reason)` ⇒ the hook delegates.
    fn check(
        &self,
        source: NativeSource,
        id: &str,
        epoch: Epoch,
        args: &[Arc<str>],
    ) -> Result<(), String>;
}

/// The seam snp-run uses (S12); [`crate::StoreResolver`] is the real implementation.
pub trait Resolve: Send + Sync + Debug {
    /// `all_hooks()`: every repo in config order; background prefetch of eligible clones (`NetLimiter`, ADR-064) and
    /// `[INFO] Initializing environment for …` printed in config order at pre-commit's moment (marker-driven, with
    /// the foreground clone exactly as pre-commit when the prefetch is absent or failed); manifest load
    /// (+ deprecated-stage warning on a new clone); unknown-id error; merge (+ overlay hook patch); language default
    /// resolution; env-less guards; fast-path decisions G1–G6 incl. G5 through `cx.preflight` and the clone of
    /// delegated Tier A repos.
    fn all_hooks(&self, cx: &ResolveCtx<'_>) -> Result<Vec<ResolvedHook>, Failure>;
    /// `install_hook_envs()`: dedup by `InstallKey`, `check_support` before FS mutation, per-repo locks, parallel
    /// across distinct prefixes (`InstallLimiter`), `[INFO] Installing environment for …` triplets in config order
    /// (incl. virtual ones).
    fn install_envs(&self, hooks: &[&ResolvedHook], cx: &ResolveCtx<'_>) -> Result<(), Failure>;
    /// A counter of the clones, installs and downloads this resolver made (ADR-078(3), ADR-104): two equal readings
    /// mean none happened in between. `None` (the default) means "not tracked": a caller that skips work when
    /// nothing happened must then assume something did (fail closed).
    fn store_work(&self) -> Option<u64> {
        None
    }
    /// The hook's environment; cached per `InstallKey`.
    fn hook_env(&self, h: &ResolvedHook, cx: &ResolveCtx<'_>) -> Result<Env, Failure>;
    /// The language's run plan for the hook.
    fn plan(&self, h: &ResolvedHook, cx: &ResolveCtx<'_>, color: bool) -> Result<RunPlan, Failure>;
}
