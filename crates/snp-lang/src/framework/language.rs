//! The `Language` trait, the registry and the support checks (04b §4, §5.1).
//!
//! Owner: WP-16.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-16")]
#![expect(unused_variables, reason = "W0 stub parameters, removed by WP-16")]

use std::sync::Arc;

use snp_core::brand::Brand;
use snp_core::env::EnvPatch;
use snp_core::lang::LanguageId;

use crate::{
    EnvCtx, Health, Host, InstallCtx, InstallOutcome, InstallState, LangError, PlanCtx, RunPlan,
};

/// How a language treats `language_version`.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum VersionSupport {
    /// Only `default` (`assert_version_default`).
    DefaultOnly,
    /// Any version.
    Any,
    /// The value is ignored.
    Ignored,
}

/// What a language accepts.
#[derive(Copy, Clone, Debug)]
pub struct Support {
    /// `language_version` handling.
    pub version: VersionSupport,
    /// `additional_dependencies` allowed (else `assert_no_additional_deps`).
    pub deps: bool,
}

/// One pre-commit language (`pre_commit/languages/<lang>.py`).
pub trait Language: Send + Sync + 'static {
    /// The language.
    fn id(&self) -> LanguageId;
    /// Version and dependency support.
    fn support(&self) -> Support;
    /// Memoised by `ToolProbe`.
    fn default_version(&self, h: &Host<'_>) -> Result<Arc<str>, LangError>;
    /// `env_dir` does NOT exist on entry.
    fn install(&self, cx: &InstallCtx<'_>) -> Result<InstallOutcome, LangError>;
    /// `health_check`.
    fn health(&self, cx: &EnvCtx<'_>, recorded: Option<&InstallState>)
    -> Result<Health, LangError>;
    /// `in_env` as data.
    fn activate(&self, cx: &EnvCtx<'_>) -> Result<EnvPatch, LangError>;
    /// What to run.
    fn plan(&self, cx: &PlanCtx<'_>) -> Result<RunPlan, LangError>;
}

/// Static match over the ZSTs in `langs`.
pub fn backend(id: LanguageId) -> &'static dyn Language {
    crate::langs::registry(id)
}

/// Verbatim `assert_version_default` / `assert_no_additional_deps` texts (brand-aware); run BEFORE any FS mutation
/// (04b §5.1).
pub fn check_support(
    id: LanguageId,
    version: &str,
    deps: &[Arc<str>],
    brand: Brand,
) -> Result<(), LangError> {
    todo!()
}

/// 4.6.2 `empty_template_*` bytes (03 §7.7). W0 placeholder: WP-16 embeds the resources.
pub const LOCAL_TEMPLATE: &[(&str, &[u8])] = &[];
