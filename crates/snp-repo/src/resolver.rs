//! The real resolver and the `doctor --fast-path` decision table.
//!
//! Owner: WP-31.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use snp_config::{Config, RepoConfig};
use snp_core::env::Env;
use snp_core::failure::Failure;
use snp_lang::{EnvCtx, PlanCtx, RunPlan};
use snp_store::StoreGuard;

use crate::fastpath::Policy;
use crate::install::{self, LangInstaller};
use crate::prefetch::Prefetch;
use crate::resolve::{self, RemotePlan, Resolution};
use crate::{ExecKind, HookSrc, InstallKey, Resolve, ResolveCtx, ResolvedHook, messages};

/// The store-backed [`Resolve`] implementation.
///
/// pre-commit's `repository.all_hooks` / `install_hook_envs` and `languages[lang].in_env` / `run_hook` over snp's
/// store; it holds the per-`InstallKey` activation cache and the store's shared lock from the first repository use
/// until it is dropped (gc can never delete a repository a run uses, ARCHITECTURE §5.5).
#[derive(Debug, Default)]
pub struct StoreResolver {
    /// `hook_env` results per environment (activation is computed once per process).
    envs: Mutex<HashMap<InstallKey, Env>>,
    /// The store's shared lock (taken at the first repository use).
    guard: Mutex<Option<StoreGuard>>,
}

impl StoreResolver {
    /// Takes the store's shared lock once (the store exists by then).
    fn hold_store(&self, cx: &ResolveCtx<'_>) -> Result<(), Failure> {
        let mut g = snp_core::sync::lock(&self.guard);
        if g.is_none() && cx.store.exists() {
            *g = Some(cx.store.shared(&mut resolve::lock_notice(cx), cx.cancel)?);
        }
        Ok(())
    }
}

/// Whether `cfg` needs the store at all (remote repositories, or local hooks with an environment).
fn uses_store(cfg: &Config) -> bool {
    cfg.repos.iter().any(|r| match r {
        RepoConfig::Remote { .. } => true,
        RepoConfig::Local { hooks } => hooks.iter().any(|h| h.language.env_dir_base().is_some()),
        _ => false,
    })
}

impl Resolve for StoreResolver {
    /// `all_hooks()` (`repository.py:232-237`) with clone prefetch (ADR-064) and the fast path (ADR-070); see
    /// [`crate::Resolve::all_hooks`].
    fn all_hooks(&self, cx: &ResolveCtx<'_>) -> Result<Vec<ResolvedHook>, Failure> {
        let policy = Policy::new(&cx.fast_path, cx.cfg);
        if uses_store(cx.cfg) {
            cx.store.ensure_created(cx.brand, cx.printer)?;
            self.hold_store(cx)?;
        }
        let plans: Vec<Option<RemotePlan>> = cx
            .cfg
            .repos
            .iter()
            .map(|r| match r {
                RepoConfig::Remote { repo, rev, .. } => {
                    Some(RemotePlan::new(cx, &policy, repo, rev))
                }
                _ => None,
            })
            .collect();
        let jobs = resolve::prefetch_jobs(cx, &plans);
        if jobs.is_empty() {
            return Resolution::new(cx, policy, plans, None).run();
        }
        let prefetch = Prefetch::new(jobs, cx.store.scratch_dir());
        let workers = prefetch.len().min(cx.net.permits().get());
        std::thread::scope(|s| {
            let mut handles = Vec::new();
            for k in 0..workers {
                let p = &prefetch;
                match std::thread::Builder::new()
                    .name(format!("snp-prefetch-{k}"))
                    .spawn_scoped(s, move || p.work(cx))
                {
                    Ok(h) => handles.push(h),
                    // Fewer workers: unclaimed jobs are cloned in the foreground.
                    Err(e) => tracing::debug!("prefetch worker refused: {e}"),
                }
            }
            let result = Resolution::new(cx, policy, plans, Some(&prefetch)).run();
            prefetch.abandon();
            for h in handles {
                let _joined = h.join();
            }
            prefetch.cleanup();
            result
        })
    }

    /// `install_hook_envs()` (`repository.py:214-229`); see [`crate::Resolve::install_envs`].
    fn install_envs(&self, hooks: &[&ResolvedHook], cx: &ResolveCtx<'_>) -> Result<(), Failure> {
        let entries = install::entries(hooks, cx.store)?;
        if entries.is_empty() {
            return Ok(());
        }
        cx.store.ensure_created(cx.brand, cx.printer)?;
        self.hold_store(cx)?;
        install::run(
            &LangInstaller { cx },
            &entries,
            cx.printer,
            cx.installs,
            cx.cancel,
        )
    }

    /// `languages[lang].in_env(prefix, version)` as data, cached per `InstallKey`; env-less and native hooks get the
    /// run environment unchanged.
    fn hook_env(&self, h: &ResolvedHook, cx: &ResolveCtx<'_>) -> Result<Env, Failure> {
        let (Some(key), Some(repo)) = (&h.install_key, &h.repo) else {
            return Ok(cx.env.clone());
        };
        if let Some(env) = snp_core::sync::lock(&self.envs).get(key) {
            return Ok(env.clone());
        }
        let env_dir = cx.store.env_dir(repo, key.lang, &key.version);
        let patch = snp_lang::backend(key.lang)
            .activate(&EnvCtx {
                host: &cx.host,
                prefix: &h.prefix,
                version: &key.version,
                env_dir: &env_dir,
            })
            .map_err(Failure::from)?;
        let env = cx.env.patched(&patch);
        snp_core::sync::lock(&self.envs).insert(key.clone(), env.clone());
        Ok(env)
    }

    /// `languages[lang].run_hook` as a [`RunPlan`] (the hazmat rewrite happens in the backends' `hook_cmd`).
    ///
    /// # Errors
    ///
    /// An `AssertionError` for a native or meta hook (snp-run runs those itself), the backend's error otherwise.
    fn plan(&self, h: &ResolvedHook, cx: &ResolveCtx<'_>, color: bool) -> Result<RunPlan, Failure> {
        if !matches!(h.exec, ExecKind::Language) {
            return Err(Failure::unexpected(
                "AssertionError",
                messages::no_language_plan(&h.def.id),
            ));
        }
        let env = self.hook_env(h, cx)?;
        let env_dir = match (&h.install_key, &h.repo) {
            (Some(key), Some(repo)) => Some(cx.store.env_dir(repo, key.lang, &key.version)),
            _ => None,
        };
        snp_lang::backend(h.def.language)
            .plan(&PlanCtx {
                host: &cx.host,
                prefix: &h.prefix,
                version: &h.language_version,
                env_dir: env_dir.as_deref(),
                env: &env,
                entry: &h.def.entry,
                args: &h.def.args,
                is_local: matches!(h.src, HookSrc::Local),
                color,
                docker_tag: h.docker_tag.as_deref(),
            })
            .map_err(Failure::from)
    }
}

/// `(hook id, decision, reason)` rows; wrapped by `snp_run::doctor_fast_path`.
///
/// 09 §6.8 `snp doctor fast-path`: for every remote and builtin hook of `cfg`, the fast-path decision `all_hooks`
/// would make (`native`, `delegate`, `language` for hooks that are not pre-commit-hooks hooks, `undecided` for a
/// Tier B repository that is not cloned yet, `error` for a hook that would not resolve). Pure: nothing is cloned or
/// printed.
pub fn decision_table(cfg: &Config, cx: &ResolveCtx<'_>) -> Vec<(Arc<str>, Arc<str>, String)> {
    resolve::table(cfg, cx)
}
