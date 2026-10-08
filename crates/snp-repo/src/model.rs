//! Resolution context and the resolved-hook model (§2.1).
//!
//! Owner: WP-31.

use std::path::PathBuf;
use std::sync::Arc;

use snp_config::overlay::Overlay;
use snp_config::{Config, HookDef, ext::FastPath};
use snp_core::brand::Brand;
use snp_core::cancel::CancelToken;
use snp_core::env::Env;
use snp_core::lang::LanguageId;
use snp_core::output::{Printer, WarnSink};
use snp_git::{GitBackend, GitExe};
use snp_hooks_data::{Epoch, NativeSource};
use snp_lang::Host;
use snp_proc::pool::{JobPool, Limiter};
use snp_pyre::RegexCache;
use snp_store::{RepoHandle, RepoKey, Store};

use crate::Preflight;

/// Everything resolution needs (per project, §1.6).
#[derive(Debug)]
pub struct ResolveCtx<'a> {
    /// The loaded config.
    pub cfg: &'a Config,
    /// The store.
    pub store: &'a Store,
    /// The resolved git executable.
    pub git_exe: &'a Arc<GitExe>,
    /// The run's Tool-role backend.
    pub git_backend: Arc<dyn GitBackend>,
    /// The run environment.
    pub env: &'a Env,
    /// The output sink.
    pub printer: &'a Printer,
    /// The job pool.
    pub pool: &'a JobPool,
    /// `NetLimiter` (8).
    pub net: &'a Limiter,
    /// `InstallLimiter`.
    pub installs: &'a Limiter,
    /// The run's cancel token.
    pub cancel: &'a CancelToken,
    /// Host services for the language backends.
    pub host: Host<'a>,
    /// Fast-path policy (§7.2).
    pub fast_path: FastPathPolicy,
    /// G5 (snp-run's impl over `snp_hooks::native`).
    pub preflight: &'a dyn Preflight,
    /// The local overlay's hook patches.
    pub overlay: Option<&'a Overlay>,
    /// The run's regex cache.
    pub regexes: &'a RegexCache,
    /// `[WARNING] …` sink.
    pub warn: &'a dyn WarnSink,
    /// The brand.
    pub brand: Brand,
}

/// G1 inputs (`--no-fast-path`, `SNP_NO_FAST_PATH`, root `fast_path*` keys) and `install-hooks --with-fallback`.
#[derive(Clone, Debug)]
pub struct FastPathPolicy {
    /// `fast_path`.
    pub mode: FastPath,
    /// `fast_path_exclude`.
    pub exclude: Vec<Arc<str>>,
    /// `fast_path_coalesce`.
    pub coalesce: bool,
    /// Also provision delegate environments of native hooks.
    pub with_fallback: bool,
}

/// Where a hook comes from.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum HookSrc {
    /// A remote repository.
    Remote {
        /// `repo`.
        repo: Arc<str>,
        /// `rev`.
        rev: Arc<str>,
    },
    /// `repo: local`.
    Local,
    /// `repo: meta`.
    Meta,
    /// `repo: builtin`.
    Builtin,
}

/// pre-commit's meta hooks (`meta_hooks/`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum MetaHook {
    /// `check-hooks-apply`.
    CheckHooksApply,
    /// `check-useless-excludes`.
    CheckUselessExcludes,
    /// `identity`.
    Identity,
}

/// Data only (snp-repo does not depend on snp-hooks): snp-run maps (source, id) to the impl via `snp_hooks::native`.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum ExecKind {
    /// Run through the language backend's `RunPlan`.
    Language,
    /// Run natively.
    Native {
        /// Which registry.
        source: NativeSource,
        /// The hook id.
        id: Arc<str>,
        /// The pre-commit-hooks epoch.
        epoch: Epoch,
    },
    /// A meta hook.
    Meta(MetaHook),
}

/// One environment: repository, language, version and dependencies.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct InstallKey {
    /// The repository.
    pub repo: RepoKey,
    /// The language.
    pub lang: LanguageId,
    /// The resolved `language_version`.
    pub version: Arc<str>,
    /// `additional_dependencies`.
    pub deps: Arc<[Arc<str>]>,
}

/// A hook ready to schedule (pre-commit's `Hook` + `prefix`).
#[derive(Clone, Debug)]
pub struct ResolvedHook {
    /// Flattened config index.
    pub index: u32,
    /// Where it comes from.
    pub src: HookSrc,
    /// The merged (and overlay-patched) definition.
    pub def: Arc<HookDef>,
    /// The hook repository checkout (pre-commit's `prefix`).
    pub prefix: PathBuf,
    /// The materialised repository, if any.
    pub repo: Option<RepoHandle>,
    /// The resolved `language_version` (language default applied).
    pub language_version: Arc<str>,
    /// How it runs.
    pub exec: ExecKind,
    /// The environment to install, if any.
    pub install_key: Option<InstallKey>,
    /// Tier-A message parity (ADR-032).
    pub virtual_install: Option<InstallKey>,
    /// docker image tag (ADR-035).
    pub docker_tag: Option<Arc<str>>,
}
